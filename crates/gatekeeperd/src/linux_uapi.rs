//! linux_uapi — the ONE place raw Linux syscalls and their ABI structs live.
//!
//! Rationale: the control plane is dependency-free by design (see workspace `Cargo.toml`). Rather
//! than pull `rustix`/`libc`/`nix` into a sealed, privileged (`CAP_SYS_ADMIN`) binary, gatekeeperd
//! issues the handful of syscalls it needs directly. Every syscall number, every `#[repr(C)]` ABI
//! struct, and every raw `syscall` instruction is centralized here — nothing scattered through the
//! rest of the crate. If this surface grows materially, revisit the rustix trade-off (a deliberate
//! decision recorded in docs/phase5-slice1-mount.md).
//!
//! **x86-64 ONLY.** Syscall numbers and the register calling convention below are amd64-specific.
//! Shrek OS is built exclusively for x86-64 (`image/mkosi.conf`: linux-image-amd64, `%a=x86-64`).
//! There is no fallback path: if a needed syscall is unavailable the caller gets the raw errno and
//! must fail closed (the security posture forbids silently degrading to a weaker primitive).

#![allow(dead_code)] // some constants/wrappers are used only by specific gates

use std::ffi::CStr;
use std::io;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};

// ---- syscall numbers (x86-64) ----
pub const SYS_GETSOCKOPT: i64 = 55;
pub const SYS_MOUNT: i64 = 165;
pub const SYS_UMOUNT2: i64 = 166;
pub const SYS_UNSHARE: i64 = 272;
pub const SYS_STATX: i64 = 332;
pub const SYS_OPENAT2: i64 = 437;

// ---- unshare / mount-propagation ----
pub const CLONE_NEWNS: i64 = 0x00020000;
pub const MS_PRIVATE: u64 = 1 << 18;

// ---- file-type bits (statx st_mode) ----
pub const S_IFMT: u32 = 0o170000;
pub const S_IFDIR: u32 = 0o040000;

// ---- open flags (subset used) ----
pub const O_RDONLY: u64 = 0;
pub const O_CLOEXEC: u64 = 0o2000000;
pub const O_DIRECTORY: u64 = 0o200000;
pub const O_PATH: u64 = 0o10000000;
pub const O_NOFOLLOW: u64 = 0o400000;

// ---- openat2 resolve flags (linux/openat2.h) ----
pub const RESOLVE_NO_XDEV: u64 = 0x01;
pub const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
pub const RESOLVE_NO_SYMLINKS: u64 = 0x04;
pub const RESOLVE_BENEATH: u64 = 0x08;
pub const RESOLVE_IN_ROOT: u64 = 0x10;

// ---- *at() dirfd + flags ----
pub const AT_FDCWD: i64 = -100;
pub const AT_EMPTY_PATH: i32 = 0x1000;
pub const AT_SYMLINK_NOFOLLOW: i32 = 0x100;

// ---- statx mask bits (subset) ----
pub const STATX_TYPE: u32 = 0x0001;
pub const STATX_INO: u32 = 0x0100;

// ---- mount flags (subset) ----
pub const MS_RDONLY: u64 = 1;
pub const MS_NOSUID: u64 = 2;
pub const MS_NODEV: u64 = 4;
pub const MS_NOEXEC: u64 = 8;
pub const MS_REMOUNT: u64 = 32;
pub const MS_BIND: u64 = 4096;
pub const MS_REC: u64 = 16384;

// ---- getsockopt (SO_PEERCRED) ----
pub const SOL_SOCKET: i32 = 1;
pub const SO_PEERCRED: i32 = 17;

// -------------------------------------------------------------------------------------------------
// ABI structs — exact kernel layout. Sizes are asserted in the test module below.
// -------------------------------------------------------------------------------------------------

/// `struct open_how` (linux/openat2.h) — exactly three u64 (24 bytes). Passed by pointer to
/// openat2, with its size as the 4th argument (the kernel rejects an unexpected size).
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct OpenHow {
    pub flags: u64,
    pub mode: u64,
    pub resolve: u64,
}

/// `struct statx_timestamp` (linux/stat.h) — 16 bytes.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct StatxTimestamp {
    pub tv_sec: i64,
    pub tv_nsec: u32,
    pub __reserved: i32,
}

/// `struct statx` (linux/stat.h) — 256 bytes. Only the identity fields (mask, type, ino, dev) are
/// read by the mount plane; the rest is carried verbatim so the ABI size is exact.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Statx {
    pub stx_mask: u32,
    pub stx_blksize: u32,
    pub stx_attributes: u64,
    pub stx_nlink: u32,
    pub stx_uid: u32,
    pub stx_gid: u32,
    pub stx_mode: u16,
    pub __spare0: u16,
    pub stx_ino: u64,
    pub stx_size: u64,
    pub stx_blocks: u64,
    pub stx_attributes_mask: u64,
    pub stx_atime: StatxTimestamp,
    pub stx_btime: StatxTimestamp,
    pub stx_ctime: StatxTimestamp,
    pub stx_mtime: StatxTimestamp,
    pub stx_rdev_major: u32,
    pub stx_rdev_minor: u32,
    pub stx_dev_major: u32,
    pub stx_dev_minor: u32,
    pub stx_mnt_id: u64,
    pub stx_dio_mem_align: u32,
    pub stx_dio_offset_align: u32,
    pub __spare3: [u64; 12],
}

/// `struct ucred` (SO_PEERCRED) — 12 bytes, no padding.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Ucred {
    pub pid: i32,
    pub uid: u32,
    pub gid: u32,
}

// -------------------------------------------------------------------------------------------------
// Raw syscall primitives (x86-64). rax=nr; args rdi,rsi,rdx,r10,r8; rcx/r11 clobbered; ret in rax.
// No `nomem`: the implied memory clobber is REQUIRED — several of these write through pointer args
// (statx buffer, ucred, openat2's open_how is read). `nostack` matches the existing peer_cred idiom.
// -------------------------------------------------------------------------------------------------

#[inline]
unsafe fn sc1(n: i64, a1: i64) -> i64 {
    let ret: i64;
    core::arch::asm!("syscall",
        inlateout("rax") n => ret,
        in("rdi") a1,
        lateout("rcx") _, lateout("r11") _, options(nostack));
    ret
}

#[inline]
unsafe fn sc2(n: i64, a1: i64, a2: i64) -> i64 {
    let ret: i64;
    core::arch::asm!("syscall",
        inlateout("rax") n => ret,
        in("rdi") a1, in("rsi") a2,
        lateout("rcx") _, lateout("r11") _, options(nostack));
    ret
}

#[inline]
unsafe fn sc4(n: i64, a1: i64, a2: i64, a3: i64, a4: i64) -> i64 {
    let ret: i64;
    core::arch::asm!("syscall",
        inlateout("rax") n => ret,
        in("rdi") a1, in("rsi") a2, in("rdx") a3, in("r10") a4,
        lateout("rcx") _, lateout("r11") _, options(nostack));
    ret
}

#[inline]
unsafe fn sc5(n: i64, a1: i64, a2: i64, a3: i64, a4: i64, a5: i64) -> i64 {
    let ret: i64;
    core::arch::asm!("syscall",
        inlateout("rax") n => ret,
        in("rdi") a1, in("rsi") a2, in("rdx") a3, in("r10") a4, in("r8") a5,
        lateout("rcx") _, lateout("r11") _, options(nostack));
    ret
}

/// Map a raw syscall return (negative = -errno) into an `io::Result`.
#[inline]
fn res(ret: i64) -> io::Result<i64> {
    if ret < 0 {
        Err(io::Error::from_raw_os_error(-ret as i32))
    } else {
        Ok(ret)
    }
}

// -------------------------------------------------------------------------------------------------
// Typed wrappers — the only surface the rest of the crate calls.
// -------------------------------------------------------------------------------------------------

/// `openat2(dirfd, path, &how, sizeof how)`. Returns an owned fd. The caller supplies the resolve
/// flags (e.g. RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS) — there is no softened
/// fallback: an old kernel returns ENOSYS and the caller must fail closed.
pub fn openat2(dirfd: RawFd, path: &CStr, how: &OpenHow) -> io::Result<OwnedFd> {
    let ret = unsafe {
        sc4(
            SYS_OPENAT2,
            dirfd as i64,
            path.as_ptr() as i64,
            how as *const OpenHow as i64,
            core::mem::size_of::<OpenHow>() as i64,
        )
    };
    let fd = res(ret)? as RawFd;
    // Safety: a successful openat2 yields a fresh, owned file descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// `statx` on an already-open fd via AT_EMPTY_PATH, requesting the identity mask (type+ino). Used to
/// record dev/inode of a pinned fd and re-verify it after the relocate bind.
pub fn statx_fd(fd: RawFd) -> io::Result<Statx> {
    let mut buf = Statx::default();
    let empty = c"";
    let ret = unsafe {
        sc5(
            SYS_STATX,
            fd as i64,
            empty.as_ptr() as i64,
            AT_EMPTY_PATH as i64,
            (STATX_TYPE | STATX_INO) as i64,
            &mut buf as *mut Statx as i64,
        )
    };
    res(ret)?;
    Ok(buf)
}

/// `mount(source, target, fstype, flags, data)`. fstype/data are optional (NULL for a bind).
pub fn mount(
    source: &CStr,
    target: &CStr,
    fstype: Option<&CStr>,
    flags: u64,
    data: Option<&CStr>,
) -> io::Result<()> {
    let fsp = fstype.map(|c| c.as_ptr()).unwrap_or(core::ptr::null());
    let datp = data.map(|c| c.as_ptr()).unwrap_or(core::ptr::null());
    let ret = unsafe {
        sc5(
            SYS_MOUNT,
            source.as_ptr() as i64,
            target.as_ptr() as i64,
            fsp as i64,
            flags as i64,
            datp as i64,
        )
    };
    res(ret).map(|_| ())
}

/// `umount2(target, flags)`.
pub fn umount2(target: &CStr, flags: i32) -> io::Result<()> {
    let ret = unsafe { sc2(SYS_UMOUNT2, target.as_ptr() as i64, flags as i64) };
    res(ret).map(|_| ())
}

/// `unshare(flags)` — used with CLONE_NEWNS to give the caller a private mount namespace so the
/// relocate binds are contained and never touch the host/parent mount table.
pub fn unshare(flags: i64) -> io::Result<()> {
    let ret = unsafe { sc1(SYS_UNSHARE, flags) };
    res(ret).map(|_| ())
}

/// Read the connecting peer's (pid,uid,gid) via `getsockopt(SO_PEERCRED)`. This is the authoritative,
/// unspoofable identity of a unix-socket peer — the broker's per-uid admission gate.
pub fn peer_cred(fd: RawFd) -> io::Result<Ucred> {
    let mut cred = Ucred::default();
    let mut len: u32 = core::mem::size_of::<Ucred>() as u32;
    let ret = unsafe {
        sc5(
            SYS_GETSOCKOPT,
            fd as i64,
            SOL_SOCKET as i64,
            SO_PEERCRED as i64,
            &mut cred as *mut Ucred as i64,
            &mut len as *mut u32 as i64,
        )
    };
    res(ret)?;
    Ok(cred)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, offset_of, size_of};

    #[test]
    fn struct_sizes_match_kernel_abi() {
        assert_eq!(size_of::<OpenHow>(), 24, "open_how must be 3×u64");
        assert_eq!(size_of::<StatxTimestamp>(), 16, "statx_timestamp is 16 bytes");
        assert_eq!(size_of::<Statx>(), 256, "struct statx is 256 bytes");
        assert_eq!(size_of::<Ucred>(), 12, "struct ucred is 12 bytes, no padding");
        assert_eq!(align_of::<Statx>(), 8);
    }

    #[test]
    fn statx_field_offsets_are_stable() {
        // The four identity fields the mount plane reads. Offsets are fixed in the kernel ABI
        // regardless of which trailing __spare fields newer kernels repurpose.
        assert_eq!(offset_of!(Statx, stx_mask), 0);
        assert_eq!(offset_of!(Statx, stx_ino), 32);
        assert_eq!(offset_of!(Statx, stx_mode), 28);
        assert_eq!(offset_of!(Statx, stx_dev_major), 136);
        assert_eq!(offset_of!(Statx, stx_dev_minor), 140);
    }

    #[test]
    fn openhow_field_offsets() {
        assert_eq!(offset_of!(OpenHow, flags), 0);
        assert_eq!(offset_of!(OpenHow, mode), 8);
        assert_eq!(offset_of!(OpenHow, resolve), 16);
    }

    // ---- kernel-behavior tests (unprivileged: pure path resolution, no mount) ----

    #[test]
    fn openat2_opens_a_plain_dir_beneath_anchor() {
        let dir = std::env::temp_dir().join(format!("shrek-uapi-plain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("project")).unwrap();
        let anchor = open_dir(&dir);
        let how = OpenHow {
            flags: O_PATH | O_CLOEXEC | O_DIRECTORY,
            resolve: RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS,
            ..Default::default()
        };
        let fd = openat2(anchor, c"project", &how).expect("plain dir beneath anchor must open");
        let st = statx_fd(fd.as_raw_fd_ext()).unwrap();
        assert!(st.stx_mask & STATX_INO != 0, "kernel returned the inode");
        assert!(st.stx_ino != 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn openat2_no_symlinks_rejects_a_symlink_component() {
        let dir = std::env::temp_dir().join(format!("shrek-uapi-sym-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("real")).unwrap();
        std::os::unix::fs::symlink("real", dir.join("link")).unwrap();
        let anchor = open_dir(&dir);
        let how = OpenHow {
            flags: O_PATH | O_CLOEXEC,
            resolve: RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS,
            ..Default::default()
        };
        // "real" opens; the symlink "link" is refused with ELOOP — the TOCTOU defense in one call.
        assert!(openat2(anchor, c"real", &how).is_ok());
        let err = openat2(anchor, c"link", &how).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc_eloop()), "NO_SYMLINKS must reject a symlink");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn openat2_beneath_rejects_parent_escape() {
        let dir = std::env::temp_dir().join(format!("shrek-uapi-esc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let anchor = open_dir(&dir);
        let how = OpenHow {
            flags: O_PATH | O_CLOEXEC,
            resolve: RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS,
            ..Default::default()
        };
        // ".." tries to climb above the anchor — RESOLVE_BENEATH blocks it (EXDEV).
        let err = openat2(anchor, c"..", &how).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(libc_exdev()), "BENEATH must block a parent escape");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- tiny test helpers (avoid a libc dep even in tests) ---
    fn libc_eloop() -> i32 {
        40
    }
    fn libc_exdev() -> i32 {
        18
    }
    fn open_dir(p: &std::path::Path) -> RawFd {
        use std::os::fd::IntoRawFd;
        std::fs::File::open(p).unwrap().into_raw_fd()
    }
    // Small shim so the test can statx the OwnedFd without importing AsRawFd everywhere.
    trait AsRawFdExt {
        fn as_raw_fd_ext(&self) -> RawFd;
    }
    impl AsRawFdExt for OwnedFd {
        fn as_raw_fd_ext(&self) -> RawFd {
            use std::os::fd::AsRawFd;
            self.as_raw_fd()
        }
    }
}
