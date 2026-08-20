//! mount_plane — pin → verify → relocate.
//!
//! Materialize a filesystem grant into a plain, broker-owned path that `systemd-nspawn` can bind,
//! defeating the CVE-2019-5736 / TOCTOU class where a symlink or rename between the policy check and
//! the mount redirects the bind to an ungranted target. M0 (docs/phase5-slice1-mount.md) confirmed
//! nspawn re-resolves its `--bind=` string in its own process and will not honor a pinned fd, so the
//! broker must resolve-and-pin, then hand nspawn a controlled plain path.
//!
//! The recipe:
//!   1. `openat2` the grant beneath a trusted anchor with RESOLVE_BENEATH | NO_SYMLINKS |
//!      NO_MAGICLINKS → an `O_PATH` fd bound to one inode, immune to any later path swap.
//!   2. `statx` the fd → record (dev, ino).
//!   3. in a private mount namespace, bind `/proc/self/fd/N` onto the plain target.
//!   4. re-verify the target's (dev, ino) equals the pin — else unmount and fail closed.
//!   5. drop the mount to read-only + nosuid/nodev/noexec.
//! nspawn is then pointed at the plain target — nothing left to race.

use crate::linux_uapi::*;
use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

const EINVAL: i32 = 22;

/// Filesystem object identity: (dev_major, dev_minor, ino). Recorded at pin time, re-checked after
/// the relocate bind. Any drift means the object under the target changed — fail closed.
#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub struct Ident {
    pub dev_major: u32,
    pub dev_minor: u32,
    pub ino: u64,
}

/// A grant pinned to a single inode via an `O_PATH` fd. The fd — not any path string — is the
/// authority: once held, no rename/symlink swap of the source path can redirect it.
#[derive(Debug)]
pub struct Pinned {
    pub fd: OwnedFd,
    pub ident: Ident,
    pub is_dir: bool,
}

fn path_cstr(p: &Path) -> io::Result<CString> {
    CString::new(p.as_os_str().as_bytes()).map_err(|_| io::Error::from_raw_os_error(EINVAL))
}

fn ident_of(st: &Statx) -> io::Result<Ident> {
    if st.stx_mask & STATX_INO == 0 {
        return Err(io::Error::new(io::ErrorKind::Other, "statx did not return an inode"));
    }
    Ok(Ident {
        dev_major: st.stx_dev_major,
        dev_minor: st.stx_dev_minor,
        ino: st.stx_ino,
    })
}

/// Open a trusted anchor directory (e.g. `/srv`) as an `O_PATH` dir fd. The anchor lives on a
/// non-attacker-writable parent (security-model mount-TOCTOU invariant); grants are resolved
/// strictly *beneath* it.
pub fn open_anchor(dir: &Path) -> io::Result<OwnedFd> {
    let how = OpenHow {
        flags: O_PATH | O_CLOEXEC | O_DIRECTORY,
        resolve: RESOLVE_NO_MAGICLINKS,
        ..Default::default()
    };
    openat2(AT_FDCWD as RawFd, &path_cstr(dir)?, &how)
}

/// Pin a grant `name` beneath `anchor`, refusing any symlink, magic-link, or parent escape in one
/// atomic `openat2`. Records (dev, ino). There is no softened retry: a rejected resolve is a hard
/// error the caller must propagate (fail closed).
pub fn pin_beneath(anchor: &OwnedFd, name: &str) -> io::Result<Pinned> {
    let cname = CString::new(name).map_err(|_| io::Error::from_raw_os_error(EINVAL))?;
    let how = OpenHow {
        flags: O_PATH | O_CLOEXEC,
        resolve: RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS,
        ..Default::default()
    };
    let fd = openat2(anchor.as_raw_fd(), &cname, &how)?;
    let st = statx_fd(fd.as_raw_fd())?;
    let ident = ident_of(&st)?;
    let is_dir = (st.stx_mode as u32 & S_IFMT) == S_IFDIR;
    Ok(Pinned { fd, ident, is_dir })
}

/// statx an absolute (broker-controlled) path and return its identity.
fn ident_at_path(p: &Path) -> io::Result<Ident> {
    let how = OpenHow {
        flags: O_PATH | O_CLOEXEC,
        resolve: 0,
        ..Default::default()
    };
    let fd = openat2(AT_FDCWD as RawFd, &path_cstr(p)?, &how)?;
    ident_of(&statx_fd(fd.as_raw_fd())?)
}

/// Enter a private mount namespace and make the mount tree recursively private, so every relocate
/// bind is contained and never propagates to the parent/host mount table. Call once in the
/// per-request child before any `relocate_ro`.
pub fn enter_private_mount_ns() -> io::Result<()> {
    unshare(CLONE_NEWNS)?;
    mount(c"none", c"/", None, MS_REC | MS_PRIVATE, None)
}

/// Read-only bind of a plain source directory onto a target (used to stage the sandbox's base OS
/// runtime, e.g. `/usr`, into the synthetic root). Unlike a grant this is not attacker-influenced,
/// so it needs no fd-pin — but it is still hardened to ro/nosuid/nodev.
pub fn bind_ro(src: &Path, target: &Path) -> io::Result<()> {
    std::fs::create_dir_all(target)?;
    let s = path_cstr(src)?;
    let t = path_cstr(target)?;
    mount(&s, &t, None, MS_BIND | MS_REC, None)?;
    mount(&s, &t, None, MS_BIND | MS_REC | MS_REMOUNT | MS_RDONLY | MS_NOSUID | MS_NODEV, None)
}

/// Bind the pinned fd onto a plain, broker-owned `target`, re-verify the destination is exactly the
/// pinned inode, then drop the mount to read-only + nosuid/nodev/noexec. On identity drift the bind
/// is torn down and an error returned — the sandbox build must not proceed.
pub fn relocate_ro(p: &Pinned, target: &Path) -> io::Result<()> {
    std::fs::create_dir_all(target)?;
    let src = CString::new(format!("/proc/self/fd/{}", p.fd.as_raw_fd()))
        .map_err(|_| io::Error::from_raw_os_error(EINVAL))?;
    let tgt = path_cstr(target)?;

    // Bind FROM the pinned fd — the source inode is fixed, immune to a path swap.
    mount(&src, &tgt, None, MS_BIND, None)?;

    // Re-verify: whatever is now at target must be the inode we pinned.
    match ident_at_path(target) {
        Ok(id) if id == p.ident => {}
        Ok(_) => {
            let _ = umount2(&tgt, 0);
            return Err(io::Error::new(io::ErrorKind::Other, "identity drift after relocate bind"));
        }
        Err(e) => {
            let _ = umount2(&tgt, 0);
            return Err(e);
        }
    }

    // Read-only hardening. Flags take effect only via a bind-remount; source/data are ignored.
    mount(
        &src,
        &tgt,
        None,
        MS_BIND | MS_REMOUNT | MS_RDONLY | MS_NOSUID | MS_NODEV | MS_NOEXEC,
        None,
    )
}

/// slice-9 — bind a pinned inode over its OWN existing anchor path and harden it
/// `RO|NOSUID|NODEV|NOEXEC`, contained to the caller's private mount ns. Unlike `relocate_ro` this
/// does NOT create the target (the grant already exists at its anchor) and does not relocate it — it
/// re-asserts `MS_NOEXEC` in place so a `T-pinned` entrypoint cannot `execve` or `mmap(PROT_EXEC)`
/// a mutable grant (mmap(2) `EPERM`), while leaving the path the workload sees unchanged (I2/I5). The
/// bind is FROM the pinned fd, so a rename/symlink swap of the source cannot redirect it; identity is
/// re-checked after the bind and any drift fails closed.
pub fn seal_noexec_in_place(p: &Pinned, target: &Path) -> io::Result<()> {
    let src = CString::new(format!("/proc/self/fd/{}", p.fd.as_raw_fd()))
        .map_err(|_| io::Error::from_raw_os_error(EINVAL))?;
    let tgt = path_cstr(target)?;

    mount(&src, &tgt, None, MS_BIND, None)?;
    match ident_at_path(target) {
        Ok(id) if id == p.ident => {}
        Ok(_) => {
            let _ = umount2(&tgt, 0);
            return Err(io::Error::new(io::ErrorKind::Other, "identity drift after grant noexec bind"));
        }
        Err(e) => {
            let _ = umount2(&tgt, 0);
            return Err(e);
        }
    }
    mount(
        &src,
        &tgt,
        None,
        MS_BIND | MS_REMOUNT | MS_RDONLY | MS_NOSUID | MS_NODEV | MS_NOEXEC,
        None,
    )
}

/// Measure an island target's fs-verity digest. Must open the path `O_RDONLY` (the verity ioctl is
/// rejected on `O_PATH` fds) under `RESOLVE_NO_SYMLINKS` — the target is a broker-owned island path,
/// so no symlink may sit on it, but we resolve strictly regardless.
fn measure_at_path(p: &Path) -> io::Result<(u16, Vec<u8>)> {
    let how = OpenHow {
        flags: O_RDONLY | O_CLOEXEC,
        resolve: RESOLVE_NO_SYMLINKS,
        ..Default::default()
    };
    let fd = openat2(AT_FDCWD as RawFd, &path_cstr(p)?, &how)?;
    measure_verity(fd.as_raw_fd())
}

/// slice-9 — bind a `T-pinned` entrypoint inode onto a broker-owned island path and harden it
/// `RO|NOSUID|NODEV` **without `MS_NOEXEC`**. This one dropped flag — for exactly one re-verified
/// inode — is the whole exec-home boundary (docs/phase5-slice9-pin-exec-home.md §3).
///
/// `oracle_fd` is `der.exec_fd`: the `O_RDONLY` fd measured during derivation — the identity + digest
/// AUTHORITY. It cannot itself be the bind source: an `MS_BIND` of `/proc/self/fd/N` fails `EINVAL`
/// when N's `vfsmount` belongs to a DIFFERENT mount namespace (the fd was opened in the gatekeeper's
/// ns; this runs in the per-request private ns). So we re-open the entrypoint BY PATH here — inside
/// this ns, giving an ns-local `vfsmount` that CAN be a bind source — and prove it is the same object:
///   (a) `(dev,ino)` == the derived fd's inode (a rename/symlink swap resolves to a different inode →
///       caught; `RESOLVE_NO_SYMLINKS|NO_MAGICLINKS` blocks link tricks); and
///   (b) fs-verity digest == the derived fd's digest (forging this is a content-hash preimage). The
///       island bind is then re-checked for (a)+(b) again, so drift at any step fails closed.
/// The exec surface's identity therefore stays bound to the derived evidence, not the reopened path
/// (I1/I3); the path is only a handle to fetch an ns-local reference to the SAME inode.
pub fn relocate_exec_island(entry_path: &Path, oracle_fd: RawFd, target: &Path) -> io::Result<()> {
    let lbl = |s: &str, e: io::Error| io::Error::new(e.kind(), format!("{s}: {e}"));

    // Ground truth from the derived evidence fd.
    let expect_ident = ident_of(&statx_fd(oracle_fd).map_err(|e| lbl("statx-oracle", e))?)?;
    let expect_digest = measure_verity(oracle_fd).map_err(|e| lbl("measure-oracle", e))?;

    // Re-open the entrypoint O_RDONLY in THIS mount ns (TOCTOU-safe: no symlink/magiclink). O_RDONLY
    // both lets us re-measure fs-verity and gives an ns-local vfsmount usable as a bind source.
    let how = OpenHow {
        flags: O_RDONLY | O_CLOEXEC,
        resolve: RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS,
        ..Default::default()
    };
    let local = openat2(AT_FDCWD as RawFd, &path_cstr(entry_path)?, &how).map_err(|e| lbl("reopen-entry", e))?;

    // The re-opened path must be the EXACT inode + fs-verity digest the derivation pinned.
    if ident_of(&statx_fd(local.as_raw_fd())?)? != expect_ident {
        return Err(io::Error::new(io::ErrorKind::Other, "entrypoint inode drift vs derived fd"));
    }
    if measure_verity(local.as_raw_fd()).map_err(|e| lbl("measure-local", e))? != expect_digest {
        return Err(io::Error::new(io::ErrorKind::Other, "entrypoint fs-verity digest drift vs derived fd"));
    }

    // The island mountpoint is a FILE (the entrypoint), not a directory: create the parent tree and
    // an empty file to bind over.
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| lbl("mkdir-island", e))?;
    }
    std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(target).map_err(|e| lbl("touch-island", e))?;

    let src = CString::new(format!("/proc/self/fd/{}", local.as_raw_fd()))
        .map_err(|_| io::Error::from_raw_os_error(EINVAL))?;
    let tgt = path_cstr(target)?;

    // Bind FROM the ns-local fd — the source inode is fixed, immune to a path swap.
    mount(&src, &tgt, None, MS_BIND, None).map_err(|e| lbl("bind", e))?;

    // (a) identity: whatever is now at target must be the exact inode we measured.
    match ident_at_path(target) {
        Ok(id) if id == expect_ident => {}
        Ok(_) => {
            let _ = umount2(&tgt, 0);
            return Err(io::Error::new(io::ErrorKind::Other, "identity drift after exec-island bind"));
        }
        Err(e) => {
            let _ = umount2(&tgt, 0);
            return Err(e);
        }
    }

    // (b) fs-verity: re-measure the island inode; the digest must still equal what the source carried.
    match measure_at_path(target) {
        Ok(d) if d == expect_digest => {}
        Ok(_) => {
            let _ = umount2(&tgt, 0);
            return Err(io::Error::new(io::ErrorKind::Other, "fs-verity digest drift after exec-island bind"));
        }
        Err(e) => {
            let _ = umount2(&tgt, 0);
            return Err(lbl("measure-island", e));
        }
    }

    // Read-only hardening WITHOUT MS_NOEXEC — the single deliberate omission, for this one inode that
    // is now proven fs-verity-immutable + identity-matched. Every other mount keeps NOEXEC (I2/I5).
    mount(
        &src,
        &tgt,
        None,
        MS_BIND | MS_REMOUNT | MS_RDONLY | MS_NOSUID | MS_NODEV,
        None,
    )
    .map_err(|e| lbl("remount-ro", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The core TOCTOU property, provable WITHOUT mount privilege: once pinned, an `O_PATH` fd holds
    /// the original inode even after the source path is swapped to a symlink at an ungranted target,
    /// and the swapped path can no longer be re-pinned (NO_SYMLINKS rejects it).
    #[test]
    fn pinned_fd_survives_a_source_swap() {
        let base = std::env::temp_dir().join(format!("shrek-pin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("project")).unwrap();
        std::fs::create_dir_all(base.join("vault")).unwrap();
        std::fs::write(base.join("project/marker"), b"PROJECT").unwrap();
        std::fs::write(base.join("vault/marker"), b"VAULT").unwrap();

        let anchor = open_anchor(&base).unwrap();
        let pinned = pin_beneath(&anchor, "project").unwrap();
        let orig = pinned.ident;
        assert!(pinned.is_dir);

        // Adversary swaps the granted path AFTER the pin: project -> symlink to vault.
        std::fs::rename(base.join("project"), base.join("project.orig")).unwrap();
        std::os::unix::fs::symlink(base.join("vault"), base.join("project")).unwrap();

        // The pinned fd still resolves to the ORIGINAL inode — the swap is a no-op against it.
        let after = ident_of(&statx_fd(pinned.fd.as_raw_fd()).unwrap()).unwrap();
        assert_eq!(after, orig, "pinned fd must still point at the original inode");

        // And re-pinning the now-symlinked path is refused outright (NO_SYMLINKS -> ELOOP=40).
        let repin = pin_beneath(&anchor, "project").unwrap_err();
        assert_eq!(repin.raw_os_error(), Some(40), "swapped symlink path must not re-pin");

        let _ = std::fs::remove_dir_all(&base);
    }
}
