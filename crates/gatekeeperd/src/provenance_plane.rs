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
use crate::pin_manifest::{Closure, PinManifest, PinMatch};
use shrek_policy::{derive_band, Evidence, TrustBand};
use std::ffi::CString;
use std::io;
use std::os::fd::AsRawFd;
use std::os::fd::OwnedFd;
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

/// The sealed static pin-manifest (slice-8, amendment B): a versioned file under the dm-verity `/usr`
/// root, so it carries §4-static custody (change it ⇒ signed image update). There is deliberately NO
/// runtime path override — an env/flag-selectable manifest would be the ADV-8 writable-label attack
/// renamed (point the broker at attacker-authored "policy"). The shipped image ships this file EMPTY
/// (header only) or absent, so nothing earns `T-pinned` until a real pin lands.
const PIN_MANIFEST_PATH: &str = "/usr/lib/shrek/pin-manifest";

/// The outcome of measuring an entrypoint: the derived band plus the raw facts, for the audit line.
pub struct Derivation {
    pub band: TrustBand,
    pub evidence: Evidence,
    pub sealed_root: Option<(u32, u32)>,
    pub entrypoint: Option<String>,
    /// The measured entrypoint fd, BOUND to a `T-pinned` derivation (slice-8, amendment A). It is the
    /// single fd the fs-verity digest was measured on, retained so the classification is provably tied
    /// to THAT object — not re-resolved by path. `None` for every other band.
    ///
    /// This slice is CLASSIFICATION-ONLY: a `T-pinned` workload is NOT runnable (a pinned artifact on a
    /// writable grant has no executable home — grants are `MS_NOEXEC` + Landlock read-only, and this
    /// slice does not reopen that posture). `sandbox` therefore REFUSES a `T-pinned` construction
    /// deterministically (`pinned-exec-home-unavailable`); this fd is never executed. It exists to
    /// demonstrate the pathname-independent binding for the separately-reviewed exec-home slice.
    pub exec_fd: Option<OwnedFd>,
    /// slice-10: `Some` when the matched entrypoint is a **sealed-dynamic closure** (a dynamically-
    /// linked pin whose interpreter + transitive `DT_NEEDED` are all identity-pinned in the manifest).
    /// Carries the closure spec (member paths + digests) forward so the constructor builds the N-inode
    /// closure island; `None` for a slice-8/9 single-inode static pin. Only ever set together with
    /// `exec_fd` (the entrypoint fd) on a `T-pinned` band.
    pub closure: Option<Closure>,
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

/// Load + parse the sealed pin-manifest. `None` on absence (the shipped default ⇒ no pins) OR on ANY
/// parse error (fail-high: one bad line poisons the whole manifest rather than shipping an optimistic
/// subset — docs slice-8 §4). A rejected manifest is logged so a malformed sealed policy is diagnosable.
fn load_manifest() -> Option<PinManifest> {
    let text = std::fs::read_to_string(PIN_MANIFEST_PATH).ok()?;
    match PinManifest::parse(&text) {
        Ok(m) => Some(m),
        Err(e) => {
            eprintln!("gatekeeperd/pin: MANIFEST REJECTED (fail-high, no pins): {e}");
            None
        }
    }
}

/// Open the entrypoint `O_RDONLY` (needed for the verity ioctl AND to carry to `execveat`), TOCTOU-safe
/// with the same resolve flags as the sealed measurement. This is the single fd that is both measured
/// and executed for a pin (amendment A) — not an `O_PATH` fd (ioctls are rejected on `O_PATH`).
fn open_entry_rdonly(p: &Path) -> io::Result<OwnedFd> {
    let how = OpenHow {
        flags: O_RDONLY | O_CLOEXEC,
        mode: 0,
        resolve: RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS,
    };
    openat2(AT_FDCWD as RawFd, path_cstr(p)?.as_c_str(), &how)
}

/// Measure the fd's fs-verity `(algorithm, digest)` and look the tuple up in the sealed manifest.
/// Returns `(pinned_digest_match, closed_world)`. Any measurement failure (`ENODATA` = no verity on
/// the file, unknown algorithm/size) or a manifest miss ⇒ `(false, false)` ⇒ the pin arm contributes
/// nothing and the band falls high. Split out so the oracle can drive it with an in-memory manifest
/// and a real verity fd, exercising the production path without a manifest-path override.
pub fn pin_lookup_fd(fd: RawFd, manifest: &PinManifest) -> (bool, bool) {
    let (algo, digest) = match measure_verity(fd) {
        Ok(v) => v,
        Err(_) => return (false, false),
    };
    match manifest.lookup(algo, &digest) {
        Some(class) => (true, class.is_closed_world()),
        None => (false, false),
    }
}

/// The pin arm: for an absolute entrypoint, load the sealed manifest and, if it has pins, open+measure
/// the entrypoint and look it up. Returns `(matched, closed_world, fd, closure)` where `fd` is `Some`
/// (the measured==executed fd) only on a match, and `closure` is `Some` only when the match is a
/// sealed-dynamic closure entry (slice-10). No manifest / empty manifest / non-absolute path / open
/// failure / miss ⇒ `(false, false, None, None)`.
fn pin_arm(entrypoint: &Path) -> (bool, bool, Option<OwnedFd>, Option<Closure>) {
    if !entrypoint.is_absolute() {
        return (false, false, None, None);
    }
    let Some(manifest) = load_manifest() else {
        return (false, false, None, None);
    };
    if manifest.is_empty() {
        return (false, false, None, None);
    }
    let Ok(fd) = open_entry_rdonly(entrypoint) else {
        return (false, false, None, None);
    };
    let (algo, digest) = match measure_verity(fd.as_raw_fd()) {
        Ok(v) => v,
        Err(_) => return (false, false, None, None),
    };
    match manifest.lookup_match(algo, &digest) {
        Some(PinMatch::Static(class)) => (true, class.is_closed_world(), Some(fd), None),
        Some(PinMatch::Closure(c)) => (true, c.class.is_closed_world(), Some(fd), Some(c.clone())),
        None => (false, false, None, None),
    }
}

/// SPIKE-ONLY fixture helper (stripped before ship, with the gate scaffolding): `pin-verity enable
/// <path>` turns on fs-verity for a fixture file; `pin-verity measure <path>` prints `<algo> <hex>` so
/// the oracle / VM gate can write the fixture into the sealed pin-manifest. NOT a production verb — the
/// shipped image has no writable verity fixtures and the real manifest is baked under dm-verity `/usr`.
pub fn pin_verity_cli(args: &[String]) -> i32 {
    let (Some(op), Some(path)) = (args.first().map(String::as_str), args.get(1)) else {
        eprintln!("usage: gatekeeperd pin-verity <enable|measure> <path>");
        return 2;
    };
    let cpath = match path_cstr(Path::new(path)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("pin-verity: bad path {path}: {e}");
            return 2;
        }
    };
    let how = OpenHow { flags: O_RDONLY | O_CLOEXEC, mode: 0, resolve: 0 };
    let fd = match openat2(AT_FDCWD as RawFd, cpath.as_c_str(), &how) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("pin-verity: open {path}: {e}");
            return 1;
        }
    };
    match op {
        "enable" => match enable_verity(fd.as_raw_fd()) {
            Ok(()) => {
                println!("verity-enabled {path}");
                0
            }
            Err(e) => {
                eprintln!("pin-verity: enable {path}: {e}");
                1
            }
        },
        "measure" => match measure_verity(fd.as_raw_fd()) {
            Ok((algo, digest)) => {
                let name = match algo {
                    1 => "sha256",
                    2 => "sha512",
                    _ => "unknown",
                };
                let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
                println!("{name} {hex}");
                0
            }
            Err(e) => {
                eprintln!("pin-verity: measure {path}: {e}");
                1
            }
        },
        other => {
            eprintln!("pin-verity: unknown op {other}");
            2
        }
    }
}

/// Measure the entrypoint and derive its band. `sealed` is the once-computed sealed-root device.
pub fn derive(entrypoint: Option<&String>, sealed: Option<(u32, u32)>) -> Derivation {
    let ep = entrypoint.map(|s| s.as_str());
    let mut evidence = measure(ep, sealed);

    // Pin arm (slice-8): a digest match against the sealed manifest earns `T-pinned` when the
    // entrypoint is NOT on the sealed root. The matched manifest entry's class supplies
    // `domain_execution_sealed` for the pinned object — the compiled-in `CLOSED_WORLD` path list only
    // covers sealed-root programs, so a writable-mount pin needs the manifest as the domain authority.
    // A sealed-root entrypoint is already `T-first` (strongest-first in `derive_band`), so the pin arm
    // only ever *adds* a weaker positive proof; it can never lower a band.
    let mut exec_fd = None;
    let mut closure = None;
    if let Some(p) = ep.map(Path::new) {
        let (matched, closed, fd, clo) = pin_arm(p);
        if matched {
            evidence.pinned_digest_match = true;
            evidence.domain_execution_sealed = evidence.domain_execution_sealed || closed;
            exec_fd = fd;
            closure = clo;
        }
    }

    let band = derive_band(&evidence);
    // Carry the measured fd + closure to exec ONLY for a pin-derived band (amendment A). A non-pin band
    // drops both — a sealed-dynamic closure is only ever acted on for a `T-pinned` derivation.
    let (exec_fd, closure) = if band == TrustBand::Pinned { (exec_fd, closure) } else { (None, None) };
    Derivation {
        band,
        evidence,
        sealed_root: sealed,
        entrypoint: entrypoint.cloned(),
        exec_fd,
        closure,
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
    fn plain_file_has_no_verity_so_no_pin() {
        // A real fd to a NON-fs-verity file: the measurement ioctl errors (ENODATA/ENOTTY), so the pin
        // arm yields (false, false) — fail high. Exercises the actual `measure_verity` syscall + the
        // lookup wiring on a live fd without needing a verity filesystem (that end-to-end is the VM
        // gate). Guards against a measurement error being mistaken for a match.
        use std::io::Write;
        let raw = std::env::temp_dir().join(format!("shrek-pin-noverity-{}", std::process::id()));
        {
            let mut f = std::fs::File::create(&raw).unwrap();
            f.write_all(b"not a pinned artifact").unwrap();
        }
        // Canonicalise so RESOLVE_NO_SYMLINKS (in open_entry_rdonly) can't trip on a symlinked tmpdir.
        let path = std::fs::canonicalize(&raw).unwrap();
        let fd = open_entry_rdonly(&path).unwrap();
        assert!(measure_verity(fd.as_raw_fd()).is_err(), "a non-verity file must not measure");
        let manifest = crate::pin_manifest::PinManifest::parse("shrek-pin-manifest v1\n").unwrap();
        assert_eq!(pin_lookup_fd(fd.as_raw_fd(), &manifest), (false, false));
        let _ = std::fs::remove_file(&raw);
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
