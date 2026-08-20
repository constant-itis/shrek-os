//! gate-probe — a sealed CLOSED-WORLD in-sandbox acceptance probe (Phase-5 slice-7 / B1).
//!
//! WHY THIS EXISTS: B1 derives the trust band by measuring the workload entrypoint and correctly
//! treats an interpreter (`/bin/sh`) as OPEN-WORLD, so a shell can no longer legitimately derive
//! `T-first` (docs/phase5-slice7-trust-provenance.md §5.1, §8.1). The VM gates that assert in-sandbox
//! isolation *mechanics* at T0/T1 therefore need a fixed, sealed, closed-world program instead of a
//! shell one-liner. This is that program.
//!
//! CLOSED-WORLD INVARIANTS (so it legitimately earns `T-first` once sealed + enrolled):
//!   * no child `exec`, no `dlopen` of mutable objects, no interpreted/generated code;
//!   * the single argument (`mode`, and an optional anchor path) is treated ONLY as DATA — it selects
//!     which fixed check-set to run and which path to `stat`; it is never executed;
//!   * outputs are FIXED, enumerated `SHREK_GATE:` lines — identical to the shell probes they replace.
//!
//! Spike-only: strip this crate, its image install, and its `CLOSED_WORLD` enrolment before ship.

use std::ffi::CString;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

fn pass(gate: &str) {
    println!("SHREK_GATE: PASS gate={gate}");
}
fn fail(gate: &str, why: &str) {
    println!("SHREK_GATE: FAIL gate={gate} {why}");
}
fn check(gate: &str, ok: bool, why: &str) {
    if ok {
        pass(gate)
    } else {
        fail(gate, why)
    }
}

/// Read a file and return its trimmed contents, or the io error kind on failure.
fn read_trim(p: &str) -> Result<String, ErrorKind> {
    fs::read_to_string(p).map(|s| s.trim().to_string()).map_err(|e| e.kind())
}

/// Sorted directory entry names, or the io error kind.
fn dir_names(p: &str) -> Result<Vec<String>, ErrorKind> {
    let mut v = Vec::new();
    for e in fs::read_dir(p).map_err(|e| e.kind())? {
        let e = e.map_err(|e| e.kind())?;
        v.push(e.file_name().to_string_lossy().into_owned());
    }
    v.sort();
    Ok(v)
}

// x86_64 raw syscalls std cannot express — used only to prove the kernel DENIES them; on error the
// kernel returns `-errno`. No libc dependency (minimal-deps convention).
const SYS_MOUNT: i64 = 165;
const SYS_MMAP: i64 = 9;
const SYS_EXECVE: i64 = 59;
const EPERM: i64 = 1;
const EACCES: i64 = 13;
const PROT_READ: i64 = 1;
const PROT_EXEC: i64 = 4;
const MAP_PRIVATE: i64 = 2;
const O_RDONLY: i64 = 0;
const SYS_OPEN: i64 = 2;
const SYS_CLOSE: i64 = 3;

unsafe fn syscall5(n: i64, a1: i64, a2: i64, a3: i64, a4: i64, a5: i64) -> i64 {
    let ret: i64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") n => ret,
        in("rdi") a1, in("rsi") a2, in("rdx") a3,
        in("r10") a4, in("r8") a5,
        lateout("rcx") _, lateout("r11") _,
        options(nostack)
    );
    ret
}

unsafe fn syscall6(n: i64, a1: i64, a2: i64, a3: i64, a4: i64, a5: i64, a6: i64) -> i64 {
    let ret: i64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") n => ret,
        in("rdi") a1, in("rsi") a2, in("rdx") a3,
        in("r10") a4, in("r8") a5, in("r9") a6,
        lateout("rcx") _, lateout("r11") _,
        options(nostack)
    );
    ret
}

/// slice-9 exec-island mode: this program IS the pinned static-PIE entrypoint, so merely REACHING
/// `main` proves the exec island executed re-verified pinned bytes. It then probes the load-bearing
/// no-laundering property (I2, docs/phase5-slice9 §1, kernel-fact #2624): a MUTABLE grant is on an
/// `MS_NOEXEC` mount, so both `mmap(PROT_EXEC)` (library-load) AND `execve` of it must fail — the
/// `mmap` case is the one Landlock does NOT govern, so it is the critical assertion. `grant` is a path
/// to a real executable file on a mutable grant; it is DATA only (opened + probed, never our loader).
fn mode_island(grant: &str) {
    pass("island-ran"); // reaching here = the pinned static PIE ran from the exec island

    // Open the grant read-only (Landlock grants READ on a mutable grant). If even the open fails the
    // probe cannot make its point — surface it rather than silently passing.
    let cpath = match CString::new(grant) {
        Ok(c) => c,
        Err(_) => return fail("island-grant-open", "bad path"),
    };
    let fd = unsafe { syscall5(SYS_OPEN, cpath.as_ptr() as i64, O_RDONLY, 0, 0, 0) };
    if fd < 0 {
        return fail("island-grant-open", &format!("open ret={fd}"));
    }

    // (a) mmap(PROT_EXEC) of the mutable grant ⇒ EPERM on a noexec mount (mmap(2): "the mapped area
    //     belongs to a file on a filesystem mounted no-exec"). THIS is what stops mutable bytes from
    //     being loaded as a shared library, and it is not governed by Landlock EXECUTE.
    let m = unsafe { syscall6(SYS_MMAP, 0, 4096, PROT_READ | PROT_EXEC, MAP_PRIVATE, fd, 0) };
    check("island-grant-mmap-exec-eperm", m == -EPERM, &format!("mmap ret={m}"));
    unsafe {
        syscall5(SYS_CLOSE, fd, 0, 0, 0, 0);
    }

    // (b) execve of the mutable grant ⇒ denied (noexec EPERM and/or Landlock no-EXECUTE EACCES). On
    //     success execve never returns; any return is a failure code we assert is a denial.
    let argv = [cpath.as_ptr() as i64, 0i64];
    let envp = [0i64];
    let e = unsafe { syscall5(SYS_EXECVE, cpath.as_ptr() as i64, argv.as_ptr() as i64, envp.as_ptr() as i64, 0, 0) };
    check("island-grant-execve-denied", e == -EPERM || e == -EACCES, &format!("execve ret={e}"));
}

/// Attempt `mount -t tmpfs none /mnt`. Returns the raw syscall result (0 = succeeded, `-errno` else).
fn try_mount_tmpfs() -> i64 {
    let src = CString::new("none").unwrap();
    let tgt = CString::new("/mnt").unwrap();
    let fst = CString::new("tmpfs").unwrap();
    unsafe {
        syscall5(
            SYS_MOUNT,
            src.as_ptr() as i64,
            tgt.as_ptr() as i64,
            fst.as_ptr() as i64,
            0,
            0,
        )
    }
}

/// T1 (nspawn) mount-plane mechanics, guest paths under `/srv`. Mirrors the M4/S2 shell `PROBE`.
fn mode_mount_guest() {
    match read_trim("/srv/project/marker") {
        Ok(m) => check("in-project-readable", m == "PROJECT", &format!("m={m}")),
        Err(e) => fail("in-project-readable", &format!("err={e:?}")),
    }
    // vault is not mounted into the guest tree ⇒ ENOENT (absent, not merely unreadable).
    match read_trim("/srv/vault/marker") {
        Err(ErrorKind::NotFound) => pass("in-vault-enoent"),
        other => fail("in-vault-enoent", &format!("got={other:?}")),
    }
    match dir_names("/srv") {
        Ok(n) => check("in-vault-absent", n == ["project"], &format!("ls={n:?}")),
        Err(e) => fail("in-vault-absent", &format!("err={e:?}")),
    }
    // --private-users=pick maps the owner to the nobody uid (65534) inside the guest.
    match fs::metadata("/srv/project/marker") {
        Ok(md) => check("in-private-users", md.uid() == 65534, &format!("uid={}", md.uid())),
        Err(e) => fail("in-private-users", &format!("err={e:?}")),
    }
}

/// T1 no-net cell: only loopback in the private netns. Mirrors the S3 `NETPROBE`.
fn mode_nonet() {
    match dir_names("/sys/class/net") {
        Ok(n) => check("s3-nonet-loopback-only", n == ["lo"], &format!("ifs={n:?}")),
        Err(e) => fail("s3-nonet-loopback-only", &format!("err={e:?}")),
    }
}

/// T0 (Landlock+seccomp) mechanics against the REAL anchor path. Mirrors the S4 `T0PROBE`. `anchor`
/// is DATA only (a path to stat), never executed.
fn mode_landlock(anchor: &str) {
    let a = Path::new(anchor);
    let proj = a.join("project/marker");
    let vault = a.join("vault/marker");
    match read_trim(&proj.to_string_lossy()) {
        Ok(m) => check("t0-grant-readable", m == "PROJECT", &format!("m={m}")),
        Err(e) => fail("t0-grant-readable", &format!("err={e:?}")),
    }
    // Landlock denies the ungranted vault ⇒ EACCES (PermissionDenied), not ENOENT.
    match read_trim(&vault.to_string_lossy()) {
        Err(ErrorKind::PermissionDenied) => pass("t0-vault-landlock-denied"),
        other => fail("t0-vault-landlock-denied", &format!("got={other:?}")),
    }
    // seccomp denies mount(2) ⇒ EPERM/EACCES.
    let r = try_mount_tmpfs();
    check("t0-seccomp-mount-eperm", r == -EPERM || r == -EACCES, &format!("ret={r}"));
    // Landlock denies /sys (not granted) ⇒ PermissionDenied on read_dir.
    match dir_names("/sys/class/net") {
        Err(ErrorKind::PermissionDenied) => pass("t0-sys-landlock-denied"),
        other => fail("t0-sys-landlock-denied", &format!("got={other:?}")),
    }
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    match mode.as_str() {
        "mount-guest" => mode_mount_guest(),
        "nonet" => mode_nonet(),
        "landlock" => mode_landlock(&std::env::args().nth(2).unwrap_or_else(|| "/srv".into())),
        "island" => mode_island(&std::env::args().nth(2).unwrap_or_default()),
        other => {
            // Unknown mode: emit nothing actionable (fixed output), exit nonzero. Used by the refusal
            // gates where the workload must never run anyway.
            eprintln!("gate-probe: unknown mode {other:?}");
            std::process::exit(2);
        }
    }
}
