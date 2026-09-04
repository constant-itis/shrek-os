//! uapi — the ONE raw syscall egressd needs: `getsockopt(SO_PEERCRED)` to read the connecting peer's
//! `(pid, uid, gid)` off a `UnixStream`. std exposes no peer-credential API, and pulling the whole
//! sandbox-broker crate (`gatekeeperd::linux_uapi`) just for this would couple the desktop-egress plane
//! to it — so egressd keeps its own tiny, dep-free copy (mirrors `gatekeeperd::linux_uapi::peer_cred`).
//!
//! x86-64 ONLY — the sealed image target (`image/mkosi.conf`: `shrek_*_x86-64`; the freestanding coder
//! stub is likewise "raw x86-64 syscalls"). A `compile_error!` guards other arches so a future port is a
//! loud, deliberate act, never a silent mis-read of peer identity (which would be a security bug: the
//! whole socket boundary rests on this uid).

use std::io;
use std::os::fd::RawFd;

const SOL_SOCKET: i64 = 1;
const SO_PEERCRED: i64 = 17;

/// `struct ucred` (SO_PEERCRED) — 12 bytes, no padding.
#[repr(C)]
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ucred {
    pub pid: i32,
    pub uid: u32,
    pub gid: u32,
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn getsockopt(fd: i64, level: i64, optname: i64, optval: i64, optlen: i64) -> i64 {
    // SYS_getsockopt = 55 on x86-64. Clobbers rcx/r11 per the syscall ABI; rax carries the return.
    let ret: i64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") 55i64 => ret,
        in("rdi") fd,
        in("rsi") level,
        in("rdx") optname,
        in("r10") optval,
        in("r8") optlen,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack, preserves_flags)
    );
    ret
}

#[cfg(not(target_arch = "x86_64"))]
compile_error!(
    "egressd::uapi::peer_cred is x86-64 only (the sealed image target). \
     Port getsockopt(SO_PEERCRED) before building for another architecture."
);

/// Read the connecting peer's `(pid, uid, gid)` — the authoritative, un-forgeable kernel view of who is
/// on the other end of `fd`. The socket boundary's identity gate rests on this; a caller cannot lie
/// about its uid. Errors on a syscall failure (the supervisor then drops the connection, fail-closed).
pub fn peer_cred(fd: RawFd) -> io::Result<Ucred> {
    let mut cred = Ucred::default();
    let mut len: u32 = core::mem::size_of::<Ucred>() as u32;
    let ret = unsafe {
        getsockopt(
            fd as i64,
            SOL_SOCKET,
            SO_PEERCRED,
            &mut cred as *mut Ucred as i64,
            &mut len as *mut u32 as i64,
        )
    };
    if ret < 0 {
        return Err(io::Error::from_raw_os_error(-ret as i32));
    }
    if len as usize != core::mem::size_of::<Ucred>() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "short ucred"));
    }
    Ok(cred)
}

/// `LOCK_EX` (2) — an exclusive advisory lock; the whole store-mutation critical section takes it.
pub const LOCK_EX: i32 = 2;

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn flock_raw(fd: i64, op: i64) -> i64 {
    // SYS_flock = 73 on x86-64. Blocks until the lock is acquired (no LOCK_NB), so two root writers
    // serialize rather than fail. Clobbers rcx/r11 per the syscall ABI; rax carries the return.
    let ret: i64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") 73i64 => ret,
        in("rdi") fd,
        in("rsi") op,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack, preserves_flags)
    );
    ret
}

/// Take an advisory lock (`flock(2)`) on `fd`. Released automatically when the fd is closed (the guard
/// in [`crate::store::lock_store`] holds the file open for the critical section). Errors fail-closed.
pub fn flock(fd: RawFd, op: i32) -> io::Result<()> {
    let ret = unsafe { flock_raw(fd as i64, op as i64) };
    if ret < 0 {
        return Err(io::Error::from_raw_os_error(-ret as i32));
    }
    Ok(())
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn geteuid_raw() -> i64 {
    // SYS_geteuid = 107 on x86-64. Cannot fail. Clobbers rcx/r11 per the syscall ABI.
    let ret: i64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") 107i64 => ret,
        lateout("rcx") _,
        lateout("r11") _,
        options(nostack, preserves_flags)
    );
    ret
}

/// The caller's effective uid. The `confirmed-*` verbs gate on `geteuid() == 0`: they are the root-only
/// ceremony-commit surface (gatekeeperd execs them as root after a confirmed SAK ceremony), and MUST
/// refuse a uid-1000 invocation so the ceremony can never be bypassed by calling the verb directly.
pub fn geteuid() -> u32 {
    (unsafe { geteuid_raw() }) as u32
}
