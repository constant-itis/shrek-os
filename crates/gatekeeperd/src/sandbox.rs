//! sandbox — construct one T1 (`systemd-nspawn`) isolation tier for a trivial workload with a
//! capability-enforced filesystem mount-set. Phase-5 slice-1 (docs/phase5-slice1-mount.md).
//!
//! This is the M3 integration: gatekeeperd, from a grant (a subject's read-caps over paths beneath a
//! trusted anchor), builds a sandbox in which `caps ⊆ profile` holds *at construction* — a
//! granted-out path is ABSENT from the sandbox, not merely unreadable. Tier selection is stubbed to
//! T1; the `(trust×caps)→tier` matrix and the egress plane are later slices.
//!
//! Construction (all inside a private mount namespace so nothing touches the host mount table):
//!   1. synthetic OS-shaped root — the base runtime (`/usr`) bound read-only, an EMPTY grant tree.
//!   2. each grant: pin beneath the anchor (TOCTOU-safe), relocate read-only to a broker-owned path.
//!   3. `systemd-nspawn --directory=<root> --private-users=pick --bind-ro=<pinned>:<guest>` runs the
//!      workload. The empty grant tree hides ungranted siblings (ENOENT, not EACCES); --private-users
//!      is mandatory or UID isolation silently fails.

use crate::mount_plane::{bind_ro, enter_private_mount_ns, open_anchor, pin_beneath, relocate_ro};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct SandboxSpec {
    pub id: String,
    pub anchor: PathBuf,
    pub grants: Vec<String>,
    pub guest_prefix: PathBuf,
    pub workload: Vec<String>,
}

/// Build the synthetic root: an OS-tree-shaped directory (nspawn requires `/usr/` to exist — M0)
/// whose only populated content is the base runtime. The grant tree (`guest_prefix`) is created
/// EMPTY; only relocated grants are bound into it, so ungranted siblings are absent.
fn build_synth_root(root: &Path, guest_prefix: &Path) -> io::Result<()> {
    for d in ["usr", "etc", "proc", "sys", "dev", "run", "tmp"] {
        std::fs::create_dir_all(root.join(d))?;
    }
    // The grant tree, empty. guest_prefix is absolute (e.g. /srv) — join onto root.
    let gp = root.join(guest_prefix.strip_prefix("/").unwrap_or(guest_prefix));
    std::fs::create_dir_all(&gp)?;

    // Base OS runtime: bind the host /usr read-only + usr-merged symlinks so /bin,/lib,/lib64,/sbin
    // and the dynamic linker resolve. In the sealed image this is the dm-verity /usr.
    bind_ro(Path::new("/usr"), &root.join("usr"))?;
    for (link, tgt) in [("bin", "usr/bin"), ("sbin", "usr/sbin"), ("lib", "usr/lib"), ("lib64", "usr/lib64")] {
        let p = root.join(link);
        if !p.exists() {
            let _ = std::os::unix::fs::symlink(tgt, &p);
        }
    }
    std::fs::write(root.join("etc/os-release"), b"ID=shrek-sandbox\n")?;
    Ok(())
}

/// Construct and run the sandbox. Returns the workload's exit code. MUST run as root (needs
/// CAP_SYS_ADMIN for the mount namespace, the binds, and nspawn). Any failure is returned as an
/// error — the caller fails closed (no partial/unsafe sandbox is ever handed back).
pub fn construct(spec: &SandboxSpec) -> io::Result<i32> {
    // Private mount namespace — everything below is contained + non-propagating.
    enter_private_mount_ns()?;

    let runtime = Path::new("/run/shrek").join(&spec.id);
    let root = runtime.join("root");
    let stage = runtime.join("grants");
    let _ = std::fs::remove_dir_all(&runtime);
    std::fs::create_dir_all(&stage)?;

    build_synth_root(&root, &spec.guest_prefix)?;

    // Pin + relocate each grant onto a plain, broker-owned path, then bind THAT into the sandbox.
    let anchor = open_anchor(&spec.anchor)?;
    let mut binds: Vec<(String, String)> = Vec::new();
    for name in &spec.grants {
        let pinned = pin_beneath(&anchor, name)?;
        let target = stage.join(name);
        relocate_ro(&pinned, &target)?;
        let guest = spec.guest_prefix.join(name);
        binds.push((target.display().to_string(), guest.display().to_string()));
        eprintln!(
            "gatekeeperd/sandbox: pinned+relocated grant {name} (dev={}:{} ino={}) -> {}",
            pinned.ident.dev_major, pinned.ident.dev_minor, pinned.ident.ino, target.display()
        );
    }

    let mut cmd = Command::new("systemd-nspawn");
    cmd.arg("-q")
        .arg("--console=pipe") // clean non-interactive stdio (no pty \r injection)
        .arg("--register=no")
        .arg("--keep-unit")
        .arg(format!("--machine=shrek-{}", spec.id))
        .arg(format!("--directory={}", root.display()))
        // --private-users MANDATORY (M0 trap 2) — or UID isolation silently fails. ownership=off:
        // do NOT idmap the bind sources (idmapped mounts hit EBUSY inside our nested private
        // mount-ns); the plain userns view still maps ungranted host-root content to the overflow
        // uid, which is the isolation property we assert.
        .arg("--private-users=pick")
        .arg("--private-users-ownership=off");
    for (src, guest) in &binds {
        cmd.arg(format!("--bind-ro={src}:{guest}"));
    }
    cmd.arg("--");
    for w in &spec.workload {
        cmd.arg(w);
    }
    eprintln!("gatekeeperd/sandbox: exec {cmd:?}");
    let status = cmd.status()?;
    Ok(status.code().unwrap_or(-1))
}

/// CLI entrypoint: `gatekeeperd sandbox --id X --anchor /srv --grant NAME [--grant NAME]
/// [--guest-prefix /srv] -- <workload argv...>`. Returns a process exit code. Spike surface for
/// slice-1 (M3/M4); a socket verb is a later slice.
pub fn cli(args: &[String]) -> i32 {
    let mut id = String::from("s0");
    let mut anchor = PathBuf::from("/srv");
    let mut guest_prefix = PathBuf::from("/srv");
    let mut grants: Vec<String> = Vec::new();
    let mut workload: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--id" => { i += 1; id = args.get(i).cloned().unwrap_or(id); }
            "--anchor" => { i += 1; if let Some(v) = args.get(i) { anchor = PathBuf::from(v); } }
            "--guest-prefix" => { i += 1; if let Some(v) = args.get(i) { guest_prefix = PathBuf::from(v); } }
            "--grant" => { i += 1; if let Some(v) = args.get(i) { grants.push(v.clone()); } }
            "--" => { workload = args[i + 1..].to_vec(); break; }
            other => { eprintln!("gatekeeperd/sandbox: unknown arg {other}"); return 2; }
        }
        i += 1;
    }
    if grants.is_empty() || workload.is_empty() {
        eprintln!("usage: gatekeeperd sandbox --anchor DIR --grant NAME [...] -- WORKLOAD...");
        return 2;
    }
    let spec = SandboxSpec { id, anchor, grants, guest_prefix, workload };
    match construct(&spec) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("gatekeeperd/sandbox: FAIL construction: {e}");
            3
        }
    }
}
