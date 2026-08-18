//! sandbox — construct one T1 (`systemd-nspawn`) isolation tier for a trivial workload with a
//! capability-enforced filesystem mount-set. Phase-5 slice-1 (docs/phase5-slice1-mount.md), with
//! the slice-2 decision plane layered on (docs/phase5-slice2-tier.md).
//!
//! This is the M3 integration: gatekeeperd, from a grant (a subject's read-caps over paths beneath a
//! trusted anchor), builds a sandbox in which `caps ⊆ profile` holds *at construction* — a
//! granted-out path is ABSENT from the sandbox, not merely unreadable. Slice-2 adds the `(trust×caps)
//! →tier` re-check (`recheck`): the request's tier is independently recomputed from the compiled-in
//! sealed matrix and REFUSED if below floor, and any tier ≥ T2 or egress-needing caps fails closed
//! (no T2/T3 constructor or egress plane exists yet). Only T1 is constructed; the egress plane is a
//! later slice.
//!
//! Construction (all inside a private mount namespace so nothing touches the host mount table):
//!   1. synthetic OS-shaped root — the base runtime (`/usr`) bound read-only, an EMPTY grant tree.
//!   2. each grant: pin beneath the anchor (TOCTOU-safe), relocate read-only to a broker-owned path.
//!   3. `systemd-nspawn --directory=<root> --private-users=pick --bind-ro=<pinned>:<guest>` runs the
//!      workload. The empty grant tree hides ungranted siblings (ENOENT, not EACCES); --private-users
//!      is mandatory or UID isolation silently fails.

use crate::mount_plane::{bind_ro, enter_private_mount_ns, open_anchor, pin_beneath, relocate_ro};
use shrek_tier::{effective_tier, CapsProfile, Tier, TrustBand};
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

/// Outcome of the privileged re-check (isolation.md §7 steps 4–5) plus this slice's constructibility
/// gate. `Construct` carries the effective tier we are cleared to build (T0 folds up to T1);
/// `Refuse` carries a distinct exit code + an audit reason.
enum Decision {
    Construct { effective: Tier },
    Refuse { code: i32, reason: String },
}

/// The independent re-check — gatekeeperd trusts NONE of agentd's numbers. It recomputes the tier
/// bound from the COMPILED-IN `shrek-tier` matrix/floor (sealed by dm-verity in the shipped `/usr`,
/// NOT read from agentd or any writable state — isolation.md §7, security-model.md §4). This proves
/// the arithmetic/floor independence: a bug or compromise in the unprivileged resolver cannot widen
/// a sandbox. (Integrity-sourcing the trust band ITSELF is OPEN B1, a separate upstream slice; here
/// `trust`/`caps` still ride in with the request, and the fail-high parse guarantees a garbled band
/// only ever raises the wall.)
fn recheck(requested: Tier, trust: TrustBand, caps: CapsProfile, profile: CapsProfile) -> Decision {
    // (a) Downgrade/floor. Refuse anything below max(matrix, floor); a requested tier ABOVE the
    // bound is a legal upward escalation and is honored. This is the downward-forbidden invariant.
    let bound = effective_tier(trust, caps, None); // = max(matrix[trust][caps], floor(trust))
    if requested < bound {
        return Decision::Refuse {
            code: 10,
            reason: format!("downgrade-below-floor bound={}", bound.label()),
        };
    }
    // (b) caps ⊆ granted profile, re-checked independently of agentd's step 1.
    if !caps.subset_of(profile) {
        return Decision::Refuse { code: 11, reason: "caps-exceed-profile".into() };
    }
    let effective = requested; // >= bound; honor any upward escalation agentd applied
    // (c) Constructibility — the honest limit of THIS slice. Only the T1 constructor exists and
    // there is no egress plane yet. Anything stronger, or anything needing egress, FAILS CLOSED —
    // it is NEVER silently downgraded to T1 (security-model.md §7: no unconfined fallback, ever).
    if effective >= Tier::T2 {
        return Decision::Refuse {
            code: 12,
            reason: format!("no-constructor-{} (slice-3)", effective.label()),
        };
    }
    if caps.needs_egress() {
        return Decision::Refuse { code: 13, reason: "no-egress-plane (later slice)".into() };
    }
    // effective ∈ {T0, T1}, caps ∈ {C-ro-nosec, C-proj-rw}: build at T1. A T0 result at T1 is a
    // legal upward escalation until the real T0 (Landlock) constructor lands.
    Decision::Construct { effective }
}

/// CLI entrypoint: `gatekeeperd sandbox [--tier Tn --trust T --caps C [--profile C]] --id X
/// --anchor /srv --grant NAME [--grant NAME] [--guest-prefix /srv] -- <workload argv...>`.
///
/// Two modes: with `--tier` present it runs the decision-plane re-check (slice-2) before
/// constructing or refusing; without it, it is the slice-1 mount-plane path (direct T1 construct),
/// unchanged so the M0–M4 proofs still hold. Returns a process exit code; a socket verb is slice #5.
pub fn cli(args: &[String]) -> i32 {
    let mut id = String::from("s0");
    let mut anchor = PathBuf::from("/srv");
    let mut guest_prefix = PathBuf::from("/srv");
    let mut grants: Vec<String> = Vec::new();
    let mut workload: Vec<String> = Vec::new();
    let mut tier_s: Option<String> = None;
    let mut trust_s: Option<String> = None;
    let mut caps_s: Option<String> = None;
    let mut profile_s: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--id" => { i += 1; id = args.get(i).cloned().unwrap_or(id); }
            "--anchor" => { i += 1; if let Some(v) = args.get(i) { anchor = PathBuf::from(v); } }
            "--guest-prefix" => { i += 1; if let Some(v) = args.get(i) { guest_prefix = PathBuf::from(v); } }
            "--grant" => { i += 1; if let Some(v) = args.get(i) { grants.push(v.clone()); } }
            "--tier" => { i += 1; tier_s = args.get(i).cloned(); }
            "--trust" => { i += 1; trust_s = args.get(i).cloned(); }
            "--caps" => { i += 1; caps_s = args.get(i).cloned(); }
            "--profile" => { i += 1; profile_s = args.get(i).cloned(); }
            "--" => { workload = args[i + 1..].to_vec(); break; }
            other => { eprintln!("gatekeeperd/sandbox: unknown arg {other}"); return 2; }
        }
        i += 1;
    }
    if grants.is_empty() || workload.is_empty() {
        eprintln!("usage: gatekeeperd sandbox [--tier Tn --trust T --caps C] --anchor DIR --grant NAME [...] -- WORKLOAD...");
        return 2;
    }

    // Decision-plane mode (slice-2): re-check the resolved request before touching the mount plane.
    if let Some(t) = tier_s {
        let Some(requested) = Tier::parse(&t) else {
            eprintln!("SANDBOX-DECISION refused reason=bad-request-tier={t:?}");
            return 14;
        };
        // Fail-high on the security-relevant axes; a missing profile defaults to the requested caps.
        let trust = TrustBand::parse(trust_s.as_deref().unwrap_or(""));
        let caps = CapsProfile::parse(caps_s.as_deref().unwrap_or(""));
        let profile = profile_s.as_deref().map(CapsProfile::parse).unwrap_or(caps);
        match recheck(requested, trust, caps, profile) {
            Decision::Refuse { code, reason } => {
                eprintln!(
                    "SANDBOX-DECISION refused reason={reason} requested={} trust={} caps={} profile={}",
                    requested.label(), trust.label(), caps.label(), profile.label()
                );
                return code;
            }
            Decision::Construct { effective } => {
                eprintln!(
                    "SANDBOX-DECISION cleared construct-at=T1 effective={} requested={} trust={} caps={} profile={}",
                    effective.label(), requested.label(), trust.label(), caps.label(), profile.label()
                );
                // fall through to construction
            }
        }
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
