//! linux_uapi — the raw syscalls swampd needs, x86-64 only.
//!
//! This is a DELIBERATE, minimal MIRROR of `gatekeeperd/src/linux_uapi.rs` — the same proven ABI
//! structs and raw `syscall` idiom (sizes asserted vs the kernel there and here), copied rather than
//! shared so this availability-plane daemon does not couple to, or force a re-layout of, the frozen
//! privileged broker crate. It carries ONLY what swampd calls: Landlock (self-confinement to the
//! sealed indexable allow-set, swamp.md §5), `SO_PEERCRED` (authenticate a query-socket peer, §9),
//! `PR_SET_NO_NEW_PRIVS` (the precondition for unprivileged `landlock_restrict_self`), and an
//! `openat2` O_PATH open to obtain the directory fds Landlock rules are anchored beneath.
//!
//! If this surface grows, factor a shared `shrek-sys` crate (the follow noted in the slice doc) —
//! but that touches gatekeeperd, so it is out of THIS slice by design.
//!
//! **x86-64 ONLY.** No fallback: a missing syscall returns its raw errno and the caller fails closed.

#![allow(dead_code)]

use std::ffi::CStr;
use std::io;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};

// ---- syscall numbers (x86-64) ----
pub const SYS_GETSOCKOPT: i64 = 55;
pub const SYS_PRCTL: i64 = 157;
pub const SYS_OPENAT2: i64 = 437;
pub const SYS_LANDLOCK_CREATE_RULESET: i64 = 444;
pub const SYS_LANDLOCK_ADD_RULE: i64 = 445;
pub const SYS_LANDLOCK_RESTRICT_SELF: i64 = 446;

// ---- prctl ----
pub const PR_SET_NO_NEW_PRIVS: i64 = 38;

// ---- Landlock uapi (linux/landlock.h) ----
pub const LANDLOCK_CREATE_RULESET_VERSION: u64 = 1 << 0;
pub const LANDLOCK_RULE_PATH_BENEATH: i64 = 1;
pub const LANDLOCK_ACCESS_FS_EXECUTE: u64 = 1 << 0;
pub const LANDLOCK_ACCESS_FS_WRITE_FILE: u64 = 1 << 1;
pub const LANDLOCK_ACCESS_FS_READ_FILE: u64 = 1 << 2;
pub const LANDLOCK_ACCESS_FS_READ_DIR: u64 = 1 << 3;
pub const LANDLOCK_ACCESS_FS_REMOVE_DIR: u64 = 1 << 4;
pub const LANDLOCK_ACCESS_FS_REMOVE_FILE: u64 = 1 << 5;
pub const LANDLOCK_ACCESS_FS_MAKE_CHAR: u64 = 1 << 6;
pub const LANDLOCK_ACCESS_FS_MAKE_DIR: u64 = 1 << 7;
pub const LANDLOCK_ACCESS_FS_MAKE_REG: u64 = 1 << 8;
pub const LANDLOCK_ACCESS_FS_MAKE_SOCK: u64 = 1 << 9;
pub const LANDLOCK_ACCESS_FS_MAKE_FIFO: u64 = 1 << 10;
pub const LANDLOCK_ACCESS_FS_MAKE_BLOCK: u64 = 1 << 11;
pub const LANDLOCK_ACCESS_FS_MAKE_SYM: u64 = 1 << 12;
pub const LANDLOCK_ACCESS_FS_REFER: u64 = 1 << 13; // ABI v2
pub const LANDLOCK_ACCESS_FS_TRUNCATE: u64 = 1 << 14; // ABI v3
pub const LANDLOCK_ACCESS_FS_IOCTL_DEV: u64 = 1 << 15; // ABI v5

// ---- open flags / openat2 ----
pub const O_CLOEXEC: u64 = 0o2000000;
pub const O_DIRECTORY: u64 = 0o200000;
pub const O_PATH: u64 = 0o10000000;
pub const AT_FDCWD: i64 = -100;

// ---- getsockopt (SO_PEERCRED) ----
pub const SOL_SOCKET: i32 = 1;
pub const SO_PEERCRED: i32 = 17;

// -------------------------------------------------------------------------------------------------
// ABI structs — exact kernel layout (sizes asserted in tests, matching gatekeeperd).
// -------------------------------------------------------------------------------------------------

/// `struct open_how` (linux/openat2.h) — three u64, 24 bytes.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct OpenHow {
    pub flags: u64,
    pub mode: u64,
    pub resolve: u64,
}

/// `struct ucred` (SO_PEERCRED) — 12 bytes, no padding.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Ucred {
    pub pid: i32,
    pub uid: u32,
    pub gid: u32,
}

/// `struct landlock_ruleset_attr` — 3×u64.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct LandlockRulesetAttr {
    pub handled_access_fs: u64,
    pub handled_access_net: u64,
    pub scoped: u64,
}

/// `struct landlock_path_beneath_attr` — **PACKED** u64+s32, 12 bytes. The packing is load-bearing:
/// a natural 16-byte layout mis-places `parent_fd` and silently mis-scopes every rule.
#[repr(C, packed)]
#[derive(Default, Clone, Copy)]
pub struct LandlockPathBeneathAttr {
    pub allowed_access: u64,
    pub parent_fd: i32,
}

// -------------------------------------------------------------------------------------------------
// Raw syscall primitives (x86-64). rax=nr; args rdi,rsi,rdx,r10,r8; rcx/r11 clobbered; ret in rax.
// -------------------------------------------------------------------------------------------------

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
unsafe fn sc3(n: i64, a1: i64, a2: i64, a3: i64) -> i64 {
    let ret: i64;
    core::arch::asm!("syscall",
        inlateout("rax") n => ret,
        in("rdi") a1, in("rsi") a2, in("rdx") a3,
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

#[inline]
fn res(ret: i64) -> io::Result<i64> {
    if ret < 0 {
        Err(io::Error::from_raw_os_error(-ret as i32))
    } else {
        Ok(ret)
    }
}

// -------------------------------------------------------------------------------------------------
// Typed wrappers.
// -------------------------------------------------------------------------------------------------

/// Open `path` as an `O_PATH` directory fd (the anchor a Landlock `path_beneath` rule attaches to).
/// `O_PATH` needs no read permission at open time and never follows into the file — exactly right for
/// naming a directory to the kernel. Trusted absolute paths only (no resolve restrictions).
pub fn open_path_dir(path: &CStr) -> io::Result<OwnedFd> {
    let how = OpenHow { flags: O_PATH | O_DIRECTORY | O_CLOEXEC, mode: 0, resolve: 0 };
    let ret = unsafe {
        sc4(
            SYS_OPENAT2,
            AT_FDCWD,
            path.as_ptr() as i64,
            &how as *const OpenHow as i64,
            core::mem::size_of::<OpenHow>() as i64,
        )
    };
    let fd = res(ret)? as RawFd;
    // Safety: a successful openat2 yields a fresh, owned fd.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Read the connecting peer's (pid,uid,gid) — the unspoofable identity of a unix-socket peer.
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

/// `prctl(PR_SET_NO_NEW_PRIVS, 1, …)` — irreversible, the precondition for unprivileged
/// `landlock_restrict_self`.
pub fn set_no_new_privs() -> io::Result<()> {
    let ret = unsafe { sc5(SYS_PRCTL, PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    res(ret).map(|_| ())
}

/// Probe the highest supported Landlock ABI version (>=1). ENOSYS = no Landlock; EOPNOTSUPP = compiled
/// in but disabled at boot. Either way the caller fails closed (swampd refuses to serve unconfined).
pub fn landlock_abi_version() -> io::Result<i64> {
    let ret = unsafe { sc3(SYS_LANDLOCK_CREATE_RULESET, 0, 0, LANDLOCK_CREATE_RULESET_VERSION as i64) };
    res(ret)
}

/// Create an empty ruleset handling the given access classes; returns the ruleset fd.
pub fn landlock_create_ruleset(attr: &LandlockRulesetAttr) -> io::Result<OwnedFd> {
    let ret = unsafe {
        sc3(
            SYS_LANDLOCK_CREATE_RULESET,
            attr as *const LandlockRulesetAttr as i64,
            core::mem::size_of::<LandlockRulesetAttr>() as i64,
            0,
        )
    };
    let fd = res(ret)? as RawFd;
    // Safety: a successful create yields a fresh, owned ruleset fd.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Grant `allowed_access` on everything beneath `attr.parent_fd`.
pub fn landlock_add_path_beneath(ruleset_fd: RawFd, attr: &LandlockPathBeneathAttr) -> io::Result<()> {
    let ret = unsafe {
        sc4(
            SYS_LANDLOCK_ADD_RULE,
            ruleset_fd as i64,
            LANDLOCK_RULE_PATH_BENEATH,
            attr as *const LandlockPathBeneathAttr as i64,
            0,
        )
    };
    res(ret).map(|_| ())
}

/// Enforce the ruleset on the calling thread and every future child/exec. Irreversible; requires
/// no_new_privs first.
pub fn landlock_restrict_self(ruleset_fd: RawFd) -> io::Result<()> {
    let ret = unsafe { sc2(SYS_LANDLOCK_RESTRICT_SELF, ruleset_fd as i64, 0) };
    res(ret).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, offset_of, size_of};

    #[test]
    fn struct_sizes_match_kernel_abi() {
        assert_eq!(size_of::<OpenHow>(), 24, "open_how is 3×u64");
        assert_eq!(size_of::<Ucred>(), 12, "struct ucred is 12 bytes, no padding");
        assert_eq!(size_of::<LandlockRulesetAttr>(), 24, "3×u64");
        assert_eq!(size_of::<LandlockPathBeneathAttr>(), 12, "packed u64+s32 — no padding");
        assert_eq!(offset_of!(LandlockPathBeneathAttr, allowed_access), 0);
        assert_eq!(offset_of!(LandlockPathBeneathAttr, parent_fd), 8);
        assert_eq!(align_of::<Ucred>(), 4);
    }
}
