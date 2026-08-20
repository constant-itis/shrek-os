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
pub const SYS_IOCTL: i64 = 16;
pub const SYS_FORK: i64 = 57;
pub const SYS_GETSOCKOPT: i64 = 55;
pub const SYS_PRCTL: i64 = 157;
pub const SYS_MOUNT: i64 = 165;
pub const SYS_UMOUNT2: i64 = 166;
pub const SYS_UNSHARE: i64 = 272;
pub const SYS_SECCOMP: i64 = 317;
pub const SYS_STATX: i64 = 332;
pub const SYS_CLOSE_RANGE: i64 = 436;
pub const SYS_OPENAT2: i64 = 437;
// Landlock (Linux 5.13+). Verified against asm/unistd_64.h.
pub const SYS_LANDLOCK_CREATE_RULESET: i64 = 444;
pub const SYS_LANDLOCK_ADD_RULE: i64 = 445;
pub const SYS_LANDLOCK_RESTRICT_SELF: i64 = 446;

// ---- unshare / mount-propagation ----
pub const CLONE_NEWNS: i64 = 0x00020000;
pub const MS_PRIVATE: u64 = 1 << 18;
// Namespace kinds unshared by the T0 process constructor (proc_plane). CLONE_NEWUSER puts the
// CALLER into the new user ns immediately (so it must write uid_map before it can act as root
// inside); CLONE_NEWPID does NOT — only the caller's first CHILD becomes PID 1 there, which is why
// the constructor forks after unsharing.
pub const CLONE_NEWCGROUP: i64 = 0x02000000;
pub const CLONE_NEWUTS: i64 = 0x04000000;
pub const CLONE_NEWIPC: i64 = 0x08000000;
pub const CLONE_NEWUSER: i64 = 0x10000000;
pub const CLONE_NEWPID: i64 = 0x20000000;
pub const CLONE_NEWNET: i64 = 0x40000000;

// ---- prctl (linux/prctl.h) ----
pub const PR_SET_NO_NEW_PRIVS: i64 = 38;

// ---- Landlock uapi (linux/landlock.h) ----
// Passing attr=NULL,size=0,flags=VERSION returns the highest supported ABI version (>=1); the
// filesystem access-right bits below are stable 1<<n in ABI-introduction order (v1: bits 0..=12,
// v2: REFER, v3: TRUNCATE, v5: IOCTL_DEV). handled_access_fs must be masked to the probed ABI.
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

// ---- seccomp uapi (linux/seccomp.h, linux/filter.h, linux/audit.h) ----
pub const SECCOMP_SET_MODE_FILTER: i64 = 1;
pub const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
pub const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
pub const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
pub const AUDIT_ARCH_X86_64: u32 = 0xC000_003E;
// Byte offsets into `struct seccomp_data` for a BPF_LD|BPF_W|BPF_ABS load.
pub const SECCOMP_DATA_NR_OFFSET: u32 = 0;
pub const SECCOMP_DATA_ARCH_OFFSET: u32 = 4;
// classic-BPF instruction encodings (linux/bpf_common.h) used to build the filter.
pub const BPF_LD: u16 = 0x00;
pub const BPF_W: u16 = 0x00;
pub const BPF_ABS: u16 = 0x20;
pub const BPF_JMP: u16 = 0x05;
pub const BPF_JEQ: u16 = 0x10;
pub const BPF_K: u16 = 0x00;
pub const BPF_RET: u16 = 0x06;

// ---- file-type bits (statx st_mode) ----
pub const S_IFMT: u32 = 0o170000;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFREG: u32 = 0o100000;

// ---- open flags (subset used) ----
pub const O_RDONLY: u64 = 0;
pub const O_RDWR: u64 = 0o2;
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

// ---- KVM ioctls (linux/kvm.h) — used ONLY to probe whether the KVM platform is genuinely usable
// (device present is not enough; a nested host often has /dev/kvm but cannot create a VM). Request
// codes are stable ABI: KVM_GET_API_VERSION = _IO(0xAE,0x00), KVM_CREATE_VM = _IO(0xAE,0x01). ----
pub const KVM_GET_API_VERSION: u64 = 0xAE00;
pub const KVM_CREATE_VM: u64 = 0xAE01;
pub const KVM_API_VERSION: i64 = 12; // the kernel contract: applications must refuse any other value

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

/// `struct landlock_ruleset_attr` (linux/landlock.h) — the set of access classes the ruleset
/// *handles* (everything handled-but-not-explicitly-allowed is denied). Three u64 today; older
/// kernels knew fewer fields, so the wrapper passes the size we fill and leaves the tail zeroed
/// (a kernel that predates a field rejects a NON-zero unknown tail, never a zero one).
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct LandlockRulesetAttr {
    pub handled_access_fs: u64,
    pub handled_access_net: u64,
    pub scoped: u64,
}

/// `struct landlock_path_beneath_attr` (linux/landlock.h) — **PACKED**: u64 immediately followed by
/// s32, 12 bytes, NO alignment padding. `#[repr(C, packed)]` is load-bearing — a natural 16-byte
/// layout would feed the kernel a garbage `parent_fd` and silently mis-scope every rule.
#[repr(C, packed)]
#[derive(Default, Clone, Copy)]
pub struct LandlockPathBeneathAttr {
    pub allowed_access: u64,
    pub parent_fd: i32,
}

/// `struct sock_filter` (linux/filter.h) — one classic-BPF instruction, 8 bytes.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct SockFilter {
    pub code: u16,
    pub jt: u8,
    pub jf: u8,
    pub k: u32,
}

/// `struct sock_fprog` (linux/filter.h) — a BPF program: count + pointer. 16 bytes on x86-64 (the
/// u16 `len` is followed by 6 bytes of padding before the 8-byte-aligned pointer).
#[repr(C)]
pub struct SockFprog {
    pub len: u16,
    pub filter: *const SockFilter,
}

// -------------------------------------------------------------------------------------------------
// Raw syscall primitives (x86-64). rax=nr; args rdi,rsi,rdx,r10,r8; rcx/r11 clobbered; ret in rax.
// No `nomem`: the implied memory clobber is REQUIRED — several of these write through pointer args
// (statx buffer, ucred, openat2's open_how is read). `nostack` matches the existing peer_cred idiom.
// -------------------------------------------------------------------------------------------------

#[inline]
unsafe fn sc0(n: i64) -> i64 {
    let ret: i64;
    core::arch::asm!("syscall",
        inlateout("rax") n => ret,
        lateout("rcx") _, lateout("r11") _, options(nostack));
    ret
}

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
unsafe fn sc3(n: i64, a1: i64, a2: i64, a3: i64) -> i64 {
    let ret: i64;
    core::arch::asm!("syscall",
        inlateout("rax") n => ret,
        in("rdi") a1, in("rsi") a2, in("rdx") a3,
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

/// `open(path, O_RDWR | O_CLOEXEC)` via openat2 from AT_FDCWD, no resolve restrictions (the path is a
/// trusted absolute device node, e.g. `/dev/kvm`, not attacker-influenced). Fails closed on any errno
/// (ENOENT if the node is absent, EACCES if unreadable) — the caller treats that as "KVM unusable".
pub fn open_rdwr(path: &CStr) -> io::Result<OwnedFd> {
    let how = OpenHow { flags: O_RDWR | O_CLOEXEC, resolve: 0, ..Default::default() };
    openat2(AT_FDCWD as RawFd, path, &how)
}

/// `ioctl(fd, request, arg)`. Thin raw wrapper — the KVM usability probe is the only caller, issuing
/// KVM_GET_API_VERSION (arg ignored) and KVM_CREATE_VM (arg = machine type 0). Returns the raw ioctl
/// result (an api-version int, or a new VM fd) or the errno.
pub fn ioctl(fd: RawFd, request: u64, arg: u64) -> io::Result<i64> {
    res(unsafe { sc3(SYS_IOCTL, fd as i64, request as i64, arg as i64) })
}

/// `struct fsverity_digest` (linux/fsverity.h): the algorithm id, the digest length, then the digest
/// bytes. The kernel writes all three on `FS_IOC_MEASURE_VERITY`; we oversize `digest` to 64 (SHA-512)
/// so any supported algorithm fits, and pre-load `digest_size` with that capacity (the kernel returns
/// `EOVERFLOW` if the real digest is larger than we advertise).
#[repr(C)]
pub struct FsverityDigest {
    pub digest_algorithm: u16,
    pub digest_size: u16,
    pub digest: [u8; 64],
}

/// `FS_IOC_MEASURE_VERITY = _IOWR('f', 134, struct fsverity_digest)` — the flexible array counts as 0
/// for the size field, so the encoded size is 4. `_IOWR` dir=3, size=4, type='f'(0x66), nr=134(0x86)
/// ⇒ `(3<<30)|(4<<16)|(0x66<<8)|0x86 = 0xC004_6686`.
pub const FS_IOC_MEASURE_VERITY: u64 = 0xC004_6686;

/// Read a file's fs-verity measurement: the kernel-maintained `(digest_algorithm, digest)` of the
/// Merkle root over the file's content. Returns the algorithm id and the exact-length digest, or the
/// errno (`ENODATA` = the file has no fs-verity enabled, `ENOTTY`/`EOPNOTSUPP` = fs/kernel without
/// verity) — every one of which the caller treats as "no measurable pin identity" ⇒ fail high. The fd
/// must be a real readable fd (`O_RDONLY`), not `O_PATH` (ioctls are rejected on `O_PATH` fds).
pub fn measure_verity(fd: RawFd) -> io::Result<(u16, Vec<u8>)> {
    let mut d = FsverityDigest { digest_algorithm: 0, digest_size: 64, digest: [0u8; 64] };
    ioctl(fd, FS_IOC_MEASURE_VERITY, &mut d as *mut FsverityDigest as u64)?;
    let n = d.digest_size as usize;
    if n == 0 || n > d.digest.len() {
        // A digest length outside what the ABI can hold is not a value we trust — fail high.
        return Err(io::Error::from_raw_os_error(22)); // EINVAL
    }
    Ok((d.digest_algorithm, d.digest[..n].to_vec()))
}

/// `struct fsverity_enable_arg` (linux/fsverity.h) — 128 bytes. Only `version`, `hash_algorithm`, and
/// `block_size` are set; salt/signature are unused (0). SPIKE-ONLY: used by the fixture helper to turn
/// on fs-verity for the pin oracle / VM gate, never on the shipped path — the whole enable surface
/// (`FsverityEnableArg` / `FS_IOC_ENABLE_VERITY` / `enable_verity`) is `#[cfg(feature = "spike")]` and
/// absent from a default/production build (finding F1). `measure_verity` (read-only) stays in prod.
#[cfg(feature = "spike")]
#[repr(C)]
pub struct FsverityEnableArg {
    pub version: u32,
    pub hash_algorithm: u32,
    pub block_size: u32,
    pub salt_size: u32,
    pub salt_ptr: u64,
    pub sig_size: u32,
    pub reserved1: u32,
    pub sig_ptr: u64,
    pub reserved2: [u64; 11],
}

/// `FS_IOC_ENABLE_VERITY = _IOW('f', 133, struct fsverity_enable_arg)` — dir=1, size=128, type='f',
/// nr=133 ⇒ `(1<<30)|(128<<16)|(0x66<<8)|0x85 = 0x4080_6685`. SPIKE-ONLY (finding F1).
#[cfg(feature = "spike")]
pub const FS_IOC_ENABLE_VERITY: u64 = 0x4080_6685;

/// Turn on fs-verity (SHA-256, 4K blocks) for a file — SPIKE-ONLY fixture helper (the pin oracle / VM
/// gate creates a real verity inode to measure). The fd must be `O_RDONLY` with no other writers, on a
/// filesystem that has the verity feature. After this the file is immutable + block-verified.
/// Compiled out of default/production builds (`#[cfg(feature = "spike")]`, finding F1).
#[cfg(feature = "spike")]
pub fn enable_verity(fd: RawFd) -> io::Result<()> {
    let arg = FsverityEnableArg {
        version: 1,
        hash_algorithm: 1, // FS_VERITY_HASH_ALG_SHA256
        block_size: 4096,
        salt_size: 0,
        salt_ptr: 0,
        sig_size: 0,
        reserved1: 0,
        sig_ptr: 0,
        reserved2: [0; 11],
    };
    ioctl(fd, FS_IOC_ENABLE_VERITY, &arg as *const FsverityEnableArg as u64)?;
    Ok(())
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

/// `fork()`. Returns the child pid in the parent, 0 in the child. SAFETY: the caller MUST be
/// single-threaded (the whole control plane is synchronous — no tokio/rayon), so the child owns a
/// faithful copy of the one running thread and may use `std` normally between fork and execve.
pub unsafe fn fork() -> io::Result<i64> {
    res(sc0(SYS_FORK))
}

/// `prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0)` — the mandatory precondition for an unprivileged
/// seccomp filter AND for `landlock_restrict_self` without CAP_SYS_ADMIN; also stops a setuid binary
/// from ever regaining privilege inside the sandbox. Set once, irreversible, inherited across execve.
pub fn set_no_new_privs() -> io::Result<()> {
    let ret = unsafe { sc5(SYS_PRCTL, PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
    res(ret).map(|_| ())
}

/// `close_range(first, last, flags)` — scrub inherited file descriptors. Landlock only governs the
/// `open(2)` that happens AFTER `restrict_self`; a descriptor inherited from the privileged parent
/// (e.g. an O_PATH dir fd) is exempt and would be an escape hatch, so the constructor closes
/// everything above stderr before installing the walls. ENOSYS ⇒ caller falls back to a manual loop.
pub fn close_range(first: u32, last: u32, flags: u32) -> io::Result<()> {
    let ret = unsafe { sc3(SYS_CLOSE_RANGE, first as i64, last as i64, flags as i64) };
    res(ret).map(|_| ())
}

/// `landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION)` — probe the highest supported
/// ABI version (>=1). ENOSYS = kernel has no Landlock; EOPNOTSUPP = compiled in but disabled at boot.
/// Either way the caller decides (clean-preflight fall-up to T1); this is a pure query, no state.
pub fn landlock_abi_version() -> io::Result<i64> {
    let ret = unsafe { sc3(SYS_LANDLOCK_CREATE_RULESET, 0, 0, LANDLOCK_CREATE_RULESET_VERSION as i64) };
    res(ret)
}

/// `landlock_create_ruleset(&attr, sizeof attr, 0)` — create an empty ruleset handling the given
/// access classes; returns the ruleset fd. Rules are added beneath it, then it is enforced.
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

/// `landlock_add_rule(ruleset_fd, LANDLOCK_RULE_PATH_BENEATH, &attr, 0)` — grant `allowed_access`
/// on everything beneath `attr.parent_fd` (an O_PATH fd to the allowed directory/file).
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

/// `landlock_restrict_self(ruleset_fd, 0)` — enforce the ruleset on the calling thread and all its
/// future children/execs. Irreversible. Requires no_new_privs first (unless CAP_SYS_ADMIN).
pub fn landlock_restrict_self(ruleset_fd: RawFd) -> io::Result<()> {
    let ret = unsafe { sc2(SYS_LANDLOCK_RESTRICT_SELF, ruleset_fd as i64, 0) };
    res(ret).map(|_| ())
}

/// `seccomp(SECCOMP_SET_MODE_FILTER, 0, &prog)` — install a classic-BPF syscall filter on the
/// calling thread (inherited across execve). Requires no_new_privs first. The program is the curated
/// deny-list built in proc_plane; it returns before any denied syscall executes.
pub fn seccomp_set_mode_filter(prog: &SockFprog) -> io::Result<()> {
    let ret = unsafe { sc3(SYS_SECCOMP, SECCOMP_SET_MODE_FILTER, 0, prog as *const SockFprog as i64) };
    res(ret).map(|_| ())
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
        // Landlock ABI structs. The packed path_beneath is the load-bearing one: 8+4 = 12, and it
        // MUST NOT round up to 16, or parent_fd lands at the wrong offset and every rule mis-scopes.
        assert_eq!(size_of::<LandlockRulesetAttr>(), 24, "3×u64");
        assert_eq!(size_of::<LandlockPathBeneathAttr>(), 12, "packed u64+s32 — no padding");
        assert_eq!(offset_of!(LandlockPathBeneathAttr, allowed_access), 0);
        assert_eq!(offset_of!(LandlockPathBeneathAttr, parent_fd), 8);
        // seccomp ABI structs.
        assert_eq!(size_of::<SockFilter>(), 8, "one classic-BPF insn");
        assert_eq!(offset_of!(SockFilter, k), 4);
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
