//! workshop_cache — the **Tool Shed**: a content-addressed cache of DERIVED Workshop environments (ADR-003
//! Commit 3, cache trust model in mycelium #2994). A Workshop recipe ([`crate::workshop_record`]) is the
//! authoritative persistence; re-deriving it on every `launch` (Commit 2) reinstalls the declared apt/pip
//! packages over egress each time. The Tool Shed lets a launch REUSE a previously-derived, already-verified
//! image instead — so a repeat launch (and, with `--offline`, an offline launch) skips the install egress.
//!
//! TRUST MODEL — the cache is NEVER an authority (mycelium #2994, core law: *cache is a disposable
//! optimization, never a source of authority*). Two facts make a cached image safe to reuse:
//!   1. The cache **key** is a SHA-256 of the derivation INPUTS the recipe already approved — the sealed
//!      seed's IMAGE ID (not its catalog name — an OS seed update changes the Id and MUST invalidate every
//!      entry), the declared apt/pip token sets, and the declared egress-profile set. Inputs, not outputs:
//!      the key answers "is this the same derivation the recipe describes?", never "are these bytes good?".
//!   2. Cache-entry TRUST lives in this ROOT-OWNED INDEX RECORD, not in the image bytes. The bytes sit in
//!      `dev`'s rootless podman store (same posture as the seeds — recognized by tag+Id, never trusted by
//!      hashing layer bytes). A HIT requires (a) a root-owned index record at the key, AND (b) the live
//!      image's podman Id equals the Id this record captured when the privileged supervisor drove+committed
//!      that derivation. A missing record, a malformed record, an absent image, or an Id mismatch is a MISS
//!      — and a MISS's ONLY continuation is to RE-DERIVE from the recipe through the approved seed+egress
//!      (never a near-miss, never a user-supplied tag). Delete the whole cache dir and the next launch just
//!      re-derives: the cache is pure disposable optimization.
//!
//! SECURITY — same forgery anchor + line-text discipline as [`crate::workshop_record`]: the index lives
//! under the ROOT-OWNED `/home/.shrek/workshops/cache` (a subdir of the recipes dir, root:root 0755), so
//! `dev` can neither forge an index record nor `rename(2)` the dir aside — only the privileged supervisor
//! writes an entry, and only after it drove the derivation itself. Records are a fixed versioned header,
//! one field per line, atomic temp+rename, and a fail-closed parser (any missing/malformed/unknown field ⇒
//! the whole record parses to `None`, treated as absent ⇒ a MISS ⇒ re-derive).
//!
//! NO-SECRETS — the cached bytes are software-only by construction: the derivation container is PRISTINE
//! (seed + install egress only — no user grants, no `/work`, no workload) and is committed BEFORE any user
//! workload ever runs in it (see `bench_plane::derive_pristine`). So a cache entry can be shared across
//! workshops without leaking a networked install's fetched secret into a durable reusable artifact.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{chown, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::workshop_record::workshops_dir;

/// The cache SCHEMA version, folded into every key. Bumping it changes every key and so invalidates the
/// entire index at once (an intentional flag-day when the derivation semantics change).
pub const CACHE_SCHEMA: u32 = 1;

/// The root-owned Tool Shed index dir — a subdir of the recipes dir, so it inherits the same `/home/.shrek`
/// forgery anchor and the same `SHREK_WORKSHOP_DIR` oracle override in one place.
pub fn cache_dir() -> PathBuf {
    workshops_dir().join("cache")
}

/// Is `s` a well-formed cache key — exactly 64 lowercase hex chars (a SHA-256 digest)? Used both to validate
/// a computed key and to guard the key as a path component (it can only ever be `[0-9a-f]{64}`, never `..`).
pub fn valid_cache_key(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
}

/// The 12-char short key used to TAG the cached image (`localhost/shrek-cache/<key12>`). Purely a podman
/// tag for a human-legible `podman images` listing — trust rides on the full key + the recorded Id, never
/// on this tag. Callers pass a [`valid_cache_key`]; a shorter string (only reachable in tests) is used
/// whole rather than panicking on the slice.
pub fn image_tag(key: &str) -> String {
    format!("localhost/shrek-cache/{}", key.get(..12).unwrap_or(key))
}

/// The canonical byte string that gets SHA-256'd into a cache key. Deterministic: each package/egress set is
/// SORTED and every value sits on its own LABELLED line, so `{apt:[a,b]}` can never collide with
/// `{apt:[a],pip:[b]}` and reordering the same set yields the same key. INPUTS the recipe approves only —
/// the sealed seed's IMAGE ID, the declared apt/pip token sets (each token incl. any version pin, so
/// `pkg` ≠ `pkg=1.2.3`), and the declared egress-profile set. Deliberately NOT keyed on fs grants, exports,
/// or quota (those don't change which SOFTWARE the derivation produces). Pure — unit-tested.
pub fn canonical_key_input(seed_id: &str, apt: &[String], pip: &[String], egress: &[String]) -> String {
    let mut s = String::new();
    s.push_str(&format!("schema={CACHE_SCHEMA}\n"));
    s.push_str(&format!("seed={seed_id}\n"));
    let mut push_set = |label: &str, set: &[String]| {
        let mut v: Vec<&String> = set.iter().collect();
        v.sort();
        for item in v {
            s.push_str(&format!("{label}={item}\n"));
        }
    };
    push_set("apt", apt);
    push_set("pip", pip);
    push_set("egress", egress);
    s
}

/// One Tool Shed index record: the trust anchor for a cached derivation. `key` is the full SHA-256 (also the
/// filename); `image` is the podman tag the bytes live under; `image_id` is the podman image Id the
/// supervisor captured at commit time (the HIT check compares the LIVE Id against this — a mismatch is a
/// MISS+cleanup); `seed_id` is the seed image Id that fed the key (provenance/debugging); `apt_resolved` /
/// `pip_resolved` are the concrete versions the derivation actually installed (`dpkg-query` / `pip freeze`),
/// captured so `workshop show` can surface exactly what a cached (possibly offline) launch will reuse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheRecord {
    pub key: String,
    pub image: String,
    pub image_id: String,
    pub seed_id: String,
    pub created: u64,
    pub apt_resolved: Vec<String>,
    pub pip_resolved: Vec<String>,
}

impl CacheRecord {
    fn to_wire(&self) -> String {
        let mut s = String::from("SHREK-CACHE 1\n");
        s.push_str(&format!("key {}\n", self.key));
        s.push_str(&format!("image {}\n", self.image));
        s.push_str(&format!("image-id {}\n", self.image_id));
        s.push_str(&format!("seed-id {}\n", self.seed_id));
        s.push_str(&format!("created {}\n", self.created));
        for r in &self.apt_resolved {
            s.push_str(&format!("apt-resolved {r}\n"));
        }
        for r in &self.pip_resolved {
            s.push_str(&format!("pip-resolved {r}\n"));
        }
        s.push_str("END\n");
        s
    }

    /// Parse from the wire form. Fail-closed: a bad header, any missing required field, a malformed `key`
    /// (not 64-hex), an unknown field, or a missing `END` all return `None` — a corrupt index record is
    /// treated as absent, i.e. a cache MISS (the launch then re-derives). Provenance lines that don't fit
    /// the grammar are simply dropped (they never gate a HIT), so stale/odd provenance can't wedge a launch.
    fn from_wire(body: &str) -> Option<CacheRecord> {
        let mut lines = body.lines();
        if lines.next()? != "SHREK-CACHE 1" {
            return None;
        }
        let mut key = None;
        let mut image = None;
        let mut image_id = None;
        let mut seed_id = None;
        let mut created = None;
        let mut apt_resolved = Vec::new();
        let mut pip_resolved = Vec::new();
        let mut saw_end = false;
        for line in lines {
            if line == "END" {
                saw_end = true;
                break;
            }
            let (k, v) = line.split_once(' ')?;
            match k {
                "key" => key = Some(v.to_string()),
                "image" => image = Some(v.to_string()),
                "image-id" => image_id = Some(v.to_string()),
                "seed-id" => seed_id = Some(v.to_string()),
                "created" => created = Some(v.parse::<u64>().ok()?),
                "apt-resolved" => apt_resolved.push(v.to_string()),
                "pip-resolved" => pip_resolved.push(v.to_string()),
                _ => return None, // unknown field ⇒ fail closed (a MISS, never a partial trust)
            }
        }
        let rec = CacheRecord {
            key: key?,
            image: image?,
            image_id: image_id?,
            seed_id: seed_id?,
            created: created?,
            apt_resolved,
            pip_resolved,
        };
        // A record whose key/image_id are empty or whose key isn't a real digest is corrupt — fail closed.
        if !saw_end || !valid_cache_key(&rec.key) || rec.image.is_empty() || rec.image_id.is_empty() {
            return None;
        }
        Some(rec)
    }
}

/// Resolve `dev` gid so an unprivileged `workshop show` can read the cache provenance. Mirrors
/// [`crate::workshop_record`]; `None` ⇒ the dir stays root-owned (still 0644-readable).
fn dev_gid() -> Option<u32> {
    let passwd = fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() >= 4 && f[0] == "dev").then(|| f[3].parse().ok()).flatten()
    })
}

/// Write (create or replace) a cache index record. Atomic temp+rename so a reader never sees a partial
/// record; mode 0644 in a 0755 dir (the index is trust metadata, not a secret — `workshop show` reads it).
/// The `key` names the file, so it is re-validated here and MUST equal `rec.key`.
pub fn write_record(dir: &Path, rec: &CacheRecord) -> io::Result<PathBuf> {
    if !valid_cache_key(&rec.key) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid cache key"));
    }
    if rec.image.is_empty() || rec.image_id.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "cache record missing image/id"));
    }
    // Provenance lines are captured from container output; re-guard against an embedded newline so a
    // caller bug can never forge extra record lines (mirrors workshop_record's grant/export guard).
    for line in rec.apt_resolved.iter().chain(rec.pip_resolved.iter()) {
        if line.contains('\n') {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "resolved-version line contains newline"));
        }
    }
    fs::create_dir_all(dir)?;
    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o755));
    let path = dir.join(&rec.key);
    let tmp = dir.join(format!(".{}.tmp", rec.key));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(rec.to_wire().as_bytes())?;
        f.sync_all()?;
    }
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o644))?;
    if let Some(gid) = dev_gid() {
        let _ = chown(dir, Some(0), Some(gid));
    }
    fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Load a cache index record by key. Fail-closed: a bad key, a missing file, or a malformed record all
/// return `None` (⇒ a cache MISS). Also verifies the on-disk `key` field matches the requested key (a
/// record filed under the wrong name is corrupt).
pub fn load_record(dir: &Path, key: &str) -> Option<CacheRecord> {
    if !valid_cache_key(key) {
        return None;
    }
    let body = fs::read_to_string(dir.join(key)).ok()?;
    let rec = CacheRecord::from_wire(&body)?;
    (rec.key == key).then_some(rec)
}

/// Remove a cache index record (a stale/mismatched entry, or a manual eviction). Idempotent — a missing
/// record is not an error. Removes ONLY the root-owned index record; reclaiming the podman image bytes is
/// the caller's separate step (`podman rmi`), so a half-removed entry is still a clean MISS.
pub fn remove_record(dir: &Path, key: &str) -> io::Result<()> {
    if !valid_cache_key(key) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid cache key"));
    }
    match fs::remove_file(dir.join(key)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let base = std::env::var("CARGO_TARGET_TMPDIR").unwrap_or_else(|_| "/tmp".into());
        let d = PathBuf::from(base)
            .join(format!("workshop-cache-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = fs::remove_dir_all(&d);
        let _ = fs::create_dir_all(&d);
        d
    }

    // A real 64-hex key (sha256 of "x"); the exact value is irrelevant, only the shape.
    const K: &str = "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881";

    fn rec(key: &str) -> CacheRecord {
        CacheRecord {
            key: key.into(),
            image: image_tag(key),
            image_id: "sha256:abc123".into(),
            seed_id: "sha256:seed99".into(),
            created: 42,
            apt_resolved: vec!["sl=3.03-17".into()],
            pip_resolved: vec!["six==1.17.0".into()],
        }
    }

    #[test]
    fn valid_cache_key_shape() {
        assert!(valid_cache_key(K));
        assert!(!valid_cache_key("")); // empty
        assert!(!valid_cache_key(&"a".repeat(63))); // too short
        assert!(!valid_cache_key(&"a".repeat(65))); // too long
        assert!(!valid_cache_key(&"A".repeat(64))); // uppercase not admitted (sha256sum is lowercase)
        assert!(!valid_cache_key(&"g".repeat(64))); // non-hex
        assert!(!valid_cache_key(&format!("../{}", &"a".repeat(61)))); // path escape shape
    }

    #[test]
    fn key_input_is_canonical_and_sorted() {
        // Order within a set does not matter (sorted); the two orderings serialize identically.
        let a = canonical_key_input("seed1", &["b".into(), "a".into()], &["z".into()], &["debian-apt".into()]);
        let b = canonical_key_input("seed1", &["a".into(), "b".into()], &["z".into()], &["debian-apt".into()]);
        assert_eq!(a, b);
        assert!(a.contains("schema=1\n"));
        assert!(a.contains("seed=seed1\n"));
        assert!(a.contains("apt=a\n") && a.contains("apt=b\n"));
    }

    #[test]
    fn key_input_distinguishes_the_inputs_that_must_miss() {
        let base = canonical_key_input("seedA", &["sl".into()], &["six".into()], &["debian-apt".into()]);
        // (a) seed image-id bump ⇒ different key input ⇒ MISS (the seed-bump→miss guarantee, at the source).
        assert_ne!(base, canonical_key_input("seedB", &["sl".into()], &["six".into()], &["debian-apt".into()]));
        // (b) a package-set change ⇒ MISS.
        assert_ne!(base, canonical_key_input("seedA", &["sl".into(), "cowsay".into()], &["six".into()], &["debian-apt".into()]));
        // (c) a version pin is part of the identity: pinned ≠ unpinned.
        assert_ne!(base, canonical_key_input("seedA", &["sl=3.03-17".into()], &["six".into()], &["debian-apt".into()]));
        // (d) an egress-set change ⇒ MISS (a broader-egress derivation must not satisfy a narrower recipe).
        assert_ne!(base, canonical_key_input("seedA", &["sl".into()], &["six".into()], &["debian-apt".into(), "pypi-https".into()]));
        // (e) apt/pip are serialized SEPARATELY: the same token in apt vs pip must not collide.
        assert_ne!(
            canonical_key_input("s", &["x".into()], &[], &[]),
            canonical_key_input("s", &[], &["x".into()], &[]),
        );
    }

    #[test]
    fn write_then_load_roundtrips_all_fields() {
        let d = tmpdir();
        let r = rec(K);
        let p = write_record(&d, &r).unwrap();
        assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o644);
        assert_eq!(load_record(&d, K).unwrap(), r);
    }

    #[test]
    fn write_replaces_and_remove_is_idempotent() {
        let d = tmpdir();
        write_record(&d, &rec(K)).unwrap();
        let mut r2 = rec(K);
        r2.image_id = "sha256:def456".into();
        write_record(&d, &r2).unwrap(); // atomic replace
        assert_eq!(load_record(&d, K).unwrap().image_id, "sha256:def456");
        remove_record(&d, K).unwrap();
        assert!(load_record(&d, K).is_none());
        remove_record(&d, K).unwrap(); // idempotent
    }

    #[test]
    fn empty_provenance_roundtrips() {
        let d = tmpdir();
        let mut r = rec(K);
        r.apt_resolved.clear();
        r.pip_resolved.clear();
        write_record(&d, &r).unwrap();
        assert_eq!(load_record(&d, K).unwrap(), r);
    }

    #[test]
    fn malformed_records_fail_closed_to_a_miss() {
        let d = tmpdir();
        let p = d.join(K);
        for bad in [
            "garbage".to_string(),
            format!("SHREK-CACHE 2\nkey {K}\nimage i\nimage-id x\nseed-id s\ncreated 0\nEND\n"), // wrong version
            format!("SHREK-CACHE 1\nkey {K}\nimage i\nimage-id x\nseed-id s\ncreated notnum\nEND\n"), // bad created
            format!("SHREK-CACHE 1\nkey {K}\nimage i\nimage-id x\nseed-id s\ncreated 0\n"), // no END
            format!("SHREK-CACHE 1\nkey {K}\nimage i\nseed-id s\ncreated 0\nEND\n"), // no image-id
            format!("SHREK-CACHE 1\nkey {K}\nimage-id x\nseed-id s\ncreated 0\nEND\n"), // no image
            "SHREK-CACHE 1\nkey short\nimage i\nimage-id x\nseed-id s\ncreated 0\nEND\n".to_string(), // bad key
            format!("SHREK-CACHE 1\nkey {K}\nimage i\nimage-id x\nseed-id s\ncreated 0\nbogus q\nEND\n"), // unknown field
            format!("SHREK-CACHE 1\nkey {K}\nimage \nimage-id x\nseed-id s\ncreated 0\nEND\n"), // empty image
        ] {
            fs::write(&p, &bad).unwrap();
            assert!(load_record(&d, K).is_none(), "should reject: {bad:?}");
        }
    }

    #[test]
    fn record_filed_under_wrong_key_is_rejected() {
        let d = tmpdir();
        // A record whose body key ≠ its filename is corrupt (a moved/mis-filed entry) — treated as a MISS.
        let other = "0".repeat(64);
        fs::write(d.join(&other), rec(K).to_wire()).unwrap();
        assert!(load_record(&d, &other).is_none());
    }

    #[test]
    fn write_rejects_bad_key_and_newline_provenance() {
        let d = tmpdir();
        let mut bad_key = rec(K);
        bad_key.key = "../escape".into(); // a non-digest key names no valid file — refuse
        assert!(write_record(&d, &bad_key).is_err());
        let mut r = rec(K);
        r.image_id = String::new();
        assert!(write_record(&d, &r).is_err()); // empty id refused
        let mut r2 = rec(K);
        r2.apt_resolved = vec!["sl=3\nEVIL forged".into()];
        assert!(write_record(&d, &r2).is_err()); // newline-forgery refused
    }
}
