//! provenance_plane — B1 (slice-7): gatekeeperd MEASURES the workload entrypoint and DERIVES its
//! trust band, so the band is never a caller assertion. This is the last caller-asserted input to the
//! tier decision; after this slice the workload no longer tells Shrek how trustworthy it is.
//!
//! Design of record: `docs/phase5-slice7-trust-provenance.md`. The pure lattice is
//! `shrek_policy::derive_band`; this plane does the I/O the lattice must not — pinning + measuring the
//! entrypoint (the same TOCTOU-safe `openat2`+`statx` machinery as `mount_plane`) and confirming the
//! sealed dm-verity root — and hands the measured facts in as `Evidence`.
//!
//! MVP scope (locked): the only band this arm can POSITIVELY earn is `T-first`, and only for a
//! closed-world sealed entrypoint. Everything else fails high to `T-hostile` (the deferred pin /
//! provenance-log stores earn `T-pinned`/`T-untrust` later). Fail-high is total: any measurement
//! error, a non-absolute or unresolvable entrypoint, an unsealed root, or an open-world profile all
//! resolve `T-hostile` — the broker never distinguishes "could not measure" from "measured hostile".

use crate::linux_uapi::*;
use shrek_policy::{derive_band, Evidence, TrustBand};
use std::ffi::CString;
use std::io;
use std::os::fd::AsRawFd;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// The canonical always-sealed anchor: `/usr` lives on the dm-verity root partition (the whole root
/// is sealed — `image/mkosi.repart/10-root.conf` `Verity=data`, roothash in the signed UKI). Its
/// `st_dev` IS the verity device; any entrypoint on the same device is on the sealed root.
const SEALED_ANCHOR: &str = "/usr";

/// The compiled-in sealed EXECUTION PROFILE (docs §5.1): the closed-world sealed programs — fixed
/// sealed binaries that do NOT read-and-execute external / mutable / interpreted / generated code.
/// Anything not listed is treated OPEN-WORLD and fails high. This is sealed policy (compiled into the
/// dm-verity `/usr` binary), never caller-supplied — there is deliberately NO env/flag override, so a
/// caller can never enrol its own code (no production trust-override hatch).
///
/// The MVP enrollee is the sealed acceptance probe `/usr/libexec/shrek/gate-probe` (docs §8.1): a
/// genuine closed-world sealed program (execs nothing, no interpreted/generated code, args are data
/// only) resident on the dm-verity root, so it legitimately derives `T-first` and drives the
/// production-shaped T0/T1 VM gates. A shell CANNOT be enrolled — B1 treats interpreters as
/// open-world. First-party closed-world tools are enrolled here the same way as they are built.
/// Spike-only for now (stripped before ship with the probe binary): a SHIPPED image starts with no
/// enrollee and thus nothing `T-first` until real first-party tools land — the correct fail-safe.
/// Paths are CANONICAL (measurement uses `RESOLVE_NO_SYMLINKS`, so a symlinked form would not match).
const CLOSED_WORLD: &[&str] = &["/usr/libexec/shrek/gate-probe"];

/// The outcome of measuring an entrypoint: the derived band plus the raw facts, for the audit line.
pub struct Derivation {
    pub band: TrustBand,
    pub evidence: Evidence,
    pub sealed_root: Option<(u32, u32)>,
    pub entrypoint: Option<String>,
}

fn path_cstr(p: &Path) -> io::Result<CString> {
    CString::new(p.as_os_str().as_bytes()).map_err(|_| io::Error::from_raw_os_error(22))
}

/// statx a path via an `openat2` `O_PATH` open with the given resolve flags (TOCTOU-safe when
/// `RESOLVE_NO_SYMLINKS` is passed). Returns the kernel `statx` — dev fields are always populated;
/// `statx_fd` requests `STATX_TYPE` so `stx_mode` is valid too.
fn statx_of(p: &Path, resolve: u64) -> io::Result<Statx> {
    let how = OpenHow { flags: O_PATH | O_CLOEXEC, mode: 0, resolve };
    let fd = openat2(AT_FDCWD as RawFd, path_cstr(p)?.as_c_str(), &how)?;
    statx_fd(fd.as_raw_fd())
}

/// Confirm the block device `(maj,min)` is read-only AND a dm-verity target, via sysfs. A dm-verity
/// device carries a DM UUID that contains `verity` (libcryptsetup/veritysetup — systemd's root-verity
/// path — writes `CRYPT-VERITY-…`; the match is case-insensitive on the `verity` token to tolerate
/// naming variation across the setup path). Requires the block device to be read-only too. A dev host
/// with a writable, non-dm root returns false ⇒ nothing is sealed there and derivation fails high.
/// (Validated empirically by the sealed VM gate — docs §10; the `SANDBOX-PROVENANCE` line prints
/// `sealed_root=Some/None` so a detection miss is diagnosable from the serial console.)
fn is_verity_ro(maj: u32, min: u32) -> bool {
    let base = format!("/sys/dev/block/{maj}:{min}");
    let ro = std::fs::read_to_string(format!("{base}/ro"))
        .map(|s| s.trim() == "1")
        .unwrap_or(false);
    // A dm device exposes dm/uuid (and dm/name); a verity target's uuid contains the `verity` token.
    let uuid = std::fs::read_to_string(format!("{base}/dm/uuid")).unwrap_or_default();
    let name = std::fs::read_to_string(format!("{base}/dm/name")).unwrap_or_default();
    let is_verity = uuid.to_ascii_lowercase().contains("verity")
        || name.to_ascii_lowercase().contains("verity");
    ro && is_verity
}

/// Identify the sealed dm-verity ROOT device, or `None` when the root is not a confirmed sealed
/// verity device (then NOTHING can be `entrypoint_sealed` and every derivation fails high). There is
/// deliberately NO override: on a host/container with no real dm-verity this returns `None` and every
/// derivation is `T-hostile` — which is exactly what the host oracle asserts (the positive `T-first`
/// arm is proven only in the sealed VM, where verity is real). No production trust-override hatch.
pub fn sealed_root_dev() -> Option<(u32, u32)> {
    let st = statx_of(Path::new(SEALED_ANCHOR), RESOLVE_NO_MAGICLINKS).ok()?;
    let (maj, min) = (st.stx_dev_major, st.stx_dev_minor);
    if is_verity_ro(maj, min) {
        Some((maj, min))
    } else {
        None
    }
}

/// True if the entrypoint's canonical sealed path is a closed-world sealed program (docs §5.1),
/// looked up in the compiled-in sealed [`CLOSED_WORLD`] list only — no env/flag override.
fn exec_class_closed_world(p: &Path) -> bool {
    CLOSED_WORLD.iter().any(|s| Path::new(s) == p)
}

/// Build the measured `Evidence` for a workload entrypoint. Any doubt ⇒ `Evidence::UNVERIFIABLE`.
fn measure(entrypoint: Option<&str>, sealed: Option<(u32, u32)>) -> Evidence {
    // Root not a confirmed sealed verity device ⇒ nothing is first-party. Fail high.
    let Some(dev) = sealed else { return Evidence::UNVERIFIABLE };
    let Some(ep) = entrypoint else { return Evidence::UNVERIFIABLE };
    let p = Path::new(ep);
    // Require an absolute path: a sealed program is named by its sealed location, not resolved through
    // a mutable CWD/PATH. Non-absolute ⇒ fail high.
    if !p.is_absolute() {
        return Evidence::UNVERIFIABLE;
    }
    // TOCTOU-safe measurement: no symlink or magic-link may redirect what we measure vs what runs.
    let st = match statx_of(p, RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS) {
        Ok(s) => s,
        Err(_) => return Evidence::UNVERIFIABLE,
    };
    // entrypoint_sealed: a REGULAR file resident on the confirmed sealed device (st_dev match).
    let entrypoint_sealed =
        (st.stx_dev_major, st.stx_dev_minor) == dev && (st.stx_mode as u32 & S_IFMT) == S_IFREG;
    // domain_execution_sealed: a SEPARATE fact — the sealed execution profile is closed-world. st_dev
    // proves provenance only; an interpreter/JIT/plugin host on the sealed root would still be
    // open-world and must NOT earn T-first (docs §5.1, the no-laundering rule).
    let domain_execution_sealed = exec_class_closed_world(p);
    Evidence::mvp(entrypoint_sealed, domain_execution_sealed)
}

/// Measure the entrypoint and derive its band. `sealed` is the once-computed sealed-root device.
pub fn derive(entrypoint: Option<&String>, sealed: Option<(u32, u32)>) -> Derivation {
    let ep = entrypoint.map(|s| s.as_str());
    let evidence = measure(ep, sealed);
    Derivation {
        band: derive_band(&evidence),
        evidence,
        sealed_root: sealed,
        entrypoint: entrypoint.cloned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_sealed_root_fails_high() {
        // A host with no confirmed verity root: even an absolute entrypoint derives T-hostile.
        let d = derive(Some(&"/usr/bin/true".to_string()), None);
        assert_eq!(d.band, TrustBand::Hostile);
        assert!(!d.evidence.entrypoint_sealed);
    }

    #[test]
    fn no_entrypoint_fails_high() {
        let d = derive(None, Some((254, 0)));
        assert_eq!(d.band, TrustBand::Hostile);
    }

    #[test]
    fn relative_entrypoint_fails_high() {
        // Non-absolute entrypoint is never trusted (resolved through a mutable CWD/PATH).
        let e = measure(Some("bin/tool"), Some((254, 0)));
        assert_eq!(derive_band(&e), TrustBand::Hostile);
        assert!(!e.entrypoint_sealed);
    }

    #[test]
    fn closed_world_lookup_is_exact_and_canonical() {
        assert!(exec_class_closed_world(Path::new("/usr/libexec/shrek/gate-probe")));
        // a shell/interpreter is NEVER enrolled (open-world), and a non-canonical form must not match.
        assert!(!exec_class_closed_world(Path::new("/bin/sh")));
        assert!(!exec_class_closed_world(Path::new("/usr/bin/bash")));
        assert!(!exec_class_closed_world(Path::new("/usr/libexec/shrek/../shrek/gate-probe")));
    }
}
