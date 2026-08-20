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
use crate::linux_uapi::path_is_noexec;
use crate::mount_plane::{
    mask_with_empty, open_anchor, pin_beneath, relocate_exec_island, relocate_member,
    seal_noexec_in_place, seal_subtree_noexec, seal_subtree_noexec_writable, Pinned,
};
use std::ffi::CString;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
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
    /// slice-9 exec island: `Some` ⇒ this is a `T-pinned` static-PIE build. The fd is `der.exec_fd`
    /// (the `O_RDONLY` fd measured during derivation, bound to the exact pinned inode). When set, the
    /// constructor re-binds every mutable grant `MS_NOEXEC` and gives the pinned inode alone a
    /// re-verified exec-capable island + a single-inode Landlock EXECUTE rule; the workload execs the
    /// island, not the source fd (docs/phase5-slice9-pin-exec-home.md). `None` ⇒ ordinary T0 (no exec
    /// home for third-party bytes; grants Landlock-read only).
    pub exec_island: Option<OwnedFd>,
    /// slice-10 sealed-dynamic: `Some` ⇒ the pinned entrypoint is dynamically linked and this carries
    /// its authenticated closure (interpreter + transitive `DT_NEEDED`, each identity-pinned). When set
    /// (alongside `exec_island`, the entry fd), the constructor builds an **N-inode closure island** —
    /// the entry, the interpreter at its `PT_INTERP` path, and each library under the island lib dir are
    /// exec-capable re-verified binds, while `/usr` and every mutable grant are forced `MS_NOEXEC`.
    /// `None` ⇒ slice-9 single-inode static-PIE island (`exec_island` set, `closure` unset).
    pub closure: Option<ClosureSpec>,
}

/// One authenticated member of a sealed-dynamic closure, as the constructor needs it (slice-10).
pub struct ClosureMemberSpec {
    /// fs-verity algorithm id + digest — the identity authority, re-measured at bind time (I10).
    pub algo_id: u16,
    pub digest: Vec<u8>,
    /// The interpreter's absolute `PT_INTERP` path (`is_interp`), or a library's bare SONAME.
    pub name: String,
    pub is_interp: bool,
}

/// The authenticated closure the constructor binds (slice-10). Interpreter is bound at its absolute
/// `PT_INTERP` pathname (shadowing the host loader path); libraries are bound under the island lib dir
/// (`$ORIGIN/lib`, resolved by the pinned loader's baked RUNPATH). Sources are the same-basename files
/// in the entrypoint's own directory.
pub struct ClosureSpec {
    pub interp: ClosureMemberSpec,
    pub libs: Vec<ClosureMemberSpec>,
}

/// The broker-owned exec-island path for a request: the single re-verified inode the pinned
/// entrypoint is bound to and the ONLY non-`noexec` third-party surface in the sandbox.
fn island_path(id: &str) -> PathBuf {
    PathBuf::from(format!("/run/shrek/{id}/exec/entry"))
}

/// The island library directory for a request — `$ORIGIN/lib` relative to the entry island
/// (`/run/shrek/<id>/exec/entry`), where the pinned loader (baked RUNPATH `$ORIGIN/lib`) resolves each
/// `DT_NEEDED`. Each pinned library is bound at `island_lib_path(id)/<soname>` (slice-10).
fn island_lib_path(id: &str, soname: &str) -> PathBuf {
    PathBuf::from(format!("/run/shrek/{id}/exec/lib/{soname}"))
}

/// The writable exec-island ROOT for a request (`/run/shrek/<id>/exec`) — parent of both the entry
/// island and the lib dir. F2 (docs/phase5-consolidation.md §2): this subtree is sealed `MS_NOEXEC`
/// (writable) BEFORE any member bind, so the only exec-capable surface under it is the set of
/// re-verified member binds laid on top.
fn island_root(id: &str) -> PathBuf {
    PathBuf::from(format!("/run/shrek/{id}/exec"))
}

/// F2 self-check + proof: after the island is built, assert the writable island ROOT is `MS_NOEXEC`
/// while the entrypoint island and each re-verified member bind laid on top are **independently
/// exec-capable** (their own mount lacks `noexec`). Runs in P1 (root, pre-Landlock, `/proc`+`statfs`
/// available). Fail-closed if the parent seal did not take (would open a laundering surface) or a
/// member wrongly inherited `noexec` (would break the pinned workload). The emitted audit line is what
/// the host oracle / sealed-VM gate grep to prove the mount-flag independence directly. `member_libs`
/// is the SONAME list (empty for a static-PIE island — entry only).
fn verify_island_exec_flags(id: &str, member_libs: &[String]) -> io::Result<()> {
    let check = |p: &Path, want_noexec: bool, what: &str| -> io::Result<()> {
        let c = CString::new(p.to_string_lossy().as_bytes())
            .map_err(|_| io::Error::from_raw_os_error(22))?;
        let got = path_is_noexec(&c).map_err(|e| {
            io::Error::new(e.kind(), format!("island-flags statfs {} ({what}): {e}", p.display()))
        })?;
        if got != want_noexec {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("island-flags: {what} {} noexec={got} want={want_noexec}", p.display()),
            ));
        }
        Ok(())
    };
    // Parent island root MUST be noexec; entry + each member bind MUST be exec-capable (not noexec).
    check(&island_root(id), true, "island-root")?;
    check(&island_path(id), false, "island-entry")?;
    for so in member_libs {
        check(&island_lib_path(id, so), false, "island-lib")?;
    }
    eprintln!(
        "SANDBOX-ISLAND-FLAGS parent-noexec=1 members-exec-ok=1 entry=1 libs={}",
        member_libs.len()
    );
    Ok(())
}

/// Reject anything that is not a static (loader-free) ELF64 — Fork A, slice-9. A `PT_INTERP` program
/// header means the binary asks a dynamic loader to pull in shared libraries, an exec/library closure
/// v1 deliberately does not authenticate. We parse the ELF header + program headers directly (dep-free)
/// and fail closed on a bad magic, a non-ELF64 class, ANY `PT_INTERP`, or any read/parse error.
fn reject_if_dynamic(path: &Path) -> io::Result<()> {
    const PT_INTERP: u32 = 3;
    let fail = |m: &str| io::Error::new(io::ErrorKind::Other, format!("not a static ELF64: {m}"));

    let mut f = std::fs::File::open(path)?;
    let mut eh = [0u8; 64];
    f.read_exact(&mut eh)?;
    if &eh[0..4] != b"\x7fELF" {
        return Err(fail("bad magic"));
    }
    if eh[4] != 2 {
        return Err(fail("not ELFCLASS64"));
    }
    if eh[5] != 1 {
        return Err(fail("not little-endian"));
    }
    let e_phoff = u64::from_le_bytes(eh[32..40].try_into().unwrap());
    let e_phentsize = u16::from_le_bytes(eh[54..56].try_into().unwrap()) as usize;
    let e_phnum = u16::from_le_bytes(eh[56..58].try_into().unwrap()) as usize;
    if e_phnum == 0 || e_phentsize < 4 {
        return Err(fail("no program headers"));
    }
    // Cap the header table we read (defends a hostile e_phnum) — 4096 phdrs is far beyond any real ELF.
    if e_phnum > 4096 {
        return Err(fail("implausible program-header count"));
    }
    f.seek(SeekFrom::Start(e_phoff))?;
    let mut ph = vec![0u8; e_phentsize];
    for _ in 0..e_phnum {
        f.read_exact(&mut ph)?;
        let p_type = u32::from_le_bytes(ph[0..4].try_into().unwrap());
        if p_type == PT_INTERP {
            return Err(fail("PT_INTERP present (dynamically linked)"));
        }
    }
    Ok(())
}

/// slice-10 — read a dynamic ELF64's `PT_INTERP` string and its direct `DT_NEEDED` SONAMEs, dep-free
/// and fail-closed. Used by [`authenticate_closure`] to prove the entrypoint's loader inputs are all
/// pinned closure members. `interp` is `None` for a static (loader-free) binary. Bounds every field so
/// a hostile header cannot drive an unbounded read.
struct DynInfo {
    interp: Option<String>,
    needed: Vec<String>,
}

fn parse_interp_and_needed(path: &Path) -> io::Result<DynInfo> {
    const PT_LOAD: u32 = 1;
    const PT_DYNAMIC: u32 = 2;
    const PT_INTERP: u32 = 3;
    const DT_NULL: i64 = 0;
    const DT_NEEDED: i64 = 1;
    const DT_STRTAB: i64 = 5;
    const DT_STRSZ: i64 = 10;
    let fail = |m: &str| io::Error::new(io::ErrorKind::Other, format!("closure ELF parse: {m}"));
    let u16le = |b: &[u8]| u16::from_le_bytes(b.try_into().unwrap());
    let u32le = |b: &[u8]| u32::from_le_bytes(b.try_into().unwrap());
    let u64le = |b: &[u8]| u64::from_le_bytes(b.try_into().unwrap());
    let i64le = |b: &[u8]| i64::from_le_bytes(b.try_into().unwrap());

    let mut f = std::fs::File::open(path)?;
    let mut eh = [0u8; 64];
    f.read_exact(&mut eh)?;
    if &eh[0..4] != b"\x7fELF" {
        return Err(fail("bad magic"));
    }
    if eh[4] != 2 || eh[5] != 1 {
        return Err(fail("not little-endian ELFCLASS64"));
    }
    let e_phoff = u64le(&eh[32..40]);
    let e_phentsize = u16le(&eh[54..56]) as usize;
    let e_phnum = u16le(&eh[56..58]) as usize;
    if e_phnum == 0 || e_phentsize < 56 {
        return Err(fail("no/short program headers"));
    }
    if e_phnum > 4096 {
        return Err(fail("implausible program-header count"));
    }

    f.seek(SeekFrom::Start(e_phoff))?;
    let mut ph = vec![0u8; e_phentsize];
    let mut interp_seg: Option<(u64, u64)> = None; // (offset, filesz)
    let mut dyn_seg: Option<(u64, u64)> = None;
    let mut loads: Vec<(u64, u64, u64)> = Vec::new(); // (vaddr, filesz, offset)
    for _ in 0..e_phnum {
        f.read_exact(&mut ph)?;
        let p_type = u32le(&ph[0..4]);
        let p_offset = u64le(&ph[8..16]);
        let p_vaddr = u64le(&ph[16..24]);
        let p_filesz = u64le(&ph[32..40]);
        match p_type {
            PT_INTERP => interp_seg = Some((p_offset, p_filesz)),
            PT_DYNAMIC => dyn_seg = Some((p_offset, p_filesz)),
            PT_LOAD => loads.push((p_vaddr, p_filesz, p_offset)),
            _ => {}
        }
    }

    let interp = match interp_seg {
        Some((off, sz)) => {
            if sz == 0 || sz > 4096 {
                return Err(fail("implausible PT_INTERP size"));
            }
            f.seek(SeekFrom::Start(off))?;
            let mut buf = vec![0u8; sz as usize];
            f.read_exact(&mut buf)?;
            let s = buf.split(|&b| b == 0).next().unwrap_or(&[]);
            Some(String::from_utf8_lossy(s).into_owned())
        }
        None => None,
    };

    let mut needed: Vec<String> = Vec::new();
    if let Some((doff, dsz)) = dyn_seg {
        if dsz > (1 << 20) {
            return Err(fail("implausible PT_DYNAMIC size"));
        }
        let n = (dsz / 16) as usize;
        f.seek(SeekFrom::Start(doff))?;
        let mut db = vec![0u8; n * 16];
        f.read_exact(&mut db)?;
        let mut needed_offsets: Vec<u64> = Vec::new();
        let mut strtab_vaddr: Option<u64> = None;
        let mut strsz: Option<u64> = None;
        for i in 0..n {
            let tag = i64le(&db[i * 16..i * 16 + 8]);
            let val = u64le(&db[i * 16 + 8..i * 16 + 16]);
            match tag {
                DT_NULL => break,
                DT_NEEDED => {
                    if needed_offsets.len() >= 4096 {
                        return Err(fail("implausible DT_NEEDED count"));
                    }
                    needed_offsets.push(val);
                }
                DT_STRTAB => strtab_vaddr = Some(val),
                DT_STRSZ => strsz = Some(val),
                _ => {}
            }
        }
        if !needed_offsets.is_empty() {
            let sv = strtab_vaddr.ok_or_else(|| fail("DT_NEEDED without DT_STRTAB"))?;
            // Map the strtab virtual address to a file offset via the PT_LOAD segment that covers it.
            let str_off = loads
                .iter()
                .find_map(|&(va, fsz, off)| if sv >= va && sv < va.saturating_add(fsz) { Some(off + (sv - va)) } else { None })
                .ok_or_else(|| fail("DT_STRTAB vaddr not in any PT_LOAD"))?;
            let cap: u64 = strsz.filter(|&s| s > 0 && s <= (1 << 20)).unwrap_or(1 << 16);
            f.seek(SeekFrom::Start(str_off))?;
            let mut sbuf = vec![0u8; cap as usize];
            // The strtab may run to EOF short of `cap`; read what is there (fail only on a hard error).
            let mut got = 0usize;
            loop {
                match f.read(&mut sbuf[got..]) {
                    Ok(0) => break,
                    Ok(k) => {
                        got += k;
                        if got == sbuf.len() {
                            break;
                        }
                    }
                    Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e),
                }
            }
            sbuf.truncate(got);
            for no in needed_offsets {
                let start = no as usize;
                if start >= sbuf.len() {
                    return Err(fail("DT_NEEDED offset past strtab"));
                }
                let s = sbuf[start..].split(|&b| b == 0).next().unwrap_or(&[]);
                if s.is_empty() {
                    return Err(fail("empty DT_NEEDED string"));
                }
                needed.push(String::from_utf8_lossy(s).into_owned());
            }
        }
    }
    Ok(DynInfo { interp, needed })
}

/// slice-10 — authenticate the sealed-dynamic closure of the re-verified island entrypoint. Replaces
/// `reject_if_dynamic` for the dynamic route: instead of rejecting any `PT_INTERP`, require that the
/// entrypoint's loader inputs are ALL pinned closure members —
///   (1) it IS dynamic (has a `PT_INTERP`) — a static binary belongs on the slice-9 route;
///   (2) its `PT_INTERP` equals the closure's pinned interpreter path (so the kernel runs the pinned
///       `ld.so` we bound there, never another loader);
///   (3) every direct `DT_NEEDED` SONAME is one of the pinned closure libraries.
/// Transitive completeness is build-enumerated; the runtime security teeth are that every non-member
/// mount is `MS_NOEXEC` (so an unlisted dep cannot be mapped as code — it simply fails to load,
/// fail-closed). Fail-closed on any parse error or mismatch.
fn authenticate_closure(island_entry: &Path, closure: &ClosureSpec) -> io::Result<()> {
    let fail = |m: String| io::Error::new(io::ErrorKind::Other, m);
    let info = parse_interp_and_needed(island_entry)?;
    let interp = info
        .interp
        .ok_or_else(|| fail("closure entrypoint has no PT_INTERP (not dynamically linked)".to_string()))?;
    if interp != closure.interp.name {
        return Err(fail(format!("entrypoint PT_INTERP {interp:?} != pinned interpreter {:?}", closure.interp.name)));
    }
    for need in &info.needed {
        if !closure.libs.iter().any(|l| &l.name == need) {
            return Err(fail(format!("DT_NEEDED {need:?} is not a pinned closure member")));
        }
    }
    Ok(())
}

/// slice-10 — construct the **N-inode closure island** for a `T-pinned` dynamically-linked entrypoint,
/// in the per-request child's private mount ns. Fail-closed throughout. Runs in P1 after the unshare +
/// mount-private, before the P2 fork (mounts survive the fd scrub; the inherited `exec_fd` is never
/// handed to the workload). The exec-capable surface is exactly {entry, interpreter, each pinned lib};
/// `/usr` and every mutable grant are forced `MS_NOEXEC` so no non-member byte can be mapped as code.
fn build_closure_island(spec: &T0Spec, exec_fd: RawFd, closure: &ClosureSpec) -> io::Result<()> {
    let ctx = |label: &str, r: io::Result<()>| -> io::Result<()> {
        r.map_err(|e| io::Error::new(e.kind(), format!("{label}: {e}")))
    };
    // 1. Re-assert MS_NOEXEC on every mutable grant, TOCTOU-safe (same as the static island).
    let anchor = open_anchor(&spec.anchor).map_err(|e| io::Error::new(e.kind(), format!("open-anchor: {e}")))?;
    for name in &spec.grants {
        let p = pin_beneath(&anchor, name).map_err(|e| io::Error::new(e.kind(), format!("pin-grant {name}: {e}")))?;
        ctx(&format!("seal-grant {name}"), seal_noexec_in_place(&p, &spec.anchor.join(name)))?;
    }
    // 2. Force /usr MS_NOEXEC in this ns — the executable-mapping boundary (Landlock gates execve, not
    //    mmap(PROT_EXEC), so a non-member /usr .so must not be loadable). The sealed image is merged-usr
    //    (/lib,/lib64 → /usr), so /usr covers the default library search; the oracle/VM gate verify a
    //    /usr .so cannot be dlopen/mmap-loaded. Member binds are laid on TOP of this afterwards.
    ctx("seal-usr-noexec", seal_subtree_noexec(Path::new("/usr")))?;
    // 2b. Mask loader config so no preload/cache input can steer resolution outside the closure (I8/I9).
    ctx("mask-ld-preload", mask_with_empty(Path::new("/etc/ld.so.preload")))?;
    ctx("mask-ld-cache", mask_with_empty(Path::new("/etc/ld.so.cache")))?;
    // 2c. F2 (docs/phase5-consolidation.md §2): seal the WRITABLE exec-island root `/run/shrek/<id>/exec`
    //     `MS_NOEXEC` BEFORE any entry/member bind, so the island directory itself is not an
    //     executable-mapping surface. This is an `MS_NOEXEC` barrier independent of — and co-load-bearing
    //     with — Landlock's `MAKE_REG`/`WRITE` deny-all: even if a byte reached the island dir, it could
    //     not be `mmap(PROT_EXEC)`-loaded. Fresh member binds placed on top (steps 3–4) each re-add exec
    //     for exactly their one re-verified inode (same seal-then-reopen-per-inode pattern as `/usr`).
    let iroot = island_root(&spec.id);
    ctx("mkdir-island-lib", std::fs::create_dir_all(iroot.join("lib")))?;
    ctx("seal-island-root", seal_subtree_noexec_writable(&iroot))?;
    // 3. The entrypoint exec island (reuse the slice-9 machinery unchanged) — exec-capable, re-verified
    //    (dev,ino)+fs-verity against der.exec_fd.
    ctx("relocate-island", relocate_exec_island(Path::new(&spec.workload[0]), exec_fd, &island_path(&spec.id)))?;
    // 4. The pinned members. Sources are the same-basename files in the entrypoint's own directory.
    let entry_dir = Path::new(&spec.workload[0])
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "entrypoint has no parent dir"))?;
    fn base(name: &str) -> &str {
        Path::new(name).file_name().and_then(|s| s.to_str()).unwrap_or(name)
    }
    // 4a. Interpreter — bound at its absolute PT_INTERP path (shadowing the host loader).
    let interp = &closure.interp;
    ctx(
        "relocate-interp",
        relocate_member(&entry_dir.join(base(&interp.name)), interp.algo_id, &interp.digest, Path::new(&interp.name)),
    )?;
    // 4b. Each library — bound under the island lib dir ($ORIGIN/lib), keyed by SONAME.
    for lib in &closure.libs {
        ctx(
            &format!("relocate-lib {}", lib.name),
            relocate_member(&entry_dir.join(&lib.name), lib.algo_id, &lib.digest, &island_lib_path(&spec.id, &lib.name)),
        )?;
    }
    // 5. Authenticate the entrypoint's loader inputs against the pinned closure (PT_INTERP + DT_NEEDED).
    ctx("authenticate-closure", authenticate_closure(&island_path(&spec.id), closure))?;
    // 6. F2 self-check + proof: parent island root is `noexec`; the entry island and every member bind
    //    are independently exec-capable (their own mount lacks `noexec`). Fail-closed on any mismatch.
    let libs: Vec<String> = closure.libs.iter().map(|l| l.name.clone()).collect();
    ctx("verify-island-flags", verify_island_exec_flags(&spec.id, &libs))?;
    Ok(())
}

/// Construct the exec island in the per-request child's private mount ns (slice-9). Fail-closed
/// throughout: on ANY error the caller aborts the build (no workload runs). Runs in P1 AFTER the
/// unshare + mount-private, BEFORE the P2 fork — the results are mounts (path-based) that survive P2's
/// fd scrub; the source `exec_fd` is inherited here and is never handed to the workload.
fn build_exec_island(spec: &T0Spec, exec_fd: RawFd) -> io::Result<()> {
    // 1. Re-assert MS_NOEXEC on every mutable grant, in place + TOCTOU-safe. This — not Landlock — is
    //    what stops the pinned binary from executing or mmap(PROT_EXEC)-loading mutable bytes (I2/I5).
    let ctx = |label: &str, r: io::Result<()>| -> io::Result<()> {
        r.map_err(|e| io::Error::new(e.kind(), format!("{label}: {e}")))
    };
    let anchor = open_anchor(&spec.anchor).map_err(|e| io::Error::new(e.kind(), format!("open-anchor: {e}")))?;
    for name in &spec.grants {
        let p = pin_beneath(&anchor, name).map_err(|e| io::Error::new(e.kind(), format!("pin-grant {name}: {e}")))?;
        ctx(&format!("seal-grant {name}"), seal_noexec_in_place(&p, &spec.anchor.join(name)))?;
    }
    // 1b. F2 (docs/phase5-consolidation.md §2): seal the writable exec-island root `MS_NOEXEC` before the
    //     entry bind, so the island directory is not an executable-mapping surface (independent of
    //     Landlock's deny-all). The static path has no lib dir — entry is the only member.
    let iroot = island_root(&spec.id);
    ctx("mkdir-island-root", std::fs::create_dir_all(&iroot))?;
    ctx("seal-island-root", seal_subtree_noexec_writable(&iroot))?;
    // 2. Bind the pinned entrypoint inode onto its island, re-verify (dev,ino)+fs-verity digest, and
    //    harden RO|NOSUID|NODEV WITHOUT NOEXEC (the one deliberate exception, for this one inode). The
    //    entrypoint is re-opened by path IN THIS ns (bind sources must be ns-local) and re-verified
    //    against the derived exec_fd (the identity+digest authority) — see relocate_exec_island.
    ctx("relocate-island", relocate_exec_island(Path::new(&spec.workload[0]), exec_fd, &island_path(&spec.id)))?;
    // 3. Static-PIE only (Fork A): reject a PT_INTERP entrypoint before it can run.
    ctx("static-pie-check", reject_if_dynamic(&island_path(&spec.id)))?;
    // 4. F2 self-check + proof: parent island root is `noexec`; the entry bind is independently
    //    exec-capable. Fail-closed on any mismatch.
    ctx("verify-island-flags", verify_island_exec_flags(&spec.id, &[]))?;
    Ok(())
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

    // Base runtime: /usr. Opened O_PATH here, used as the rule's parent_fd. Ordinary T0 gets exec+read
    // (first-party workload runs from /usr). The slice-9 exec-island (static-PIE, Fork A) gets READ
    // ONLY — the pinned binary links nothing, so /usr is NOT an exec surface; EXECUTE is scoped to the
    // island inode alone (added below). This keeps the reopened exec surface a single frozen inode.
    let usr = open_anchor(std::path::Path::new("/usr"))?;
    let usr_allow = if spec.exec_island.is_some() {
        (LANDLOCK_ACCESS_FS_READ_FILE | LANDLOCK_ACCESS_FS_READ_DIR) & handled
    } else {
        usr_access(handled)
    };
    let usr_rule = LandlockPathBeneathAttr { allowed_access: usr_allow, parent_fd: usr.as_raw_fd() };
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

    // slice-9 exec island: the ONE third-party inode allowed to execve. EXECUTE|READ_FILE scoped to
    // exactly the island path via a single-file O_PATH parent_fd (the per-file rule form proven for
    // /dev above). The island mount already dropped NOEXEC for this inode alone and re-verified it to
    // the pinned (dev,ino)+fs-verity digest; every other path stays no-exec (grants + /usr read-only).
    let mut island_fd: Option<OwnedFd> = None;
    if spec.exec_island.is_some() {
        let island = island_path(&spec.id);
        let ipath = island
            .to_str()
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "island path not utf-8"))?;
        let fd = open_opath(ipath)?;
        let rule = LandlockPathBeneathAttr {
            allowed_access: (LANDLOCK_ACCESS_FS_EXECUTE | LANDLOCK_ACCESS_FS_READ_FILE) & handled,
            parent_fd: fd.as_raw_fd(),
        };
        landlock_add_path_beneath(ruleset.as_raw_fd(), &rule)?;
        island_fd = Some(fd);
    }

    // slice-10 sealed-dynamic: EXECUTE|READ_FILE for each closure member too — the pinned interpreter
    // (at its PT_INTERP path) and every pinned library (under the island lib dir). Landlock gates WHICH
    // files may be execve'd/opened; the mmap(PROT_EXEC) boundary is the MS_NOEXEC posture on /usr +
    // grants (build_closure_island). Only these member inodes are both openable AND exec-capable.
    let mut member_fds: Vec<OwnedFd> = Vec::new();
    if let Some(closure) = spec.closure.as_ref() {
        let mut add_member = |target: &str| -> io::Result<()> {
            let fd = open_opath(target)?;
            let rule = LandlockPathBeneathAttr {
                allowed_access: (LANDLOCK_ACCESS_FS_EXECUTE | LANDLOCK_ACCESS_FS_READ_FILE) & handled,
                parent_fd: fd.as_raw_fd(),
            };
            landlock_add_path_beneath(ruleset.as_raw_fd(), &rule)?;
            member_fds.push(fd);
            Ok(())
        };
        add_member(&closure.interp.name)?;
        for lib in &closure.libs {
            let lp = island_lib_path(&spec.id, &lib.name);
            let lps = lp.to_str().ok_or_else(|| io::Error::new(io::ErrorKind::Other, "island lib path not utf-8"))?;
            add_member(lps)?;
        }
    }

    // no_new_privs is required before restrict_self without CAP_SYS_ADMIN and is a defence in depth
    // regardless; then enforce. The O_PATH fds (all O_CLOEXEC) vanish at execve.
    set_no_new_privs()?;
    landlock_restrict_self(ruleset.as_raw_fd())?;
    drop(pins);
    drop(dev_fds);
    drop(island_fd);
    drop(member_fds);
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

    // (4) execve (absolute path — no PATH search, which Landlock would deny anyway). For a slice-9
    // pinned build the target is the re-verified EXEC ISLAND, not the original entrypoint path
    // (whose source mount may be noexec) nor the source fd — the island is the exec-capable bind of
    // the same inode, re-verified (dev,ino)+fs-verity in build_exec_island and the sole EXECUTE inode.
    let exec_target: PathBuf = if spec.exec_island.is_some() {
        island_path(&spec.id)
    } else {
        PathBuf::from(&spec.workload[0])
    };
    let err = Command::new(&exec_target).args(&spec.workload[1..]).exec();
    eprintln!("proc_plane/P2: FAIL exec {}: {err} — failed closed", exec_target.display());
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
            // Contain any future mounts to this ns (belt-and-suspenders; base T0 mounts nothing).
            step(
                "mount-private",
                linux_uapi::mount(c"none", c"/", None, linux_uapi::MS_REC | linux_uapi::MS_PRIVATE, None),
            )?;

            // slice-9: build the exec island for a T-pinned static-PIE entrypoint — inside THIS private
            // mount ns (never touches the host mount table). Re-binds every mutable grant MS_NOEXEC and
            // gives the pinned inode alone a re-verified exec-capable island. Fail-closed: an error here
            // aborts P1 (exit 125) so no half-built exec home ever runs.
            if let Some(exec_fd) = spec.exec_island.as_ref() {
                match spec.closure.as_ref() {
                    // slice-10: a dynamically-linked pin builds the N-inode closure island.
                    Some(closure) => step("closure-island", build_closure_island(spec, exec_fd.as_raw_fd(), closure))?,
                    // slice-9: a static-PIE pin builds the single-inode exec island.
                    None => step("exec-island", build_exec_island(spec, exec_fd.as_raw_fd()))?,
                }
            }

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

    /// Build a minimal in-memory ELF64 (little-endian): a 64-byte header at `e_phoff=64` followed by
    /// `p_types.len()` program headers of 56 bytes each, only `p_type` populated. Enough to exercise
    /// the static-PIE gate without a real toolchain.
    fn synth_elf64(e_type: u16, p_types: &[u32]) -> Vec<u8> {
        const PHENT: usize = 56;
        let mut v = vec![0u8; 64 + p_types.len() * PHENT];
        v[0..4].copy_from_slice(b"\x7fELF");
        v[4] = 2; // ELFCLASS64
        v[5] = 1; // little-endian
        v[6] = 1; // version
        v[16..18].copy_from_slice(&e_type.to_le_bytes());
        v[32..40].copy_from_slice(&64u64.to_le_bytes()); // e_phoff
        v[54..56].copy_from_slice(&(PHENT as u16).to_le_bytes()); // e_phentsize
        v[56..58].copy_from_slice(&(p_types.len() as u16).to_le_bytes()); // e_phnum
        for (i, &t) in p_types.iter().enumerate() {
            let off = 64 + i * PHENT;
            v[off..off + 4].copy_from_slice(&t.to_le_bytes());
        }
        v
    }

    fn write_tmp(name: &str, bytes: &[u8]) -> PathBuf {
        let p = std::env::temp_dir().join(format!("shrek-elf-{}-{name}", std::process::id()));
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn static_pie_gate_accepts_loader_free_elf64_and_rejects_pt_interp() {
        const PT_LOAD: u32 = 1;
        const PT_INTERP: u32 = 3;
        const PT_GNU_STACK: u32 = 0x6474_e551;
        const ET_DYN: u16 = 3;
        const ET_EXEC: u16 = 2;

        // static PIE (ET_DYN, no INTERP) and static non-PIE (ET_EXEC, no INTERP) both pass.
        let pie = write_tmp("pie", &synth_elf64(ET_DYN, &[PT_LOAD, PT_LOAD, PT_GNU_STACK]));
        assert!(reject_if_dynamic(&pie).is_ok(), "static PIE must be accepted");
        let exec = write_tmp("exec", &synth_elf64(ET_EXEC, &[PT_LOAD, PT_GNU_STACK]));
        assert!(reject_if_dynamic(&exec).is_ok(), "static ET_EXEC must be accepted");

        // ANY PT_INTERP ⇒ dynamically linked ⇒ reject (Fork A).
        let dynamic = write_tmp("dyn", &synth_elf64(ET_DYN, &[PT_INTERP, PT_LOAD]));
        assert!(reject_if_dynamic(&dynamic).is_err(), "PT_INTERP must be rejected");

        // Not ELF64 / not ELF ⇒ fail closed.
        let bad_magic = write_tmp("bad", b"#!/bin/sh\necho hi\n");
        assert!(reject_if_dynamic(&bad_magic).is_err(), "non-ELF must be rejected");
        let mut elf32 = synth_elf64(ET_DYN, &[PT_LOAD]);
        elf32[4] = 1; // ELFCLASS32
        let p32 = write_tmp("elf32", &elf32);
        assert!(reject_if_dynamic(&p32).is_err(), "ELFCLASS32 must be rejected");

        for p in [pie, exec, dynamic, bad_magic, p32] {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn parse_interp_and_needed_reads_a_real_dynamic_binary() {
        // The test harness itself is a dynamically-linked ELF64 in this toolchain: it must have a
        // PT_INTERP (an ld.so) and at least one DT_NEEDED. Exercises the real ELF/dynamic parse
        // (PT_INTERP string + DT_STRTAB vaddr→offset mapping) without synthesizing a strtab.
        let me = std::fs::read_link("/proc/self/exe").expect("readlink /proc/self/exe");
        let info = parse_interp_and_needed(&me).expect("parse self");
        let interp = info.interp.expect("a dynamic binary has PT_INTERP");
        assert!(interp.contains("ld-"), "PT_INTERP should name an ld.so, got {interp:?}");
        assert!(!info.needed.is_empty(), "a dynamic binary has at least one DT_NEEDED");
    }

    #[test]
    fn authenticate_closure_matches_interp_and_needed_and_rejects_drift() {
        // Derive the true closure of /proc/self/exe, then prove authenticate_closure accepts the exact
        // closure and fails closed on a substituted interpreter or a missing DT_NEEDED member.
        let me = std::fs::read_link("/proc/self/exe").unwrap();
        let info = parse_interp_and_needed(&me).unwrap();
        let interp_path = info.interp.clone().unwrap();
        let mk = |name: &str, is_interp: bool| ClosureMemberSpec {
            algo_id: 1,
            digest: vec![0u8; 32],
            name: name.to_string(),
            is_interp,
        };
        let full = ClosureSpec {
            interp: mk(&interp_path, true),
            libs: info.needed.iter().map(|n| mk(n, false)).collect(),
        };
        assert!(authenticate_closure(&me, &full).is_ok(), "the exact closure must authenticate");

        // Wrong interpreter ⇒ reject (would let the kernel run an unpinned loader).
        let bad_interp = ClosureSpec { interp: mk("/lib/some-other-ld.so", true), libs: full.libs.iter().map(|l| mk(&l.name, false)).collect() };
        assert!(authenticate_closure(&me, &bad_interp).is_err(), "PT_INTERP drift must fail closed");

        // Drop one DT_NEEDED from the closure (only meaningful when the binary has needs) ⇒ reject.
        if !info.needed.is_empty() {
            let short = ClosureSpec {
                interp: mk(&interp_path, true),
                libs: info.needed.iter().skip(1).map(|n| mk(n, false)).collect(),
            };
            assert!(authenticate_closure(&me, &short).is_err(), "an unpinned DT_NEEDED must fail closed");
        }
    }

    #[test]
    fn authenticate_closure_rejects_a_static_entrypoint() {
        // A loader-free ELF64 (no PT_INTERP) is not a dynamic closure — it belongs on the slice-9 route.
        const ET_DYN: u16 = 3;
        const PT_LOAD: u32 = 1;
        let stat = write_tmp("static", &synth_elf64(ET_DYN, &[PT_LOAD, PT_LOAD]));
        let spec = ClosureSpec {
            interp: ClosureMemberSpec { algo_id: 1, digest: vec![0u8; 32], name: "/lib/ld.so".into(), is_interp: true },
            libs: vec![],
        };
        assert!(authenticate_closure(&stat, &spec).is_err(), "a static ELF has no PT_INTERP ⇒ reject");
        let _ = std::fs::remove_file(stat);
    }

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
