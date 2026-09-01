//! workshop_record — the persistent SOURCE OF TRUTH for the Workshop plane (ADR-002 §4, ADR-003 promote).
//!
//! A **Workshop** is the reproducible-environment + authority-template noun (ADR-002): a *named,
//! declarative recipe* produced by `promote`-ing a Bench. Where a [`crate::bench_record`] captures the
//! MUTABLE mess (a persistent container home you `apt`/`pip` into), a Workshop recipe captures the
//! reviewed INTENT — the base seed, the declared package sets, the declared-maximum FS/egress requests,
//! and the exported launchers — so the same environment can be RE-DERIVED (ADR-002 core law #5:
//! "promotion captures intent, not filesystem debris"). The recipe is the authoritative persistence; the
//! derived container bytes (the later Tool Shed cache) are a DISPOSABLE optimization keyed off this recipe,
//! never an independent source of authority.
//!
//! SECURITY — same record-forgery anchor as the Bench records (bench_record.rs:41-48): the recipes live
//! directly under the ROOT-OWNED `/home/.shrek/workshops` (root:root 0755), a SIBLING of the Bench
//! `records/` dir and NOT inside the `dev`-owned container pool. `dev` can neither create an entry there
//! nor `rename(2)` the dir aside, so a recipe can only be written by the privileged supervisor — a recipe
//! is a declared-authority template, so a forged one would be an authority-laundering gadget. The
//! individual recipes stay `root`-owned world-readable (0644) so an unprivileged `workshop list`/`show`
//! reads them directly without a broker hop.
//!
//! Same dep-free line-text discipline as [`crate::bench_record`]: a fixed versioned header, one field per
//! line, atomic temp+rename, and a fail-closed parser (any missing/malformed required field or an unknown
//! field ⇒ the whole record parses to `None`, treated as absent). The `apt`/`pip` package tokens are held
//! to a strict charset grammar ([`valid_pkg_token`]) so a recorded token can never smuggle a leading `-`
//! (a flag) or whitespace into the later `apt`/`pip` argv at launch.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{chown, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::bench_record::{bench_env, valid_bench_name};

/// Default location of the durable Workshop recipes — a SIBLING of the Bench `records/` dir, directly
/// under the root-owned `/home/.shrek` forgery anchor (never inside the `dev`-owned pool). Overridable
/// for the host/container oracle (no `/home`) via `SHREK_WORKSHOP_DIR` in the `oracle-env` build only —
/// the shipped image compiles [`bench_env`] out to a const `None`, so the sealed default is unconditional.
pub fn workshops_dir() -> PathBuf {
    bench_env("SHREK_WORKSHOP_DIR")
        .unwrap_or_else(|| "/home/.shrek/workshops".to_string())
        .into()
}

/// A declared package token for `apt`/`pip`. Grammar (Fable item-3 must-fix 5): a charset allowlist, no
/// leading `-` (so it can NEVER reach `apt`/`pip` as a flag at launch), no whitespace (so it stays one
/// newline-free record line and one argv vector), non-empty, `<= 128`. The allowed non-alnum bytes cover
/// the two package-spec dialects the workshop seeds use: `=`/`==` version pins (apt `sl=3.03-17`, pip
/// `six==1.17.0`), plus the version punctuation `. - _ + : ~` (debian epochs `2:`, tildes, local `+`
/// versions) and `/` (a pip direct path/extra index is NOT admitted here — only registry names + pins).
pub fn valid_pkg_token(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && !s.starts_with('-')
        && s.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'+' | b'=' | b':' | b'~')
        })
}

/// The durable state of one Workshop recipe. Mirrors [`crate::bench_record::BenchRecord`]'s line-text
/// model but for a REPRODUCIBLE recipe: `seed` is the sealed base image the environment re-derives from;
/// `apt`/`pip` are the declared package sets `launch` installs; `grants` (opaque `fs-ro`/`fs-rw`/`net`
/// encoded forms, identical to the Bench record's) are the DECLARED-MAXIMUM authority requests (ADR-002
/// §5 — a recipe REQUESTS, it does not grant); `exports` are the launcher apps carried over. `source` is
/// build provenance: the Bench this recipe was promoted from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkshopRecord {
    pub name: String,
    /// The sealed seed-catalog NAME the environment re-derives from (copied from the source Bench; the
    /// promote path validates it against the sealed catalog before writing).
    pub seed: String,
    /// creation time (unix seconds). Passed by the caller — the sealed daemons avoid wall-clock reads.
    pub created: u64,
    /// build provenance: the Bench name this recipe was promoted from.
    pub source: String,
    /// declared apt package tokens (each [`valid_pkg_token`]). Order preserved as declared.
    pub apt: Vec<String>,
    /// declared pip package tokens (each [`valid_pkg_token`]). Order preserved as declared.
    pub pip: Vec<String>,
    /// declared-maximum authority requests: opaque `fs-ro <path>` / `fs-rw <path>` / `net <profile>`
    /// encoded forms, byte-identical to the Bench record's `grant` lines (parsed by `bench_plane::Grant`).
    pub grants: Vec<String>,
    /// exported launcher apps: opaque `Export::encode` strings, byte-identical to the Bench record's
    /// `export` lines (parsed by `bench_plane::Export`). Empty when the source Bench exported nothing.
    pub exports: Vec<String>,
}

impl WorkshopRecord {
    /// Serialize to the dep-free line-text wire form. Package tokens are charset-validated (no newline
    /// possible); grant/export lines are guarded against a newline by [`write_record`] before this runs.
    fn to_wire(&self) -> String {
        let mut s = String::from("SHREK-WORKSHOP 1\n");
        s.push_str(&format!("name {}\n", self.name));
        s.push_str(&format!("seed {}\n", self.seed));
        s.push_str(&format!("created {}\n", self.created));
        s.push_str(&format!("source {}\n", self.source));
        for p in &self.apt {
            s.push_str(&format!("apt {p}\n"));
        }
        for p in &self.pip {
            s.push_str(&format!("pip {p}\n"));
        }
        for g in &self.grants {
            s.push_str(&format!("grant {g}\n"));
        }
        for e in &self.exports {
            s.push_str(&format!("export {e}\n"));
        }
        s.push_str("END\n");
        s
    }

    /// Parse from the wire form. Fail-closed: a bad header, any missing required field, an unknown field,
    /// a package token that fails the grammar, or a missing `END` all return `None` (a corrupt recipe is
    /// treated as absent — never partially applied).
    fn from_wire(body: &str) -> Option<WorkshopRecord> {
        let mut lines = body.lines();
        if lines.next()? != "SHREK-WORKSHOP 1" {
            return None;
        }
        let mut name = None;
        let mut seed = None;
        let mut created = None;
        let mut source = None;
        let mut apt = Vec::new();
        let mut pip = Vec::new();
        let mut grants = Vec::new();
        let mut exports = Vec::new();
        let mut saw_end = false;
        for line in lines {
            if line == "END" {
                saw_end = true;
                break;
            }
            let (k, v) = line.split_once(' ')?;
            match k {
                "name" => name = Some(v.to_string()),
                "seed" => seed = Some(v.to_string()),
                "created" => created = Some(v.parse::<u64>().ok()?),
                "source" => source = Some(v.to_string()),
                // a recorded package token MUST still satisfy the grammar — a record that somehow holds a
                // flag-shaped or whitespace token is corrupt, so fail the whole parse (never launch it).
                "apt" => {
                    if !valid_pkg_token(v) {
                        return None;
                    }
                    apt.push(v.to_string());
                }
                "pip" => {
                    if !valid_pkg_token(v) {
                        return None;
                    }
                    pip.push(v.to_string());
                }
                "grant" => grants.push(v.to_string()),
                "export" => exports.push(v.to_string()),
                _ => return None, // unknown field ⇒ fail closed (never silently ignore)
            }
        }
        let rec = WorkshopRecord {
            name: name?,
            seed: seed?,
            created: created?,
            source: source?,
            apt,
            pip,
            grants,
            exports,
        };
        // The recipe's own name must be a safe token (defence in depth: the filename already is one).
        if !saw_end || !valid_bench_name(&rec.name) {
            return None;
        }
        Some(rec)
    }
}

/// Resolve `dev` gid from /etc/passwd so recipes are group-readable by the desktop user's `workshop list`.
/// Returns `None` if absent (recipe then stays root-owned; still 0644-readable). Mirrors bench_record.
fn dev_gid() -> Option<u32> {
    let passwd = fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() >= 4 && f[0] == "dev").then(|| f[3].parse().ok()).flatten()
    })
}

/// Write (create or replace) a Workshop recipe. Atomic temp+rename so a reader never sees a partial
/// recipe; a re-promote of the same name atomically REPLACES the prior recipe. Mode 0644 in a 0755 dir —
/// a recipe is a declared-authority TEMPLATE (not a secret), so an unprivileged reader may list/show it.
pub fn write_record(dir: &Path, rec: &WorkshopRecord) -> io::Result<PathBuf> {
    if !valid_bench_name(&rec.name) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid workshop name"));
    }
    for p in rec.apt.iter().chain(rec.pip.iter()) {
        if !valid_pkg_token(p) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid package token"));
        }
    }
    // grant/export lines are copied from the (already newline-guarded) Bench record, but re-guard here so
    // a caller bug can never forge extra record lines by smuggling a newline into a grant/export value.
    for line in rec.grants.iter().chain(rec.exports.iter()) {
        if line.contains('\n') {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "grant/export entry contains newline"));
        }
    }
    fs::create_dir_all(dir)?;
    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o755));
    let path = dir.join(&rec.name);
    let tmp = dir.join(format!(".{}.tmp", rec.name));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(rec.to_wire().as_bytes())?;
        f.sync_all()?;
    }
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o644))?;
    if let Some(gid) = dev_gid() {
        // Best-effort: root-owned but `dev`-group so an unprivileged `workshop list` can read.
        let _ = chown(dir, Some(0), Some(gid));
    }
    fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Load a Workshop recipe by name. Fail-closed: a missing/malformed recipe returns `None`.
pub fn load_record(dir: &Path, name: &str) -> Option<WorkshopRecord> {
    if !valid_bench_name(name) {
        return None;
    }
    let body = fs::read_to_string(dir.join(name)).ok()?;
    WorkshopRecord::from_wire(&body)
}

/// Remove a Workshop recipe (teardown). Idempotent — a missing recipe is not an error.
pub fn remove_record(dir: &Path, name: &str) -> io::Result<()> {
    if !valid_bench_name(name) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid workshop name"));
    }
    match fs::remove_file(dir.join(name)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// List all valid Workshop recipes (for `workshop list`). Skips temp files and any recipe that fails to
/// parse — a corrupt recipe is invisible, not fatal. Sorted by name for a stable listing.
pub fn list_records(dir: &Path) -> Vec<WorkshopRecord> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else { return out };
    for ent in rd.flatten() {
        let Some(fname) = ent.file_name().to_str().map(str::to_owned) else { continue };
        if fname.starts_with('.') {
            continue; // skip `.name.tmp` and any dotfiles
        }
        if let Some(rec) = load_record(dir, &fname) {
            out.push(rec);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let base = std::env::var("CARGO_TARGET_TMPDIR").unwrap_or_else(|_| "/tmp".into());
        let d = PathBuf::from(base)
            .join(format!("workshop-rec-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = fs::remove_dir_all(&d);
        let _ = fs::create_dir_all(&d);
        d
    }

    fn rec(name: &str) -> WorkshopRecord {
        WorkshopRecord {
            name: name.into(),
            seed: "debian".into(),
            created: 42,
            source: "workbench".into(),
            apt: vec!["sl".into(), "sl=3.03-17".into()],
            pip: vec!["six==1.17.0".into()],
            grants: vec!["fs-ro /home/dev/in".into(), "net debian-apt".into()],
            exports: vec!["hello shrek-bench-wb-hello.desktop app-x label%20x sh -c echo".into()],
        }
    }

    #[test]
    fn pkg_token_grammar_admits_pins_blocks_flags_and_whitespace() {
        assert!(valid_pkg_token("sl"));
        assert!(valid_pkg_token("python3-dev"));
        assert!(valid_pkg_token("sl=3.03-17")); // apt pin
        assert!(valid_pkg_token("six==1.17.0")); // pip pin
        assert!(valid_pkg_token("2:1.2.3~rc1+deb")); // debian epoch/tilde/local
        assert!(!valid_pkg_token("")); // empty
        assert!(!valid_pkg_token("--force-yes")); // leading dash = a flag
        assert!(!valid_pkg_token("-rf")); // leading dash
        assert!(!valid_pkg_token("a b")); // whitespace
        assert!(!valid_pkg_token("pkg\nname")); // newline (record-line forgery)
        assert!(!valid_pkg_token("http://evil/x")); // `/` not admitted (no direct paths)
        assert!(!valid_pkg_token(&"x".repeat(129))); // length cap
    }

    #[test]
    fn write_then_load_roundtrips_all_fields() {
        let d = tmpdir();
        let r = rec("wsx");
        let p = write_record(&d, &r).unwrap();
        assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o644);
        let got = load_record(&d, "wsx").unwrap();
        assert_eq!(got, r);
    }

    #[test]
    fn write_replaces_and_remove_is_idempotent() {
        let d = tmpdir();
        write_record(&d, &rec("w")).unwrap();
        let mut r2 = rec("w");
        r2.apt = vec!["cowsay".into()];
        write_record(&d, &r2).unwrap(); // atomic replace
        assert_eq!(load_record(&d, "w").unwrap().apt, vec!["cowsay".to_string()]);
        remove_record(&d, "w").unwrap();
        assert!(load_record(&d, "w").is_none());
        remove_record(&d, "w").unwrap(); // idempotent
    }

    #[test]
    fn empty_package_sets_roundtrip() {
        let d = tmpdir();
        let mut r = rec("bare");
        r.apt.clear();
        r.pip.clear();
        r.grants.clear();
        r.exports.clear();
        write_record(&d, &r).unwrap();
        assert_eq!(load_record(&d, "bare").unwrap(), r);
    }

    #[test]
    fn malformed_recipes_fail_closed() {
        let d = tmpdir();
        let p = d.join("bad");
        for bad in [
            "garbage",
            "SHREK-WORKSHOP 2\nname bad\nseed debian\ncreated 0\nsource b\nEND\n", // wrong version
            "SHREK-WORKSHOP 1\nname bad\nseed debian\ncreated notnum\nsource b\nEND\n", // bad created
            "SHREK-WORKSHOP 1\nname bad\nseed debian\ncreated 0\nsource b\n", // no END
            "SHREK-WORKSHOP 1\nseed debian\ncreated 0\nsource b\nEND\n", // no name
            "SHREK-WORKSHOP 1\nname bad\ncreated 0\nsource b\nEND\n", // no seed
            "SHREK-WORKSHOP 1\nname bad\nseed debian\ncreated 0\nEND\n", // no source
            "SHREK-WORKSHOP 1\nname bad\nseed debian\ncreated 0\nsource b\nbogus x\nEND\n", // unknown field
            "SHREK-WORKSHOP 1\nname bad\nseed debian\ncreated 0\nsource b\napt --force-yes\nEND\n", // flag token
        ] {
            fs::write(&p, bad).unwrap();
            assert!(load_record(&d, "bad").is_none(), "should reject: {bad:?}");
        }
    }

    #[test]
    fn write_rejects_bad_name_flag_token_and_newline_grant() {
        let d = tmpdir();
        assert!(write_record(&d, &rec("../escape")).is_err());
        let mut r = rec("ok");
        r.apt = vec!["--force-yes".into()];
        assert!(write_record(&d, &r).is_err());
        let mut r2 = rec("ok2");
        r2.grants = vec!["fs-ro /home/dev/a\nb".into()];
        assert!(write_record(&d, &r2).is_err());
    }

    #[test]
    fn list_skips_temp_and_corrupt_recipes() {
        let d = tmpdir();
        write_record(&d, &rec("a")).unwrap();
        write_record(&d, &rec("c")).unwrap();
        fs::write(d.join(".a.tmp"), "half").unwrap(); // temp: skipped
        fs::write(d.join("z"), "corrupt").unwrap(); // corrupt: skipped
        let names: Vec<String> = list_records(&d).into_iter().map(|r| r.name).collect();
        assert_eq!(names, vec!["a".to_string(), "c".to_string()]);
    }
}
