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
