//! proc_plane — the T0 process-sandbox constructor: Landlock + seccomp + namespaces + cgroups v2.
//! Phase-5 slice-4 (docs/phase5-slice4-t0.md).
//!
//! Slice-2's decision plane already resolves three matrix cells to T0 — (T-first,C-ro-nosec),
//! (T-first,C-proj-rw), (T-pinned,C-ro-nosec) — but slice-3 *escalated* them to the T1 nspawn
//! constructor ("a T0 result at T1 is a legal upward escalation until the real T0 constructor
//! lands"). This module IS that constructor: those cells now build at genuine T0.
//!
//! Unlike T1, T0 has **no rootfs**. The workload runs against the host `/usr` (read-only, dm-verity
//! in the shipped image) and the granted paths *in place* — the containment is the LSM + syscall
//! filter, not a remapped mount tree. Filesystem caps are enforced by a Landlock ruleset that
//! DENIES everything and re-allows only {`/usr` exec+read, each pinned grant read}; ungranted paths
//! fail with EACCES (T0's deny) rather than T1's ENOENT (absent-from-mount). The grant is pinned by
//! the same TOCTOU-safe `openat2(RESOLVE_BENEATH|NO_SYMLINKS)` the mount plane uses, and the pinned
//! `O_PATH` fd — not any path string — is handed to Landlock as the rule's `parent_fd`.
//!
//! Construction is two forks (the ordering is forced by the kernel, see the comments in `construct`):
//!   gatekeeper (host root)  ── creates the cgroup + limits (needs host cgroupfs write)
//!     └─ P1 (host root)     ── joins the cgroup, THEN unshares user/mnt/pid/net/uts/ipc/cgroup ns
//!                              and writes the uid/gid maps (CLONE_NEWUSER moves the caller in at
//!                              once; the map needs the still-held host CAP_SETUID)
//!          └─ P2 (pid 1)    ── scrubs inherited fds, installs Landlock + no_new_privs + seccomp,
//!                              then execve — every wall live BEFORE the first workload instruction
//!
//! Fail-closed invariant: fall-up to T1 is decided ONLY at clean preflight (before any of this).
//! Once `construct` starts, ANY failure — cgroup, unshare, map, pin, ruleset, seccomp, exec — aborts
//! with no workload run and no weaker fallback. There is deliberately no path that degrades T0 to an
//! unconfined process (security-model.md §7).

use crate::linux_uapi::{
    self, close_range, fork, landlock_abi_version, landlock_add_path_beneath, landlock_create_ruleset,
    landlock_restrict_self, seccomp_set_mode_filter, set_no_new_privs, LandlockPathBeneathAttr,
    LandlockRulesetAttr, SockFilter, SockFprog, AUDIT_ARCH_X86_64, BPF_ABS, BPF_JEQ, BPF_JMP, BPF_K,
    BPF_LD, BPF_RET, BPF_W, CLONE_NEWCGROUP, CLONE_NEWIPC, CLONE_NEWNET, CLONE_NEWNS, CLONE_NEWPID,
    CLONE_NEWUSER, CLONE_NEWUTS, LANDLOCK_ACCESS_FS_EXECUTE, LANDLOCK_ACCESS_FS_IOCTL_DEV,
    LANDLOCK_ACCESS_FS_READ_DIR, LANDLOCK_ACCESS_FS_READ_FILE, LANDLOCK_ACCESS_FS_REFER,
    LANDLOCK_ACCESS_FS_TRUNCATE, LANDLOCK_ACCESS_FS_WRITE_FILE, SECCOMP_RET_ALLOW, SECCOMP_RET_ERRNO,
    SECCOMP_RET_KILL_PROCESS,
};
use crate::linux_uapi::{openat2, OpenHow, AT_FDCWD, O_CLOEXEC, O_PATH};
use crate::mount_plane::{open_anchor, pin_beneath, Pinned};
use std::ffi::CString;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

/// x32 syscalls carry this bit; the filter kills anything in that range so a denied syscall cannot
/// be smuggled through the x32 ABI (same arch value, different nr).
const X32_SYSCALL_BIT: u32 = 0x4000_0000;
const EPERM: u32 = 1;

/// Default cgroup-v2 bounds — a liveness/DoS ceiling, not a security wall. Overridable per spec.
pub const DEFAULT_MEM_MAX: u64 = 512 * 1024 * 1024;
pub const DEFAULT_PIDS_MAX: u64 = 256;

// -------------------------------------------------------------------------------------------------
// Landlock filesystem policy
// -------------------------------------------------------------------------------------------------

/// The set of filesystem access rights a ruleset should HANDLE (everything handled-but-unallowed is
/// denied), masked to what the probed ABI understands. Handling a right the kernel predates makes
/// `landlock_create_ruleset` return EINVAL, so we clamp: v1 = bits 0..=12, +REFER at v2, +TRUNCATE
/// at v3, +IOCTL_DEV at v5 (v4 added only network; v6+ added no new FS bits).
fn handled_fs_for_abi(abi: i64) -> u64 {
    // All v1 rights are the low 13 bits (EXECUTE..MAKE_SYM).
    let mut m: u64 = (1 << 13) - 1;
    if abi >= 2 {
        m |= LANDLOCK_ACCESS_FS_REFER;
    }
    if abi >= 3 {
        m |= LANDLOCK_ACCESS_FS_TRUNCATE;
    }
    if abi >= 5 {
        m |= LANDLOCK_ACCESS_FS_IOCTL_DEV;
    }
    m
}

/// Access we allow on the base runtime `/usr`: execute binaries + read files + list dirs. Masked to
/// `handled` so it is always a subset (Landlock rejects an `allowed` bit outside `handled`).
fn usr_access(handled: u64) -> u64 {
    (LANDLOCK_ACCESS_FS_EXECUTE | LANDLOCK_ACCESS_FS_READ_FILE | LANDLOCK_ACCESS_FS_READ_DIR) & handled
}

/// Access we allow on a granted path. Read-only in slice-4 (matching the mount plane, which is still
/// bind-ro — write realization is a later slice for BOTH tiers). A dir also gets READ_DIR to list.
fn grant_access(is_dir: bool, handled: u64) -> u64 {
    let mut a = LANDLOCK_ACCESS_FS_READ_FILE;
    if is_dir {
        a |= LANDLOCK_ACCESS_FS_READ_DIR;
    }
    a & handled
}

// -------------------------------------------------------------------------------------------------
// seccomp deny-list (curated escape/privilege syscalls). x86-64 numbers verified vs asm/unistd_64.h.
// Deliberately NOT clone/clone3: they are load-bearing for normal process/thread creation, and T0
// runs TRUSTED first-party code (defence-in-depth). Restricting CLONE_NEWUSER via seccomp argument
// inspection (so a workload cannot spawn a fresh userns to regain caps) is a documented hardening
// follow-up; because of that, the seccomp proof case is `mount`, never nested-userns.
// -------------------------------------------------------------------------------------------------
const SECCOMP_DENY: &[(&str, u32)] = &[
    ("mount", 165),
    ("umount2", 166),
    ("pivot_root", 155),
    ("chroot", 161),
    ("ptrace", 101),
    ("kexec_load", 246),
    ("kexec_file_load", 320),
    ("init_module", 175),
    ("finit_module", 313),
    ("delete_module", 176),
    ("bpf", 321),
    ("keyctl", 250),
    ("add_key", 248),
    ("request_key", 249),
    ("setns", 308),
    ("unshare", 272),
    ("perf_event_open", 298),
    ("process_vm_readv", 310),
    ("process_vm_writev", 311),
    ("open_tree", 428),
    ("move_mount", 429),
    ("fsopen", 430),
    ("reboot", 169),
    ("acct", 163),
    ("swapon", 167),
    ("settimeofday", 164),
    ("clock_settime", 227),
];

fn stmt(code: u16, k: u32) -> SockFilter {
    SockFilter { code, jt: 0, jf: 0, k }
}
fn jump(code: u16, k: u32, jt: u8, jf: u8) -> SockFilter {
    SockFilter { code, jt, jf, k }
}

/// Build the classic-BPF program: verify the arch is x86-64 (kill otherwise), kill x32-range nrs,
/// return EPERM for each denied syscall, else allow. Pure — unit-tested for shape.
fn build_seccomp_program() -> Vec<SockFilter> {
    let mut p = Vec::new();
    // A = arch; if arch != x86-64 -> KILL (blocks the i386/compat table entirely).
    p.push(stmt(BPF_LD | BPF_W | BPF_ABS, linux_uapi::SECCOMP_DATA_ARCH_OFFSET));
    p.push(jump(BPF_JMP | BPF_JEQ | BPF_K, AUDIT_ARCH_X86_64, 1, 0)); // ==: skip the kill
    p.push(stmt(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS));
    // A = nr; kill anything in the x32 range (nr >= X32_SYSCALL_BIT).
    p.push(stmt(BPF_LD | BPF_W | BPF_ABS, linux_uapi::SECCOMP_DATA_NR_OFFSET));
    // BPF_JGE (0x35 = BPF_JMP|BPF_JGE|BPF_K): if A >= bit -> KILL, else continue.
    p.push(jump(BPF_JMP | 0x30 | BPF_K, X32_SYSCALL_BIT, 0, 1)); // >=: fall to kill; else skip it
    p.push(stmt(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS));
    // Per denied syscall: if A == nr -> return EPERM; else skip the ret.
    for (_name, nr) in SECCOMP_DENY {
        p.push(jump(BPF_JMP | BPF_JEQ | BPF_K, *nr, 0, 1)); // ==: fall to ret; else skip
        p.push(stmt(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM));
    }
    // Default: allow.
    p.push(stmt(BPF_RET | BPF_K, SECCOMP_RET_ALLOW));
    p
}

// -------------------------------------------------------------------------------------------------
// Preflight — the ONE place fall-up-to-T1 is decided
// -------------------------------------------------------------------------------------------------

/// Result of the pre-construction capability probe. `Ready` carries the ABI so the policy can be
/// masked to it; `Unavailable` is the ONLY signal on which the caller may legally fall up to T1
/// (Landlock compiled out, disabled at boot, or an ABI too old to enforce). Anything that fails
/// *after* `construct` begins is a hard error, never a fall-up.
pub enum Preflight {
    Ready { abi: i64 },
    Unavailable(String),
}

/// Probe Landlock before committing to a T0 build. ABI < 1 cannot enforce a filesystem ruleset.
pub fn preflight() -> Preflight {
    match landlock_abi_version() {
        Ok(abi) if abi >= 1 => Preflight::Ready { abi },
        Ok(abi) => Preflight::Unavailable(format!("landlock-abi-{abi}-too-old")),
        Err(e) => Preflight::Unavailable(format!("landlock-unavailable ({e})")),
    }
}

// -------------------------------------------------------------------------------------------------
// Construction
// -------------------------------------------------------------------------------------------------

pub struct T0Spec {
    pub id: String,
    pub anchor: PathBuf,
    pub grants: Vec<String>,
    pub workload: Vec<String>,
    pub abi: i64,
    pub mem_max: u64,
    pub pids_max: u64,
}

/// A cgroup-v2 leaf that bounds the sandbox. Created by the (host-root) gatekeeper BEFORE any user
/// namespace exists — after `unshare(CLONE_NEWUSER)` the process can no longer write host cgroupfs.
struct CgroupScope {
    leaf: PathBuf,
}

fn cg_write(path: &std::path::Path, val: &str) -> io::Result<()> {
    let mut f = std::fs::OpenOptions::new().write(true).open(path)?;
    f.write_all(val.as_bytes())
}

impl CgroupScope {
    /// Base = the cgroup gatekeeper itself is in (from /proc/self/cgroup: unified `0::<path>`), which
    /// is a delegation root on the shipped image (gatekeeperd.service Delegate=yes) and the container
    /// root in the oracle — both may manage `subtree_control`. We move the daemon into a sibling leaf
    /// so `base` has no internal process, enable the controllers, then create the sandbox leaf.
    fn create(id: &str, mem_max: u64, pids_max: u64) -> io::Result<CgroupScope> {
        let rel = std::fs::read_to_string("/proc/self/cgroup")?
            .lines()
            .find_map(|l| l.strip_prefix("0::").map(|s| s.to_string()))
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "no unified cgroup for self"))?;
        let base = PathBuf::from("/sys/fs/cgroup").join(rel.trim_start_matches('/'));

        // Vacate `base`: move self to base/_daemon so base can enable controllers for its children.
        let daemon = base.join("_daemon");
        let _ = std::fs::create_dir(&daemon);
        cg_write(&daemon.join("cgroup.procs"), &format!("{}\n", std::process::id()))?;
        // Enable the controllers we bound. Ignore "already enabled"; a missing controller surfaces
        // later as the leaf's memory.max/pids.max simply not existing (a fail-closed write error).
        let _ = cg_write(&base.join("cgroup.subtree_control"), "+memory +pids");

        let leaf = base.join(format!("shrek-t0-{id}"));
        let _ = std::fs::remove_dir(&leaf);
        std::fs::create_dir(&leaf)?;
        cg_write(&leaf.join("memory.max"), &format!("{mem_max}\n"))?;
        cg_write(&leaf.join("pids.max"), &format!("{pids_max}\n"))?;
        Ok(CgroupScope { leaf })
    }

    /// Join the calling process (and, by fork inheritance, its children) to the leaf. Must run in the
    /// host user namespace — i.e. before `unshare(CLONE_NEWUSER)`.
    fn join_self(&self) -> io::Result<()> {
        cg_write(&self.leaf.join("cgroup.procs"), &format!("{}\n", std::process::id()))
    }

    fn destroy(&self) {
        let _ = std::fs::remove_dir(&self.leaf);
    }
}

/// Write the uid/gid maps for the fresh user namespace. A process that just created its own userns
/// via `unshare(CLONE_NEWUSER)` has DROPPED CAP_SETUID in the parent ns, so it may only write the
/// single self-map of its own id (mapping a range needs CAP_SETUID in the parent — the runc-style
/// "parent writes the child's map" handshake). slice-4 therefore maps just container-0 → the host
/// id P1 already holds (root): a namespace boundary, NOT yet a privilege drop. Full subuid-range
/// mapping (so container-root is an unprivileged host uid) is a documented hardening follow-up; the
/// real T0 wall is Landlock + seccomp. setgroups=deny is mandatory before writing gid_map here.
fn write_id_maps() -> io::Result<()> {
    std::fs::write("/proc/self/setgroups", b"deny")?;
    std::fs::write("/proc/self/gid_map", b"0 0 1")?;
    std::fs::write("/proc/self/uid_map", b"0 0 1")?;
    Ok(())
}

/// Minimal device nodes any real workload expects. These are ALLOW-widenings, not security grants:
/// if one is absent we simply skip it (a missing allowance only ever makes the sandbox stricter, so
/// it does not violate fail-closed). `/dev/null` in particular is needed for shell redirects.
const BASE_DEV_RW: &[&str] = &["/dev/null", "/dev/zero", "/dev/full"];
const BASE_DEV_RO: &[&str] = &["/dev/urandom", "/dev/random"];

/// Open a trusted absolute path as an O_PATH fd (non-directory ok) for use as a Landlock parent_fd.
fn open_opath(path: &str) -> io::Result<OwnedFd> {
    let c = CString::new(path).map_err(|_| io::Error::from_raw_os_error(22))?;
    let how = OpenHow { flags: O_PATH | O_CLOEXEC, resolve: 0, ..Default::default() };
    openat2(AT_FDCWD as RawFd, &c, &how)
}

/// Install the Landlock filesystem ruleset: deny-all, then re-allow `/usr` (exec+read), a minimal
/// `/dev` set, and each pinned grant (read). Runs in P2 after the fd scrub; the pinned grant fds are
/// re-derived here (in the sandbox's own ns) so nothing attacker-influenced is inherited across fork.
fn install_landlock(spec: &T0Spec) -> io::Result<()> {
    let handled = handled_fs_for_abi(spec.abi);
    let attr = LandlockRulesetAttr { handled_access_fs: handled, ..Default::default() };
    let ruleset = landlock_create_ruleset(&attr)?;
    let mut dev_fds: Vec<OwnedFd> = Vec::new(); // keep alive until enforced

    // Base runtime: /usr (exec+read). Opened O_PATH here, used as the rule's parent_fd.
    let usr = open_anchor(std::path::Path::new("/usr"))?;
    let usr_rule = LandlockPathBeneathAttr { allowed_access: usr_access(handled), parent_fd: usr.as_raw_fd() };
    landlock_add_path_beneath(ruleset.as_raw_fd(), &usr_rule)?;

    // Minimal /dev — best-effort (skip absent nodes; a missing allowance only tightens the sandbox).
    let read = LANDLOCK_ACCESS_FS_READ_FILE & handled;
    let rw = (LANDLOCK_ACCESS_FS_READ_FILE | LANDLOCK_ACCESS_FS_WRITE_FILE) & handled;
    for (paths, access) in [(BASE_DEV_RW, rw), (BASE_DEV_RO, read)] {
        for p in paths {
            match open_opath(p) {
                Ok(fd) => {
                    let rule = LandlockPathBeneathAttr { allowed_access: access, parent_fd: fd.as_raw_fd() };
                    landlock_add_path_beneath(ruleset.as_raw_fd(), &rule)?;
                    dev_fds.push(fd);
                }
                Err(_) => eprintln!("proc_plane/P2: base dev {p} absent — skipped"),
            }
        }
    }

    // Each grant, pinned TOCTOU-safe beneath the anchor, allowed read-only.
    let anchor = open_anchor(&spec.anchor)?;
    let mut pins: Vec<Pinned> = Vec::new();
    for name in &spec.grants {
        let p = pin_beneath(&anchor, name)?;
        let rule = LandlockPathBeneathAttr {
            allowed_access: grant_access(p.is_dir, handled),
            parent_fd: p.fd.as_raw_fd(),
        };
        landlock_add_path_beneath(ruleset.as_raw_fd(), &rule)?;
        pins.push(p); // keep fds alive until the ruleset is enforced
    }

    // no_new_privs is required before restrict_self without CAP_SYS_ADMIN and is a defence in depth
    // regardless; then enforce. The O_PATH fds (all O_CLOEXEC) vanish at execve.
    set_no_new_privs()?;
    landlock_restrict_self(ruleset.as_raw_fd())?;
    drop(pins);
    drop(dev_fds);
    drop(anchor);
    drop(usr);
    drop(ruleset);
    Ok(())
}

/// Install the seccomp deny-list on the calling thread (after no_new_privs, before execve).
fn install_seccomp() -> io::Result<()> {
    let prog = build_seccomp_program();
    let fprog = SockFprog { len: prog.len() as u16, filter: prog.as_ptr() };
    seccomp_set_mode_filter(&fprog)
}

/// The pid-1 body (P2): fail-closed all the way; on ANY error it prints and _exit(126/127) so the
/// parent sees a non-zero code and NEVER a half-sandboxed workload.
fn sandbox_init_and_exec(spec: &T0Spec) -> ! {
    // (1) FD scrub — mandatory rules-before-usable invariant. Landlock only governs opens AFTER
    // restrict_self; an fd inherited from the privileged parent (esp. an O_PATH dir fd) is exempt
    // and would be an escape hatch. Close everything above stderr before opening anything.
    if let Err(e) = close_range(3, u32::MAX, 0) {
        // No close_range (pre-5.9): manual fallback over /proc/self/fd.
        eprintln!("proc_plane/P2: close_range failed ({e}); manual fd scrub");
        if let Ok(rd) = std::fs::read_dir("/proc/self/fd") {
            for ent in rd.flatten() {
                if let Some(n) = ent.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) {
                    if n > 2 {
                        unsafe { libc_close(n) };
                    }
                }
            }
        }
    }

    // (2) Landlock, then (3) seccomp — walls live before the first workload instruction.
    if let Err(e) = install_landlock(spec) {
        eprintln!("proc_plane/P2: FAIL landlock: {e} — failed closed (no exec)");
        std::process::exit(126);
    }
    if let Err(e) = install_seccomp() {
        eprintln!("proc_plane/P2: FAIL seccomp: {e} — failed closed (no exec)");
        std::process::exit(126);
    }

    // (4) execve in place (absolute path — no PATH search, which Landlock would deny anyway).
    let err = Command::new(&spec.workload[0]).args(&spec.workload[1..]).exec();
    eprintln!("proc_plane/P2: FAIL exec {}: {err} — failed closed", spec.workload[0]);
    std::process::exit(127);
}

/// One raw close(2) for the manual fd-scrub fallback (avoids a libc dep).
unsafe fn libc_close(fd: i32) {
    core::arch::asm!("syscall", in("rax") 3i64, in("rdi") fd as i64,
        lateout("rax") _, lateout("rcx") _, lateout("r11") _, options(nostack));
}

/// Construct and run one T0 sandbox. Returns the workload's exit code. MUST be called single-threaded
/// and as host root (needs cgroupfs write + full unshare). Caller decides fall-up at preflight; this
/// function only ever fails closed.
pub fn construct(spec: &T0Spec) -> io::Result<i32> {
    if spec.workload.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty workload"));
    }
    let cg = CgroupScope::create(&spec.id, spec.mem_max, spec.pids_max)?;

    // P1: the outer helper. Joins the cgroup while still host-root, then unshares. CLONE_NEWPID does
    // not move P1 itself into the new pid ns — only its child (P2) becomes pid 1 there.
    let p1 = unsafe { fork()? };
    if p1 == 0 {
        let rc = (|| -> io::Result<i32> {
            let step = |label: &str, r: io::Result<()>| -> io::Result<()> {
                r.map_err(|e| io::Error::new(e.kind(), format!("{label}: {e}")))
            };
            step("cgroup-join", cg.join_self())?;
            step(
                "unshare",
                linux_uapi::unshare(
                    CLONE_NEWUSER | CLONE_NEWNS | CLONE_NEWPID | CLONE_NEWNET | CLONE_NEWUTS
                        | CLONE_NEWIPC | CLONE_NEWCGROUP,
                ),
            )?;
            step("id-maps", write_id_maps())?;
            // Contain any future mounts to this ns (belt-and-suspenders; T0 mounts nothing today).
            step(
                "mount-private",
                linux_uapi::mount(c"none", c"/", None, linux_uapi::MS_REC | linux_uapi::MS_PRIVATE, None),
            )?;

            // P2: pid 1 of the new pid ns. It installs the walls and execs (or _exits non-zero).
            let p2 = unsafe { fork()? };
            if p2 == 0 {
                sandbox_init_and_exec(spec); // never returns
            }
            let code = wait_code(p2)?;
            Ok(code)
        })();
        match rc {
            Ok(code) => std::process::exit(code),
            Err(e) => {
                eprintln!("proc_plane/P1: FAIL setup: {e} — failed closed");
                std::process::exit(125);
            }
        }
    }

    // gatekeeper: wait for P1, tear the cgroup down. Fail-closed teardown regardless of outcome.
    let code = wait_code(p1);
    cg.destroy();
    code
}

/// waitpid(pid) → exit code (or 128+signal), dep-free via the wait4 syscall.
fn wait_code(pid: i64) -> io::Result<i32> {
    const SYS_WAIT4: i64 = 61;
    let mut status: i32 = 0;
    let ret = unsafe {
        let r: i64;
        core::arch::asm!("syscall",
            inlateout("rax") SYS_WAIT4 => r,
            in("rdi") pid, in("rsi") &mut status as *mut i32 as i64, in("rdx") 0i64, in("r10") 0i64,
            lateout("rcx") _, lateout("r11") _, options(nostack));
        r
    };
    if ret < 0 {
        return Err(io::Error::from_raw_os_error(-ret as i32));
    }
    // WIFEXITED ? WEXITSTATUS : 128 + WTERMSIG
    if status & 0x7f == 0 {
        Ok((status >> 8) & 0xff)
    } else {
        Ok(128 + (status & 0x7f))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux_uapi::{
        LANDLOCK_ACCESS_FS_MAKE_DIR, LANDLOCK_ACCESS_FS_MAKE_REG, LANDLOCK_ACCESS_FS_REMOVE_DIR,
        LANDLOCK_ACCESS_FS_REMOVE_FILE,
    };

    #[test]
    fn abi_mask_grows_monotonically_and_never_exceeds_known_bits() {
        let v1 = handled_fs_for_abi(1);
        let v2 = handled_fs_for_abi(2);
        let v3 = handled_fs_for_abi(3);
        let v5 = handled_fs_for_abi(5);
        assert_eq!(v1, (1 << 13) - 1, "v1 = low 13 FS rights");
        assert_eq!(v2, v1 | LANDLOCK_ACCESS_FS_REFER);
        assert_eq!(v3, v2 | LANDLOCK_ACCESS_FS_TRUNCATE);
        assert_eq!(v5, v3 | LANDLOCK_ACCESS_FS_IOCTL_DEV);
        // v4 added only network — no new FS bit over v3.
        assert_eq!(handled_fs_for_abi(4), v3);
        // Every ABI must handle at least read+exec so those can be selectively re-allowed.
        for abi in 1..=8 {
            let h = handled_fs_for_abi(abi);
            assert_eq!(usr_access(h), LANDLOCK_ACCESS_FS_EXECUTE | LANDLOCK_ACCESS_FS_READ_FILE | LANDLOCK_ACCESS_FS_READ_DIR);
            assert!(grant_access(false, h) & LANDLOCK_ACCESS_FS_READ_FILE != 0);
            assert!(grant_access(true, h) & LANDLOCK_ACCESS_FS_READ_DIR != 0);
        }
    }

    #[test]
    fn grant_access_is_read_only_no_write_bits() {
        let h = handled_fs_for_abi(8);
        let dir = grant_access(true, h);
        let file = grant_access(false, h);
        // Never grant write/create/remove/exec on a granted path in slice-4.
        let write_ish = LANDLOCK_ACCESS_FS_WRITE_FILE
            | LANDLOCK_ACCESS_FS_MAKE_REG
            | LANDLOCK_ACCESS_FS_MAKE_DIR
            | LANDLOCK_ACCESS_FS_REMOVE_FILE
            | LANDLOCK_ACCESS_FS_REMOVE_DIR
            | LANDLOCK_ACCESS_FS_EXECUTE;
        assert_eq!(dir & write_ish, 0);
        assert_eq!(file & write_ish, 0);
    }

    #[test]
    fn allowed_is_always_subset_of_handled() {
        // Landlock EINVALs if a rule allows a bit the ruleset does not handle. Guard every ABI.
        for abi in 1..=8 {
            let h = handled_fs_for_abi(abi);
            assert_eq!(usr_access(h) & !h, 0);
            assert_eq!(grant_access(true, h) & !h, 0);
            assert_eq!(grant_access(false, h) & !h, 0);
        }
    }

    #[test]
    fn seccomp_program_shape_is_wellformed() {
        let p = build_seccomp_program();
        // arch-load, arch-check, arch-kill, nr-load, x32-check, x32-kill, then 2 insns per deny, +1 allow.
        assert_eq!(p.len(), 6 + SECCOMP_DENY.len() * 2 + 1);
        // First insn loads the arch word.
        assert_eq!(p[0].code, BPF_LD | BPF_W | BPF_ABS);
        assert_eq!(p[0].k, super::linux_uapi::SECCOMP_DATA_ARCH_OFFSET);
        // Last insn is the default-allow.
        let last = p.last().unwrap();
        assert_eq!(last.code, BPF_RET | BPF_K);
        assert_eq!(last.k, SECCOMP_RET_ALLOW);
        // mount(165) is in the deny-set (the seccomp proof case).
        assert!(SECCOMP_DENY.iter().any(|(n, nr)| *n == "mount" && *nr == 165));
        // clone/clone3 are deliberately NOT denied (would break process/thread creation).
        assert!(!SECCOMP_DENY.iter().any(|(_, nr)| *nr == 56 || *nr == 435));
    }
}
