//! M2 — fd-pinning TOCTOU proof (privileged: needs CAP_SYS_ADMIN for the bind + mount ns).
//!
//! Proves the pin → verify → relocate recipe defeats a source-path swap: we pin `srv/project`, an
//! adversary then swaps `srv/project` into a symlink at `srv/vault`, and the relocate — binding from
//! the *pinned fd* — still lands the ORIGINAL project inode, read-only, at a plain path nspawn can
//! consume. A naive path-string bind would have leaked the vault.
//!
//! Build on the host (`cargo build --example mount_toctou_proof`), run inside a privileged
//! debian:trixie container (docs/phase5-slice1-mount.md, M2). Spike-only: not shipped in the image.

use gatekeeperd::mount_plane::{enter_private_mount_ns, open_anchor, pin_beneath, relocate_ro};
use std::path::Path;
use std::process::exit;

fn gate(name: &str, cond: bool, detail: &str) -> bool {
    if cond {
        println!("SHREK_GATE: PASS gate={name} {detail}");
    } else {
        println!("SHREK_GATE: FAIL gate={name} reason={detail}");
    }
    cond
}

fn fatal(gatename: &str, e: impl std::fmt::Display) -> ! {
    println!("SHREK_GATE: FAIL gate={gatename} reason={e}");
    println!("SHREK_GATE: FAIL M2 fd-pinning TOCTOU proof");
    exit(3);
}

fn main() {
    let base = std::env::var("M2_BASE").unwrap_or_else(|_| "/run/shrek-m2".into());
    let base = Path::new(&base);
    let srv = base.join("srv");
    let sandbox = base.join("sandbox");

    // Private mount namespace FIRST — every bind below is contained and non-propagating.
    if let Err(e) = enter_private_mount_ns() {
        fatal("ns", e);
    }

    // Fixtures: host tree with a granted 'project' and a denied 'vault', each with a distinct marker.
    let _ = std::fs::remove_dir_all(base);
    for d in ["project", "vault"] {
        std::fs::create_dir_all(srv.join(d)).unwrap_or_else(|e| fatal("fixtures", e));
    }
    std::fs::write(srv.join("project/marker"), b"PROJECT").unwrap();
    std::fs::write(srv.join("vault/marker"), b"VAULT").unwrap();

    let anchor = open_anchor(&srv).unwrap_or_else(|e| fatal("anchor", e));

    // 1. Pin the grant beneath the trusted anchor.
    let pinned = pin_beneath(&anchor, "project").unwrap_or_else(|e| fatal("pin", e));
    let orig = pinned.ident;
    let mut ok = true;
    ok &= gate("pin", pinned.is_dir, &format!("ino={} dir={}", orig.ino, pinned.is_dir));

    // 2. Adversary swaps the granted path AFTER the pin: project -> symlink to vault.
    std::fs::rename(srv.join("project"), srv.join("project.orig")).unwrap();
    std::os::unix::fs::symlink(srv.join("vault"), srv.join("project")).unwrap();
    let naive = std::fs::read_to_string(srv.join("project/marker")).unwrap_or_default();
    println!("  [evidence] post-swap srv/project/marker now resolves to {naive:?} — a naive path bind would leak this");

    // 3. Relocate from the pinned fd onto a plain, broker-owned target.
    let target = sandbox.join("project");
    match relocate_ro(&pinned, &target) {
        Ok(()) => ok &= gate("relocate", true, "bind + re-verify ok"),
        Err(e) => ok &= gate("relocate", false, &format!("{e}")),
    }

    // 4. Content at the target must be the ORIGINAL project, never the vault.
    let got = std::fs::read_to_string(target.join("marker")).unwrap_or_default();
    ok &= gate("content", got == "PROJECT", &format!("marker={got:?} (must be \"PROJECT\")"));

    // 5. Read-only enforced — creating a file under the target must fail.
    let werr = std::fs::write(target.join("attacker"), b"x");
    ok &= gate("readonly", werr.is_err(), &format!("write_errno={:?}", werr.err().and_then(|e| e.raw_os_error())));

    // 6. Re-pinning the swapped (now-symlinked) path is refused outright.
    let repin = pin_beneath(&anchor, "project");
    ok &= gate("repin-refused", repin.is_err(), &format!("errno={:?}", repin.err().and_then(|e| e.raw_os_error())));

    // 7. The pin identity never moved.
    ok &= gate("ident-stable", orig == pinned.ident, "pin identity unchanged across the swap");

    println!("SHREK_GATE: {} M2 fd-pinning TOCTOU proof", if ok { "PASS" } else { "FAIL" });
    exit(if ok { 0 } else { 1 });
}
