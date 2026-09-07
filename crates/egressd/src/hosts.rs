//! hosts — the root-authoritative `/etc/hosts` composition + the owner's bounded provider bindings
//! (ADR-008, S2). This is the #3121 fix: root, not uid 1000, is the sole author of the file every root
//! daemon's `getaddrinfo` reads.
//!
//! Three artefacts, all root-owned:
//!
//!   * The PERSISTENT binding store `/home/.shrek-system/hosts-bindings` (`root:root 0600`, dir owned by
//!     `tmpfiles.d`). One line `<provider-token> <ipv4>` per bound model provider — TOKENS, never
//!     resolvable names, so even a store corruption can only ever name the 4 sealed model brokers
//!     ([`shrek_policy::provider_bind`]). uid 1000 can neither read nor write it; its only influence is
//!     the egressd `bind`/`unbind` verb, which lands here through [`write_binding`]/[`remove_binding`].
//!   * The `/run` PROJECTION `/run/shrek/hosts` (`root:root 0644`) — the real hosts-syntax file
//!     `/etc/hosts` symlinks to (S3). Composed from a SEALED-IN-CODE baseline ([`HOSTS_BASELINE`],
//!     localhost) plus one `<ipv4> <sealed-host>` line per readable binding. localhost lives in code, so
//!     a fresh box resolves it and uid 1000 can never break it.
//!   * The legacy path `/home/.shrek-system/hosts` — the PRE-fix uid-1000-owned store. Migration reads it
//!     DEFENSIVELY, absorbs only the 4 model-name lines into the new store, discards everything else, and
//!     overwrites it with a root-owned localhost baseline (rollback-compat + re-own; ADR-008 §7).
//!
//! [`compose_hosts`] is the whole routine, run by the base `shrek-hosts-compose` oneshot at boot AND by
//! the daemon after every `bind`/`unbind`. It ALWAYS installs the projection from the baseline plus
//! whatever bindings are readable — store absence/unreadability can NEVER block localhost (`[R2-MF1]`,
//! the first-boot ordering guarantee: `tmpfiles` runs after this oneshot on a virgin disk).

use shrek_policy::egress_capability::{is_sealed_deliverable_host, Catalog};
use shrek_policy::provider_bind::{provider_host, valid_bind_addr, PROVIDER_BINDINGS};
use std::fs;
use std::io::{self, Read, Write};
use std::net::Ipv4Addr;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{chown, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

// asm-generic (x86-64) open flags — the `[R1-MF5]` defensive-read guards. No `libc` dep (this crate
// carries its own thin `uapi`), so the two constants are spelled out here as they are in
// `include/uapi/asm-generic/fcntl.h`.
const O_NOFOLLOW: i32 = 0o400000;
const O_NONBLOCK: i32 = 0o4000;

/// A legacy/store hosts file is tiny; cap the defensive read so a hostile giant file can't be slurped.
const MAX_READ_BYTES: u64 = 64 * 1024;

/// The sealed baseline of `/run/shrek/hosts`, baked into the binary — NEVER read from a writable file.
/// The image ships no `libnss-myhostname`, so without this `localhost` is unresolvable via NSS `files`.
pub const HOSTS_BASELINE: &str = "127.0.0.1 localhost\n::1 localhost\n";

// ---- locations (oracle-env overridable, mirroring `store`) ---------------------------------------

#[cfg(feature = "oracle-env")]
fn hosts_env(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|s| !s.is_empty())
}
#[cfg(not(feature = "oracle-env"))]
fn hosts_env(_var: &str) -> Option<String> {
    None
}

/// The persistent-plane dir holding the binding store + the legacy file. `tmpfiles.d/shrek-home.conf`
/// declares it `root:root 0700` (the SINGLE ownership authority — this module never chowns it, `[R1-MF4]`).
pub fn hosts_home_dir() -> PathBuf {
    hosts_env("SHREK_HOSTS_HOME")
        .unwrap_or_else(|| "/home/.shrek-system".to_string())
        .into()
}

/// The `/run` dir holding the world-readable projection. `0755` (uid 1000 traverses to read the `0644`
/// projection, owns nothing). One level ABOVE the egress store's `/run/shrek/egress`.
pub fn hosts_run_dir() -> PathBuf {
    hosts_env("SHREK_HOSTS_RUN").unwrap_or_else(|| "/run/shrek".to_string()).into()
}

/// The token-format binding store (`root:root 0600`).
pub fn bindings_file(home: &Path) -> PathBuf {
    home.join("hosts-bindings")
}
/// The pre-fix uid-1000-owned hosts file (migration source; then a root localhost baseline for rollback).
pub fn legacy_hosts_file(home: &Path) -> PathBuf {
    home.join("hosts")
}
/// The composed projection `/etc/hosts` symlinks to (`root:root 0644`).
pub fn projection_file(run: &Path) -> PathBuf {
    run.join("hosts")
}

// ---- one binding --------------------------------------------------------------------------------

/// A bound provider: a closed-set TOKEN and the IPv4 its sealed name resolves to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub token: String,
    pub addr: Ipv4Addr,
}

/// Reverse the sealed `token → host` map: the provider token whose sealed name is `host`, if any. Used
/// only by migration to recognize a legacy `<ip> <sealed-host>` line.
fn token_for_host(host: &str) -> Option<&'static str> {
    PROVIDER_BINDINGS.iter().find(|b| b.host == host).map(|b| b.token)
}

// ---- atomic write + defensive read --------------------------------------------------------------

/// Write `body` to `path` atomically (temp + fsync + rename), `mode`, best-effort `root:root`. The temp
/// carries the target mode BEFORE the rename (never briefly world-readable), and a rename over an
/// existing symlink/FIFO replaces the DIR ENTRY — never writes THROUGH it — so this is safe even against
/// a hostile pre-fix target `[R1-MF5]`.
fn atomic_write(path: &Path, body: &str, mode: u32) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no parent"))?;
    // Root default, NO chown of the dir — tmpfiles owns `/home/.shrek-system` `[R1-MF4]`. create_dir_all
    // is the `[R2-MF1]` "may mkdir the store dir" allowance so a first-boot write lands before tmpfiles.
    fs::create_dir_all(dir)?;
    let leaf = path.file_name().and_then(|s| s.to_str()).unwrap_or("f");
    let tmp = dir.join(format!(".{leaf}.tmp"));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(body.as_bytes())?;
        f.sync_all()?;
    }
    fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
    let _ = chown(&tmp, Some(0), Some(0));
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Read a file DEFENSIVELY `[R1-MF5]`: regular-file-only (an `lstat` rejects a symlink/FIFO/dir/socket up
/// front), opened `O_NOFOLLOW|O_NONBLOCK` (closes the TOCTOU symlink-swap and the FIFO-open boot-hang the
/// pre-fix uid-1000-owned dir would otherwise allow), bounded to [`MAX_READ_BYTES`]. Any hostility,
/// absence, or error ⇒ `None` (compose then falls back to the sealed baseline). Used for BOTH the legacy
/// file and the binding store, since both can be planted on a pre-fix box before the dir is re-owned.
fn defensive_read(path: &Path) -> Option<String> {
    let md = fs::symlink_metadata(path).ok()?;
    if !md.file_type().is_file() {
        return None; // symlink / FIFO / dir / socket / device ⇒ hostile, ignore
    }
    let mut f = fs::OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW | O_NONBLOCK)
        .open(path)
        .ok()?;
    let mut body = String::new();
    Read::by_ref(&mut f).take(MAX_READ_BYTES).read_to_string(&mut body).ok()?;
    Some(body)
}

// ---- the binding store --------------------------------------------------------------------------

/// Every VALID binding in the store, fail-closed per line: a line is kept only if it is exactly
/// `<token> <ipv4>`, the token maps to a sealed provider ([`provider_host`]), and the address is a strict
/// IPv4 literal ([`valid_bind_addr`], canonically re-parsed). First occurrence of a token wins (`[N-R2-2]`).
/// A missing/hostile/garbage store ⇒ empty — the projection is still composed from the baseline `[R2MF1]`.
pub fn read_bindings(home: &Path) -> Vec<Binding> {
    let Some(body) = defensive_read(&bindings_file(home)) else {
        return Vec::new();
    };
    let mut out: Vec<Binding> = Vec::new();
    for line in body.lines() {
        let mut it = line.split_whitespace();
        let (Some(tok), Some(addr_s), None) = (it.next(), it.next(), it.next()) else {
            continue; // not exactly two fields ⇒ skip
        };
        if provider_host(tok).is_none() {
            continue; // not one of the 4 sealed provider tokens ⇒ skip
        }
        let Some(addr) = valid_bind_addr(addr_s) else {
            continue; // not a strict IPv4 literal ⇒ skip
        };
        if out.iter().any(|b| b.token == tok) {
            continue; // first-wins
        }
        out.push(Binding { token: tok.to_string(), addr });
    }
    out
}

/// Rewrite the whole binding store atomically (`root:root 0600`), deterministically sorted by token.
/// Caller holds [`lock_hosts`].
fn write_bindings_all(home: &Path, all: &[Binding]) -> io::Result<()> {
    let mut lines: Vec<String> = all.iter().map(|b| format!("{} {}", b.token, b.addr)).collect();
    lines.sort();
    lines.dedup();
    let body = if lines.is_empty() { String::new() } else { format!("{}\n", lines.join("\n")) };
    atomic_write(&bindings_file(home), &body, 0o600)
}

/// Bind a provider TOKEN to `addr_str` (the `bind` verb). Re-validates BOTH the token (must map to a
/// sealed provider) and the address (strict IPv4) — defense in depth, though the parser already checked.
/// Replaces any prior binding for the token. Returns the canonical IPv4 for the audit line. Caller holds
/// [`lock_hosts`].
pub fn write_binding(home: &Path, token: &str, addr_str: &str) -> io::Result<Ipv4Addr> {
    if provider_host(token).is_none() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a sealed provider token"));
    }
    let addr = valid_bind_addr(addr_str)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "address must be an IPv4 literal"))?;
    let mut all = read_bindings(home);
    all.retain(|b| b.token != token);
    all.push(Binding { token: token.to_string(), addr });
    write_bindings_all(home, &all)?;
    Ok(addr)
}

/// Remove a provider binding (the `unbind` verb). Idempotent — an already-unbound provider is a clean
/// no-op (`[N-4]`). Refused for a non-provider token. Caller holds [`lock_hosts`].
pub fn remove_binding(home: &Path, token: &str) -> io::Result<()> {
    if provider_host(token).is_none() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a sealed provider token"));
    }
    let mut all = read_bindings(home);
    all.retain(|b| b.token != token);
    write_bindings_all(home, &all)
}

// ---- migration ----------------------------------------------------------------------------------

/// Extract only the sealed-model-name lines from a legacy hosts body. First occurrence of a token wins
/// (`[N-R2-2]`, matching glibc `files` first-match). A hosts line may carry a comment and several names.
fn extract_model_bindings(body: &str) -> Vec<Binding> {
    let mut out: Vec<Binding> = Vec::new();
    for raw in body.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split_whitespace();
        let Some(addr_s) = it.next() else { continue };
        let Some(addr) = valid_bind_addr(addr_s) else { continue };
        for host in it {
            if let Some(token) = token_for_host(host) {
                if !out.iter().any(|b| b.token == token) {
                    out.push(Binding { token: token.to_string(), addr });
                }
            }
        }
    }
    out
}

/// Migrate a pre-fix box + guarantee rollback-compat (`[R1-MF5]`/`[R1-MF6]`, ADR-008 §7). Idempotent,
/// every boot: (1) defensively read the legacy uid-1000 hosts file; (2) absorb any of the 4 model-name
/// lines into the root store WITHOUT clobbering an existing binding (first-wins); (3) overwrite the
/// legacy path with a root-owned localhost baseline — re-owning it away from uid 1000 AND leaving a sane
/// localhost-bearing file so a rollback to an old base still resolves localhost. Best-effort: a failure
/// here NEVER blocks the projection (`[R2MF1]`).
fn migrate_legacy(home: &Path) -> io::Result<()> {
    let legacy = legacy_hosts_file(home);
    // Read BEFORE we overwrite. `defensive_read` ⇒ None for a symlink/FIFO/absent legacy (hostile or
    // fresh): nothing to absorb, we still install the baseline below.
    if let Some(body) = defensive_read(&legacy) {
        let extracted = extract_model_bindings(&body);
        if !extracted.is_empty() {
            let mut all = read_bindings(home);
            for b in extracted {
                if !all.iter().any(|x| x.token == b.token) {
                    all.push(b);
                }
            }
            write_bindings_all(home, &all)?;
        }
    }
    // Always leave a root-owned localhost baseline at the legacy path (rollback-compat + re-own). A
    // rename over a hostile symlink/FIFO replaces the entry, never writes through it.
    atomic_write(&legacy, HOSTS_BASELINE, 0o644)
}

// ---- ADR-009 egress-pin delivery bridge ---------------------------------------------------------

/// The root-owned egress projection `store::project_pinned` writes (`<name> <ipv4>` per line, one level
/// under the hosts run dir). ADR-009 lifts a subset of these into the `/etc/hosts` composition.
fn egress_pinned_file(run: &Path) -> PathBuf {
    run.join("egress").join("pinned")
}

/// ADR-009: the blessed user-egress pins to lift into `/etc/hosts` so the uid-1000 DMS Go backend
/// resolves them via NSS `files` (never the dropped 127.0.0.53 DNS), TLS-name intact. Defensively read
/// the root-owned `{run}/egress/pinned` and keep only lines whose name is a SEALED-SOURCE, non-baseline,
/// `deliver hosts` catalog capability host ([`is_sealed_deliverable_host`], §4.4 layer 1 — the STRUCTURAL
/// owner-pin↔root-resolution isolation: an OWNER-manifest pin can NEVER enter the file root reads, by
/// construction) and whose addr is a strict IPv4 literal. A missing/hostile/garbage projection ⇒ empty,
/// so compose falls back to baseline + provider bindings exactly as before (`[R2MF1]`). First occurrence
/// of a name wins, mirroring [`read_bindings`].
fn read_egress_pins(run: &Path, catalog: &Catalog) -> Vec<(String, Ipv4Addr)> {
    let Some(body) = defensive_read(&egress_pinned_file(run)) else {
        return Vec::new();
    };
    let mut out: Vec<(String, Ipv4Addr)> = Vec::new();
    for line in body.lines() {
        let mut it = line.split_whitespace();
        let (Some(name), Some(addr_s), None) = (it.next(), it.next(), it.next()) else {
            continue; // not exactly `<name> <ipv4>` ⇒ skip
        };
        if !is_sealed_deliverable_host(catalog, name) {
            continue; // not a SEALED-source deliverable host ⇒ never lift (owner pins excluded, §4.4)
        }
        let Some(addr) = valid_bind_addr(addr_s) else {
            continue; // not a strict IPv4 literal ⇒ skip
        };
        if out.iter().any(|(n, _)| n == name) {
            continue; // first-wins
        }
        out.push((name.to_string(), addr));
    }
    out
}

// ---- compose ------------------------------------------------------------------------------------

/// Compose `/run/shrek/hosts` from the sealed baseline + every readable provider binding + the ADR-009
/// blessed-egress pins (SEALED-SOURCE deliverable hosts only, per `catalog`; §4.4), and install it
/// atomically (`root:root 0644` in a `0755` run dir). UNCONDITIONAL `[R2MF1]`: migration is best-effort
/// and can never block this; an empty/absent/hostile store just yields the baseline, so localhost always
/// resolves. `catalog` is the merged sealed+owner catalog ([`crate::catalog::load_catalog`]); an empty
/// catalog lifts NO egress pin (fail-closed — a variant with no manifests composes baseline+bindings
/// only). Returns the projection path. Caller holds [`lock_hosts`].
pub fn compose_hosts(home: &Path, run: &Path, catalog: &Catalog) -> io::Result<PathBuf> {
    // Migration is best-effort and MUST NOT block the projection.
    let _ = migrate_legacy(home);

    let mut body = String::from(HOSTS_BASELINE);
    for b in read_bindings(home) {
        // token → sealed host name, server-side. `read_bindings` already guaranteed the token maps.
        if let Some(host) = provider_host(&b.token) {
            body.push_str(&format!("{} {}\n", b.addr, host));
        }
    }

    // ADR-009 delivery bridge: lift blessed user-egress pins (weather's open-meteo hosts today) into
    // name resolution. The nft table still INDEPENDENTLY gates the packet — a name here is resolvable
    // but only reachable if its IP is pinned in the matching `@<profile>_pinned` set, so this removes
    // the DNS-drop deadlock without granting any egress. Sorted for a deterministic projection.
    let mut pins = read_egress_pins(run, catalog);
    pins.sort();
    for (name, addr) in pins {
        body.push_str(&format!("{addr} {name}\n"));
    }

    fs::create_dir_all(run)?;
    let _ = fs::set_permissions(run, fs::Permissions::from_mode(0o755));
    let _ = chown(run, Some(0), Some(0));
    let path = projection_file(run);
    atomic_write(&path, &body, 0o644)?;
    Ok(path)
}

// ---- lock ---------------------------------------------------------------------------------------

/// An exclusive advisory lock over the hosts store, released on drop. The base `compose-hosts` oneshot
/// and the daemon's `bind`/`unbind` both take it, so their store writes + projections never interleave
/// (`[N-2]`). Lock file `root:root 0600` in the `0700` home dir. `flock` blocks — the second writer waits.
pub struct HostsLock {
    _f: fs::File,
}

pub fn lock_hosts(home: &Path) -> io::Result<HostsLock> {
    fs::create_dir_all(home)?;
    let path = home.join(".hosts.lock");
    let f = fs::OpenOptions::new().create(true).write(true).open(&path)?;
    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    let _ = chown(&path, Some(0), Some(0));
    crate::uapi::flock(f.as_raw_fd(), crate::uapi::LOCK_EX)?;
    Ok(HostsLock { _f: f })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let base = std::env::var("CARGO_TARGET_TMPDIR").unwrap_or_else(|_| "/tmp".into());
        let d = PathBuf::from(base).join(format!(
            "hosts-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn bind_writes_a_token_line_and_projects_the_sealed_host() {
        let home = tmp();
        let run = tmp();
        write_binding(&home, "local", "192.168.1.152").unwrap();
        // store carries the TOKEN, not a resolvable name.
        let stored = fs::read_to_string(bindings_file(&home)).unwrap();
        assert_eq!(stored, "local 192.168.1.152\n");
        // projection maps token → sealed host name, with the baseline first.
        let p = compose_hosts(&home, &run, &Catalog::default()).unwrap();
        let proj = fs::read_to_string(&p).unwrap();
        assert!(proj.starts_with(HOSTS_BASELINE), "baseline must lead: {proj}");
        assert!(proj.contains("192.168.1.152 shrek-model\n"), "{proj}");
        assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o644);
        assert_eq!(fs::metadata(bindings_file(&home)).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn all_four_providers_map_to_their_sealed_hosts() {
        let home = tmp();
        let run = tmp();
        write_binding(&home, "local", "10.0.0.1").unwrap();
        write_binding(&home, "anthropic", "10.0.0.2").unwrap();
        write_binding(&home, "claude", "10.0.0.3").unwrap();
        write_binding(&home, "codex", "10.0.0.4").unwrap();
        let proj = fs::read_to_string(compose_hosts(&home, &run, &Catalog::default()).unwrap()).unwrap();
        assert!(proj.contains("10.0.0.1 shrek-model\n"));
        assert!(proj.contains("10.0.0.2 shrek-model-proxy\n"));
        assert!(proj.contains("10.0.0.3 shrek-claude-cli\n"));
        assert!(proj.contains("10.0.0.4 shrek-codex-cli\n"));
    }

    // ---- ADR-009 egress-pin delivery bridge ----

    /// Seed the root-owned egress projection exactly as `store::project_pinned` writes it.
    fn seed_egress_pinned(run: &Path, body: &str) {
        let egdir = run.join("egress");
        fs::create_dir_all(&egdir).unwrap();
        fs::write(egdir.join("pinned"), body).unwrap();
    }

    /// The catalog as the running box would load it — the sealed `weather` capability (deliver hosts).
    fn weather_catalog() -> Catalog {
        use shrek_policy::egress_capability::{build_catalog, parse_manifest, WEATHER_MANIFEST};
        build_catalog(vec![parse_manifest(WEATHER_MANIFEST).unwrap()], vec![])
    }

    /// An OWNER capability whose host, even with `deliver hosts` attempted + a live pin, must NEVER be
    /// lifted into `/etc/hosts` (§4.4 layer 1 STRUCTURAL isolation — owner source is never deliverable).
    fn catalog_with_owner_deliverable() -> Catalog {
        use shrek_policy::egress_capability::{build_catalog, parse_manifest, WEATHER_MANIFEST};
        let owner = parse_manifest(
            "schema shrek-egress-capability/1\n\
             name radar\ntitle Radar\npurpose P\nfeature owner:radar\n\
             tier one-click\ndeliver hosts\nhost radar.example.com tcp 443\n",
        )
        .unwrap();
        build_catalog(vec![parse_manifest(WEATHER_MANIFEST).unwrap()], vec![owner])
    }

    #[test]
    fn adr009_lifts_blessed_weather_pins_into_the_projection() {
        let home = tmp();
        let run = tmp();
        seed_egress_pinned(&run, "api.open-meteo.com 5.6.7.8\ngeocoding-api.open-meteo.com 5.6.7.9\n");
        let proj = fs::read_to_string(compose_hosts(&home, &run, &weather_catalog()).unwrap()).unwrap();
        assert!(proj.starts_with(HOSTS_BASELINE), "baseline must lead: {proj}");
        assert!(proj.contains("5.6.7.8 api.open-meteo.com\n"), "forecast host lifted: {proj}");
        assert!(proj.contains("5.6.7.9 geocoding-api.open-meteo.com\n"), "geocoding host lifted: {proj}");
    }

    #[test]
    fn adr009_never_lifts_off_profile_baseline_or_owner_host() {
        let home = tmp();
        let run = tmp();
        // Re-validate defensively against the CATALOG: a foreign name (poisoned/legacy), a BASELINE host
        // (desktop-updates), AND an OWNER capability's host (even one declaring `deliver hosts`) must NEVER
        // enter host-wide name resolution — only the SEALED weather hosts are lifted (§4.4).
        seed_egress_pinned(
            &run,
            "api.open-meteo.com 5.6.7.8\nevil.example.com 6.6.6.6\nshrekos-updates.iambu.dev 9.9.9.9\nradar.example.com 7.7.7.7\n",
        );
        let proj = fs::read_to_string(compose_hosts(&home, &run, &catalog_with_owner_deliverable()).unwrap()).unwrap();
        assert!(proj.contains("5.6.7.8 api.open-meteo.com\n"), "sealed weather host lifted: {proj}");
        assert!(!proj.contains("6.6.6.6"), "off-catalog host must NOT be lifted: {proj}");
        assert!(!proj.contains("evil.example.com"), "off-catalog host must NOT be lifted: {proj}");
        assert!(!proj.contains("shrekos-updates.iambu.dev"), "baseline host must NOT be lifted: {proj}");
        assert!(!proj.contains("7.7.7.7"), "OWNER pin must NOT be lifted (§4.4 structural): {proj}");
        assert!(!proj.contains("radar.example.com"), "OWNER host must NOT be lifted (§4.4 structural): {proj}");
    }

    #[test]
    fn adr009_absent_pinned_yields_baseline_plus_bindings_only() {
        let home = tmp();
        let run = tmp();
        write_binding(&home, "local", "10.0.0.1").unwrap();
        // no {run}/egress/pinned exists at all
        let proj = fs::read_to_string(compose_hosts(&home, &run, &weather_catalog()).unwrap()).unwrap();
        assert!(proj.starts_with(HOSTS_BASELINE));
        assert!(proj.contains("10.0.0.1 shrek-model\n"));
        assert!(!proj.contains("open-meteo"), "no egress pins ⇒ none lifted: {proj}");
    }

    #[test]
    fn bind_replaces_and_unbind_removes_idempotently() {
        let home = tmp();
        write_binding(&home, "local", "1.1.1.1").unwrap();
        write_binding(&home, "local", "2.2.2.2").unwrap(); // rebind replaces
        assert_eq!(read_bindings(&home), vec![Binding { token: "local".into(), addr: Ipv4Addr::new(2, 2, 2, 2) }]);
        remove_binding(&home, "local").unwrap();
        assert!(read_bindings(&home).is_empty());
        remove_binding(&home, "local").unwrap(); // idempotent: unbind of an unbound provider is OK
    }

    #[test]
    fn bind_canonicalizes_and_rejects_non_ipv4() {
        let home = tmp();
        // a non-IPv4 / hostname / hex address is refused (never reaches the store).
        for bad in ["myhost.lan", "0x7f000001", "127.1", "::1", "256.0.0.1", "1.2.3.4:8100"] {
            assert!(write_binding(&home, "local", bad).is_err(), "{bad} must be refused");
        }
        assert!(read_bindings(&home).is_empty());
        // a non-provider token is refused even with a good address.
        assert!(write_binding(&home, "swamp", "1.2.3.4").is_err());
        assert!(write_binding(&home, "github", "1.2.3.4").is_err());
    }

    #[test]
    fn read_bindings_is_fail_closed_per_line() {
        let home = tmp();
        // hand-write a hostile store: good line, a non-provider token, a hostname addr, a smuggled 3rd
        // field, a public host, a duplicate (first wins).
        fs::write(
            bindings_file(&home),
            "local 192.168.1.5\nswamp 6.6.6.6\nanthropic myhost.lan\ncodex 1.2.3.4 extra\nlocal 9.9.9.9\nntp 10.0.0.9\n",
        )
        .unwrap();
        let got = read_bindings(&home);
        // only the FIRST valid `local` survives; every other line is dropped.
        assert_eq!(got, vec![Binding { token: "local".into(), addr: Ipv4Addr::new(192, 168, 1, 5) }]);
    }

    #[test]
    fn projection_is_baseline_only_when_no_store() {
        let home = tmp();
        let run = tmp();
        // No bindings file at all (fresh box) — projection is still installed with localhost [R2MF1].
        let p = compose_hosts(&home, &run, &Catalog::default()).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), HOSTS_BASELINE);
    }

    #[test]
    fn compose_ignores_a_symlink_store_and_still_installs_localhost() {
        let home = tmp();
        let run = tmp();
        // A pre-fix uid-1000 plants `hosts-bindings` as a symlink to a secret. defensive_read refuses to
        // follow it; the projection is baseline-only, and the write NEVER goes through the symlink.
        let secret = tmp().join("secret");
        fs::write(&secret, "codex 6.6.6.6\n").unwrap();
        std::os::unix::fs::symlink(&secret, bindings_file(&home)).unwrap();
        let p = compose_hosts(&home, &run, &Catalog::default()).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), HOSTS_BASELINE, "symlinked store must be ignored");
        // the secret was not followed for a bind either.
        assert!(write_binding(&home, "local", "1.2.3.4").is_ok());
        // and the symlink was replaced by a real root file (rename-over, no write-through).
        assert!(fs::symlink_metadata(bindings_file(&home)).unwrap().file_type().is_file());
        assert_eq!(fs::read_to_string(&secret).unwrap(), "codex 6.6.6.6\n", "secret untouched");
    }

    #[test]
    fn migration_absorbs_only_model_lines_and_strips_the_rest() {
        let home = tmp();
        let run = tmp();
        // A pre-fix legacy hosts file: localhost, the 4 model lines, an ATTACKER line poisoning a public
        // host and NTP, and a duplicate model line (first wins).
        fs::write(
            legacy_hosts_file(&home),
            "127.0.0.1 localhost\n\
             192.168.1.10 shrek-model\n\
             10.0.0.2 shrek-model-proxy\n\
             6.6.6.6 github.com\n\
             6.6.6.6 time.cloudflare.com\n\
             7.7.7.7 shrek-model\n\
             10.0.0.3 shrek-claude-cli\n\
             10.0.0.4 shrek-codex-cli\n\
             6.6.6.6 shrek-swamp-broker\n",
        )
        .unwrap();
        let p = compose_hosts(&home, &run, &Catalog::default()).unwrap();
        let proj = fs::read_to_string(&p).unwrap();
        // The 4 model bindings migrated (first `shrek-model` = 192.168.1.10, NOT 7.7.7.7).
        assert!(proj.contains("192.168.1.10 shrek-model\n"), "{proj}");
        assert!(!proj.contains("7.7.7.7"), "duplicate must lose to first: {proj}");
        assert!(proj.contains("10.0.0.2 shrek-model-proxy\n"));
        assert!(proj.contains("10.0.0.3 shrek-claude-cli\n"));
        assert!(proj.contains("10.0.0.4 shrek-codex-cli\n"));
        // The attacker lines (github, NTP, swamp-broker) are STRIPPED — never in the projection.
        assert!(!proj.contains("6.6.6.6"), "attacker lines must be stripped: {proj}");
        assert!(!proj.contains("github.com"));
        assert!(!proj.contains("time.cloudflare.com"));
        assert!(!proj.contains("shrek-swamp-broker"), "swamp-broker is not owner-bindable");
        // The legacy path is now a root-owned localhost baseline (rollback-compat + re-owned).
        let legacy = fs::read_to_string(legacy_hosts_file(&home)).unwrap();
        assert_eq!(legacy, HOSTS_BASELINE);
        assert_eq!(fs::metadata(legacy_hosts_file(&home)).unwrap().permissions().mode() & 0o777, 0o644);
    }

    #[test]
    fn defensive_read_rejects_a_non_regular_legacy() {
        // A non-regular file at the legacy/store path (here a directory; a FIFO — the boot-hang vector —
        // takes the IDENTICAL `!is_file()` branch) is ignored, so compose still installs localhost and
        // never blocks. defensive_read returns None for any non-regular type up front.
        let home = tmp();
        let run = tmp();
        fs::create_dir_all(legacy_hosts_file(&home)).unwrap(); // legacy path IS a directory
        assert!(defensive_read(&legacy_hosts_file(&home)).is_none());
        // compose still succeeds with the baseline (migration's best-effort failure never blocks it).
        let p = compose_hosts(&home, &run, &Catalog::default()).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), HOSTS_BASELINE);
        // a planted directory at the STORE path is likewise ignored on read (bindings come back empty).
        let home2 = tmp();
        fs::create_dir_all(bindings_file(&home2)).unwrap();
        assert!(read_bindings(&home2).is_empty());
    }
}
