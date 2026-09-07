//! catalog — the two-dir capability-manifest LOADER for the data-driven desktop-egress layer
//! (ADR-009 v2, S2a). The filesystem half of [`shrek_policy::egress_capability`]: that crate is the
//! pure grammar+catalog logic (no I/O); THIS module reads the two on-disk authoring sources, parses
//! each `*.capability` file through the sealed grammar, and merges them into a [`Catalog`] with
//! sealed-always-wins collision resolution.
//!
//! Two authoring sources (ADR-009 §4.2), NO other search path ever:
//! ```text
//! /usr/lib/shrek/egress-capabilities/<name>.capability   # SEALED: dm-verity, shipped in the image
//! /home/.shrek-system/egress/manifests/<name>.capability # OWNER:  root:root 0700, written ONLY by
//!                                                         #         egressd on a confirmed ceremony verb
//! ```
//! and a VOLATILE staging dir the ceremony (gatekeeperd, S3) stages a candidate into, from which the
//! `confirmed-manifest-install` verb (S2f) reads before egressd commits it to the OWNER dir:
//! ```text
//! /run/shrek/egress-manifest-staging/<name>.capability   # root:root 0700, cleared on reboot
//! ```
//!
//! Load discipline (fail-closed, per-file — ADR-009 §4.3):
//!   * Only regular files whose name ends `.capability` are read; anything else in the dir is ignored.
//!   * Each file is read defensively (regular-file-only, bounded) and parsed through
//!     [`shrek_policy::egress_capability::parse_manifest`]. A parse error ⇒ that ONE file is SKIPPED
//!     (its capability is absent — never a partial parse, never a whole-catalog failure) and the reason
//!     is journaled. A rejected manifest ⇒ no bless possible for that name (deny-by-default).
//!   * An absent/unreadable dir ⇒ that source contributes zero manifests (the other still loads).
//!   * [`shrek_policy::egress_capability::build_catalog`] then applies `[R-MF3]` sealed-always-wins:
//!     an owner file that reuses a sealed name is moved to `faulted`, never active.
//!
//! The daemon loads the catalog at boot and after every ceremony install/remove; the boot `compose-hosts`
//! oneshot and `project_state` load it too (each caller loads its own snapshot — the load is a cheap
//! two-dir read, and there is no shared mutable state to keep in sync).

use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{chown, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use shrek_policy::egress_capability::{
    build_catalog, is_system_reserved_host, parse_manifest, Catalog, Deliver, Manifest,
};

// asm-generic (x86-64) open flags — the same defensive-read guards `hosts.rs` uses (no libc dep). A
// capability file lives under a root-owned dir, but the owner dir is on the persistent `/home` plane
// that a pre-fix box could have planted a symlink/FIFO into, so we open O_NOFOLLOW|O_NONBLOCK anyway.
const O_NOFOLLOW: i32 = 0o400000;
const O_NONBLOCK: i32 = 0o4000;

/// A capability manifest is small (the ADR §4.3 example is ~180 bytes). Cap the read so a hostile giant
/// file in a pre-fix owner dir can't be slurped; a real manifest never approaches this.
pub const MAX_MANIFEST_BYTES: u64 = 8 * 1024;

/// The file suffix every capability manifest carries (ADR-009 §4.2). Files without it are ignored.
pub const CAP_SUFFIX: &str = ".capability";

// ---- locations (oracle-env overridable, mirroring `store`/`hosts`) -------------------------------

#[cfg(feature = "oracle-env")]
fn cap_env(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|s| !s.is_empty())
}
#[cfg(not(feature = "oracle-env"))]
fn cap_env(_var: &str) -> Option<String> {
    None
}

/// SEALED capability dir — dm-verity, shipped in the image. Overridable in the oracle build only.
pub fn sealed_cap_dir() -> PathBuf {
    cap_env("SHREK_EGRESS_CAP_SEALED")
        .unwrap_or_else(|| "/usr/lib/shrek/egress-capabilities".to_string())
        .into()
}

/// OWNER capability dir — `root:root 0700` on the persistent `/home` plane, written ONLY by egressd on a
/// confirmed ceremony verb. Overridable in the oracle build only.
pub fn owner_cap_dir() -> PathBuf {
    cap_env("SHREK_EGRESS_CAP_OWNER")
        .unwrap_or_else(|| "/home/.shrek-system/egress/manifests".to_string())
        .into()
}

/// VOLATILE staging dir — `root:root 0700` under `/run`, cleared on reboot. The ceremony (gatekeeperd,
/// S3) writes the confirmed candidate here; the `confirmed-manifest-install` verb reads it. In `/run`
/// (not `/home`) so a stale/half-staged candidate never survives a reboot. Overridable in the oracle build.
pub fn staging_cap_dir() -> PathBuf {
    cap_env("SHREK_EGRESS_CAP_STAGING")
        .unwrap_or_else(|| "/run/shrek/egress-manifest-staging".to_string())
        .into()
}

/// The `<dir>/<name>.capability` path for a capability name. `name` is expected to be a
/// [`shrek_policy::egress_capability::valid_capability_token`] (the caller validates); the join is a
/// single path component so no traversal is possible for a valid token.
pub fn manifest_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}{CAP_SUFFIX}"))
}

// ---- defensive read ------------------------------------------------------------------------------

/// Read a capability file DEFENSIVELY (regular-file-only via an up-front `lstat`, opened
/// `O_NOFOLLOW|O_NONBLOCK` to close the symlink-swap TOCTOU + the FIFO-open hang, bounded to
/// [`MAX_MANIFEST_BYTES`]). Any hostility/absence/error ⇒ `None`. Mirrors `hosts::defensive_read`.
fn defensive_read(path: &Path) -> Option<String> {
    let md = fs::symlink_metadata(path).ok()?;
    if !md.file_type().is_file() {
        return None; // symlink / FIFO / dir / socket / device ⇒ ignore
    }
    let mut f = fs::OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(path)
        .ok()?;
    let mut body = String::new();
    Read::by_ref(&mut f).take(MAX_MANIFEST_BYTES).read_to_string(&mut body).ok()?;
    Some(body)
}

/// Every parseable manifest in `dir`, fail-closed per file. A file that is not a regular `*.capability`,
/// or that fails [`parse_manifest`], is SKIPPED (journaled) — never a partial parse, never a whole-dir
/// failure. An absent/unreadable dir ⇒ empty. Order is by filename so the merge is deterministic. The
/// `source` label is only for the journal line; [`build_catalog`] applies the real [`Source`] tag.
fn read_manifest_dir(dir: &Path, source: &str) -> Vec<Manifest> {
    let mut names: Vec<String> = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new(); // absent/unreadable source contributes nothing (the other still loads)
    };
    for ent in rd.flatten() {
        if !ent.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        if let Some(n) = ent.file_name().to_str() {
            if n.ends_with(CAP_SUFFIX) && !n.starts_with('.') {
                names.push(n.to_string());
            }
        }
    }
    names.sort();
    let mut out: Vec<Manifest> = Vec::new();
    for n in names {
        let path = dir.join(&n);
        let Some(body) = defensive_read(&path) else {
            eprintln!("egressd[catalog]: skip {source} manifest {n}: unreadable/non-regular");
            continue;
        };
        match parse_manifest(&body) {
            Ok(m) => out.push(m),
            Err(e) => {
                // Fail-closed: a rejected manifest ⇒ its capability is absent. Journal the reason.
                eprintln!("egressd[catalog]: reject {source} manifest {n}: {}", e.reason());
            }
        }
    }
    out
}

// ---- the two-dir load ----------------------------------------------------------------------------

/// Merge the sealed + owner dirs into the catalog the daemon consumes. Pure over the two dir paths so
/// the oracle/tests point them at temp dirs; [`load_catalog`] is the production wrapper using the sealed
/// defaults. `[R-MF3]` sealed-always-wins is applied by [`build_catalog`].
pub fn load_catalog_from(sealed_dir: &Path, owner_dir: &Path) -> Catalog {
    let sealed = read_manifest_dir(sealed_dir, "sealed");
    let owner = read_manifest_dir(owner_dir, "owner");
    build_catalog(sealed, owner)
}

/// Load the merged catalog from the sealed defaults (`oracle-env` redirects them). Every daemon path
/// that needs the catalog (boot reconcile, socket tier check, compose-hosts, project_state, the manifest
/// verbs) calls this and works with the returned snapshot — the load is a cheap two-dir read.
pub fn load_catalog() -> Catalog {
    load_catalog_from(&sealed_cap_dir(), &owner_cap_dir())
}

/// The SEALED-ONLY catalog (owner dir excluded). Used by [`validate_owner_install`] for the name/host
/// collision checks, so an owner manifest can never shadow a sealed capability regardless of load order.
pub fn load_sealed_catalog() -> Catalog {
    build_catalog(read_manifest_dir(&sealed_cap_dir(), "sealed"), Vec::new())
}

// ---- ceremony install/remove (the confirmed-manifest-{install,remove} verb backends, S2f) ---------

/// Read a STAGED candidate manifest from `staging_dir` — the ceremony (gatekeeperd, S3) wrote the
/// confirmed bytes to the VOLATILE staging dir; the `confirmed-manifest-install` verb reads this,
/// re-parses + re-validates it, then egressd (the SOLE writer of the live owner dir) commits it.
/// Defensive read; `None` if absent or hostile. Keeps the socket wire a bare `verb + <name>` token — the
/// manifest content never rides the wire. Dir is a param (tests pass a tmp; the daemon passes
/// [`staging_cap_dir`]), mirroring [`load_catalog_from`].
pub fn read_staged(staging_dir: &Path, name: &str) -> Option<String> {
    defensive_read(&manifest_path(staging_dir, name))
}

/// Remove a staged candidate from `staging_dir` (after a commit, or to clear a rejected one). Idempotent.
pub fn clear_staged(staging_dir: &Path, name: &str) -> io::Result<()> {
    match fs::remove_file(manifest_path(staging_dir, name)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// §4.4 INSTALL-REFUSE — whether an OWNER manifest may be installed. Every refusal is a legible reason for
/// the ceremony render + the journal (never a silent drop):
///   * name collides with a SEALED capability (sealed always wins, `[R-MF3]`) — refuse.
///   * `deliver hosts` — refuse: lifting owner pins into host-wide `/etc/hosts` is a SEALED-only affordance
///     (§4.4 layer 1 consequence); an owner manifest must use `deliver none`.
///   * any host is reserved by sealed/system machinery ([`is_system_reserved_host`], §4.4 layer 2, PLUS a
///     belt check against every SEALED CATALOG host so a future sealed-only capability's host is covered
///     even though it is not in the compiled table) — refuse (a refusal, never a warning).
/// `sealed` is [`load_sealed_catalog`]'s result.
pub fn validate_owner_install(m: &Manifest, sealed: &Catalog) -> Result<(), String> {
    if sealed.get(&m.name).is_some() {
        return Err(format!("`{}` shadows a sealed capability (sealed always wins)", m.name));
    }
    if m.deliver == Deliver::Hosts {
        return Err(
            "`deliver hosts` is a sealed-only affordance; an owner manifest must use `deliver none`".to_string(),
        );
    }
    for r in &m.rules {
        let reserved = is_system_reserved_host(&r.host)
            || sealed
                .entries
                .iter()
                .any(|e| e.manifest.rules.iter().any(|hr| hr.host == r.host));
        if reserved {
            return Err(format!("host `{}` is reserved by sealed/system machinery", r.host));
        }
    }
    Ok(())
}

/// Atomically write an OWNER manifest to `owner_dir` (`root:root 0600` in the `0700` dir; egressd is the
/// SOLE writer of this dir, ADR-009 §4.2). The caller has already parsed + [`validate_owner_install`]-d
/// `text`. Creates the dir (ensure-store doesn't — capabilities are optional). Dir is a param (tests pass
/// a tmp; the daemon passes [`owner_cap_dir`]).
pub fn write_owner_manifest(owner_dir: &Path, name: &str, text: &str) -> io::Result<()> {
    fs::create_dir_all(owner_dir)?;
    let _ = fs::set_permissions(owner_dir, fs::Permissions::from_mode(0o700));
    let _ = chown(owner_dir, Some(0), Some(0));
    let path = manifest_path(owner_dir, name);
    let tmp = owner_dir.join(format!(".{name}{CAP_SUFFIX}.tmp"));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    let _ = chown(&tmp, Some(0), Some(0));
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// Remove an OWNER manifest from `owner_dir` (idempotent). A SEALED manifest is never removable via this
/// path (its dir is dm-verity read-only), so this only ever affects owner-installed capabilities.
pub fn remove_owner_manifest(owner_dir: &Path, name: &str) -> io::Result<()> {
    match fs::remove_file(manifest_path(owner_dir, name)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shrek_policy::egress_capability::{Source, WEATHER_MANIFEST};

    fn tmp() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let base = std::env::var("CARGO_TARGET_TMPDIR").unwrap_or_else(|_| "/tmp".into());
        let d = PathBuf::from(base).join(format!("catalog-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn owner_manifest(name: &str, host: &str) -> String {
        format!(
            "schema shrek-egress-capability/1\n\
             name {name}\ntitle T\npurpose P\nfeature owner:{name}\n\
             tier ceremony\ndeliver none\nhost {host} tcp 443\n"
        )
    }

    #[test]
    fn loads_the_sealed_weather_capability() {
        let sealed = tmp();
        let owner = tmp();
        fs::write(manifest_path(&sealed, "weather"), WEATHER_MANIFEST).unwrap();
        let cat = load_catalog_from(&sealed, &owner);
        let w = cat.get("weather").expect("weather loaded from the sealed dir");
        assert_eq!(w.source, Source::Sealed);
        assert!(w.fault.is_none());
        assert_eq!(w.manifest.rules.len(), 2);
    }

    #[test]
    fn owner_manifest_loads_and_a_sealed_collision_faults() {
        let sealed = tmp();
        let owner = tmp();
        fs::write(manifest_path(&sealed, "weather"), WEATHER_MANIFEST).unwrap();
        // a distinct owner capability → active; an owner `weather` → faulted (sealed wins, [R-MF3]).
        fs::write(manifest_path(&owner, "radar"), owner_manifest("radar", "radar.example.com")).unwrap();
        fs::write(manifest_path(&owner, "weather"), owner_manifest("weather", "evil.example.com")).unwrap();
        let cat = load_catalog_from(&sealed, &owner);
        // weather resolves to the SEALED entry, not the owner shadow.
        assert_eq!(cat.get("weather").unwrap().source, Source::Sealed);
        assert_eq!(cat.get("radar").unwrap().source, Source::Owner);
        assert_eq!(cat.faulted.len(), 1);
        assert_eq!(cat.faulted[0].manifest.name, "weather");
        assert_eq!(cat.faulted[0].source, Source::Owner);
    }

    #[test]
    fn malformed_file_is_skipped_capability_absent() {
        let sealed = tmp();
        let owner = tmp();
        fs::write(manifest_path(&sealed, "weather"), WEATHER_MANIFEST).unwrap();
        // a malformed owner file (bad schema) → skipped; the good sealed one still loads.
        fs::write(manifest_path(&owner, "broken"), "not a manifest at all\n").unwrap();
        // a non-`.capability` file is ignored entirely.
        fs::write(owner.join("README.txt"), "ignore me\n").unwrap();
        let cat = load_catalog_from(&sealed, &owner);
        assert!(cat.get("weather").is_some());
        assert!(cat.get("broken").is_none(), "a rejected manifest ⇒ capability absent");
        assert_eq!(cat.entries.len(), 1);
    }

    #[test]
    fn absent_dirs_yield_empty_catalog_never_panic() {
        let cat = load_catalog_from(Path::new("/nonexistent/sealed"), Path::new("/nonexistent/owner"));
        assert!(cat.entries.is_empty());
        assert!(cat.faulted.is_empty());
    }

    #[test]
    fn validate_owner_install_enforces_the_44_refusals() {
        use shrek_policy::egress_capability::WEATHER_MANIFEST;
        let sealed = build_catalog(vec![parse_manifest(WEATHER_MANIFEST).unwrap()], vec![]);
        let parse = |t: &str| parse_manifest(t).unwrap();

        // OK: a distinct name, `deliver none`, a fresh host.
        let ok = parse(&owner_manifest("radar", "radar.example.com"));
        assert!(validate_owner_install(&ok, &sealed).is_ok());

        // REFUSE: name collides with the sealed `weather`.
        let shadow = parse(&owner_manifest("weather", "fresh.example.com"));
        assert!(validate_owner_install(&shadow, &sealed).unwrap_err().contains("shadows a sealed"));

        // REFUSE: names a system-reserved host (weather's open-meteo forecast host).
        let reserved = parse(&owner_manifest("radar", "api.open-meteo.com"));
        assert!(validate_owner_install(&reserved, &sealed).unwrap_err().contains("reserved"));

        // REFUSE: names an agent-egress reserved host (github).
        let gh = parse(&owner_manifest("radar", "github.com"));
        assert!(validate_owner_install(&gh, &sealed).unwrap_err().contains("reserved"));

        // REFUSE: `deliver hosts` on an owner manifest (sealed-only affordance).
        let deliver = parse(
            "schema shrek-egress-capability/1\n\
             name radar\ntitle T\npurpose P\nfeature owner:radar\n\
             tier one-click\ndeliver hosts\nhost radar.example.com tcp 443\n",
        );
        assert!(validate_owner_install(&deliver, &sealed).unwrap_err().contains("deliver hosts"));
    }

    #[test]
    fn stage_install_remove_owner_manifest_roundtrip() {
        let staging = tmp();
        let owner = tmp();
        let sealed = tmp();
        let text = owner_manifest("radar", "radar.example.com");
        // ceremony stages the candidate; the verb reads it back.
        fs::write(manifest_path(&staging, "radar"), &text).unwrap();
        let staged = read_staged(&staging, "radar").expect("staged candidate readable");
        assert_eq!(staged, text);
        // commit to the live owner dir, then it loads into the catalog.
        write_owner_manifest(&owner, "radar", &staged).unwrap();
        assert_eq!(
            fs::metadata(manifest_path(&owner, "radar")).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let cat = load_catalog_from(&sealed, &owner);
        assert!(cat.get("radar").is_some(), "installed owner capability loads");
        // clear staging (idempotent) + remove the live manifest (idempotent).
        clear_staged(&staging, "radar").unwrap();
        clear_staged(&staging, "radar").unwrap();
        assert!(read_staged(&staging, "radar").is_none());
        remove_owner_manifest(&owner, "radar").unwrap();
        remove_owner_manifest(&owner, "radar").unwrap();
        assert!(load_catalog_from(&sealed, &owner).get("radar").is_none(), "removed capability is gone");
    }

    #[test]
    fn a_symlinked_manifest_is_ignored() {
        let sealed = tmp();
        let owner = tmp();
        let secret = tmp().join("secret.capability");
        fs::write(&secret, WEATHER_MANIFEST).unwrap();
        // plant a symlink named like a capability in the owner dir; defensive_read refuses to follow it.
        std::os::unix::fs::symlink(&secret, manifest_path(&owner, "weather")).unwrap();
        let cat = load_catalog_from(&sealed, &owner);
        assert!(cat.get("weather").is_none(), "a symlinked manifest must be ignored");
    }
}
