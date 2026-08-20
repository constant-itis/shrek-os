//! sandbox — construct one T1 (`systemd-nspawn`) isolation tier for a trivial workload with a
//! capability-enforced filesystem mount-set. Phase-5 slice-1 (docs/phase5-slice1-mount.md), with
//! the slice-2 decision plane layered on (docs/phase5-slice2-tier.md).
//!
//! This is the M3 integration: gatekeeperd, from a grant (a subject's read-caps over paths beneath a
//! trusted anchor), builds a sandbox in which `caps ⊆ profile` holds *at construction* — a
//! granted-out path is ABSENT from the sandbox, not merely unreadable. Slice-2 adds the `(trust×caps)
//! →tier` re-check (`recheck`): the request's tier is independently recomputed from the compiled-in
//! sealed matrix and REFUSED if below floor, and any tier ≥ T2 fails closed (no T2/T3 constructor).
//! Slice-3 adds the egress plane: EVERY sandbox gets `--private-network` (a fresh loopback-only
//! netns — the no-net default that also closed the old host-netns hole), and a ≤T1 C-net cell that
//! names a SEALED egress profile gets a gatekeeperd-injected veth + nft allow-list (see net_plane).
//! C-broad and any tier ≥ T2 still fail closed.
//!
//! Construction (all inside a private mount namespace so nothing touches the host mount table):
//!   1. synthetic OS-shaped root — the base runtime (`/usr`) bound read-only, an EMPTY grant tree.
//!   2. each grant: pin beneath the anchor (TOCTOU-safe), relocate read-only to a broker-owned path.
//!   3. `systemd-nspawn --directory=<root> --private-users=pick --private-network
//!      --bind-ro=<pinned>:<guest>` runs the workload. The empty grant tree hides ungranted siblings
//!      (ENOENT, not EACCES); --private-users is mandatory or UID isolation silently fails;
//!      --private-network gives every sandbox its own loopback-only netns (egress is injected, never
//!      shared from the host).

use crate::mount_plane::{bind_ro, enter_private_mount_ns, open_anchor, pin_beneath, relocate_ro};
use crate::net_plane;
use crate::proc_plane::{self, T0Spec, DEFAULT_MEM_MAX, DEFAULT_PIDS_MAX};
use crate::provenance_plane;
use crate::t2_plane::{self, T2Spec};
use shrek_policy::egress::EgressProfile;
use shrek_policy::{effective_tier, CapsProfile, Tier, TrustBand};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Egress decision for a constructed sandbox. `None` = loopback-only (`--private-network`, no
/// injection) — every no-net cell, and the slice-1 mount-plane path. `Profile` = a ≤T1 C-net cell
/// whose SEALED egress profile gatekeeperd resolved; the net plane injects a veth + nft allow-list.
#[derive(Clone, Copy)]
pub enum Egress {
    None,
    Profile(&'static EgressProfile),
}

pub struct SandboxSpec {
    pub id: String,
    pub anchor: PathBuf,
    pub grants: Vec<String>,
    pub guest_prefix: PathBuf,
    pub workload: Vec<String>,
    pub egress: Egress,
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
        .arg("--private-users-ownership=off")
        // Slice-3: EVERY sandbox OWNS a fresh netns (loopback-only). This is the no-net default —
        // it closes the historical host-netns hole (construct() used to pass no network flag, so
        // nspawn shared the host netns) — and it is the baseline the C-net egress plane injects
        // into. It MUST be --private-network (nspawn owning the ns), not --network-namespace-path
        // into a pre-made one: --private-users cannot join a host-owned netns (EPERM — #2563 / C2).
        .arg("--private-network");
    for (src, guest) in &binds {
        cmd.arg(format!("--bind-ro={src}:{guest}"));
    }

    // Resolve egress destinations BEFORE spawning: an unresolvable/AAAA-only host must fail closed
    // with NO sandbox at all. An empty profile is treated as no-net (reaches nothing either way).
    let resolved = match spec.egress {
        Egress::Profile(p) if !p.rules.is_empty() => Some(net_plane::resolve_profile_v4(p)?),
        _ => None,
    };

    match resolved {
        // No-net cell (and the slice-1 mount path): loopback-only, run straight through.
        None => {
            cmd.arg("--");
            for w in &spec.workload {
                cmd.arg(w);
            }
            eprintln!("gatekeeperd/sandbox: exec (no-net) {cmd:?}");
            Ok(cmd.status()?.code().unwrap_or(-1))
        }
        Some(res) => {
            // Pin the sealed hosts into the sandbox's /etc/hosts so the workload resolves the profile
            // names WITHOUT any DNS egress (there is none — the nft allow-list is IP+port only).
            std::fs::write(root.join("etc/hosts"), net_plane::etc_hosts(&res.hosts))?;
            run_egress(cmd, &runtime, &spec.id, &spec.workload, &res.endpoints)
        }
    }
}

/// The ready-barrier the C-net workload is wrapped in: block until gatekeeperd has injected the veth
/// + nft rules (`/rv/go`), abort fail-closed if injection failed (`/rv/abort`), or time out (~60s).
/// `exec "$@"` then becomes the real workload. Requires `/bin/sh` in the sandbox (present via the
/// bound `/usr`). This is what makes egress rules-before-usable: no workload byte leaves until the
/// default-deny + allow-list is live.
const EGRESS_BARRIER: &str =
    "n=0; while [ ! -e /rv/go ]; do [ -e /rv/abort ] && exit 71; n=$((n+1)); [ \"$n\" -gt 1200 ] && exit 71; sleep 0.05; done; exec \"$@\"";

/// C-net construction: spawn nspawn (which OWNS the netns), discover the container leader, inject the
/// egress plane, release the barrier, then wait. ANY setup failure aborts the workload, tears the
/// plane down, and returns the error — fail closed, no host-network fallback, no residual plumbing.
fn run_egress(
    mut cmd: Command,
    runtime: &Path,
    id: &str,
    workload: &[String],
    endpoints: &[net_plane::Endpoint],
) -> io::Result<i32> {
    // Rendezvous dir, bound into the sandbox at /rv; the barrier polls /rv/go and /rv/abort.
    let rv = runtime.join("rv");
    std::fs::create_dir_all(&rv)?;
    cmd.arg(format!("--bind={}:/rv", rv.display()));
    cmd.arg("--").arg("/bin/sh").arg("-c").arg(EGRESS_BARRIER).arg("shrek-egress-barrier");
    for w in workload {
        cmd.arg(w);
    }

    let net = net_plane::SandboxNet::for_id(id);
    let barrier = net_plane::Barrier { dir: rv };

    eprintln!(
        "gatekeeperd/sandbox: exec (egress ns={} cip={} dsts={}) {cmd:?}",
        net.ns, net.cont_ip, endpoints.len()
    );
    let mut child = cmd.spawn()?;
    let nspawn_pid = child.id();

    let setup = (|| -> io::Result<()> {
        let leader = net_plane::discover_leader(nspawn_pid, Duration::from_secs(8))?;
        net.inject(leader, endpoints)?;
        barrier.go() // rules-before-usable: release the workload ONLY after inject succeeds
    })();

    match setup {
        Ok(()) => {
            let code = child.wait()?.code().unwrap_or(-1);
            net.teardown();
            Ok(code)
        }
        Err(e) => {
            barrier.abort();
            net.teardown();
            let _ = child.wait();
            eprintln!("gatekeeperd/sandbox: FAIL egress setup: {e} — failed closed (no network)");
            Err(e)
        }
    }
}

/// Outcome of the privileged re-check (isolation.md §7 steps 4–5) plus this slice's constructibility
/// gate. `Construct` carries the effective tier we are cleared to build (T0 folds up to T1);
/// `Refuse` carries a distinct exit code + an audit reason.
enum Decision {
    Construct { effective: Tier, egress: Egress },
    Refuse { code: i32, reason: String },
}

/// The independent re-check — gatekeeperd trusts NONE of agentd's numbers. It recomputes the tier
/// bound from the COMPILED-IN `shrek-policy` matrix/floor (sealed by dm-verity in the shipped `/usr`,
/// NOT read from agentd or any writable state — isolation.md §7, security-model.md §4). This proves
/// the arithmetic/floor independence: a bug or compromise in the unprivileged resolver cannot widen
/// a sandbox. (Integrity-sourcing the trust band ITSELF is OPEN B1, a separate upstream slice; here
/// `trust`/`caps` still ride in with the request, and the fail-high parse guarantees a garbled band
/// only ever raises the wall.)
fn recheck(
    requested: Tier,
    trust: TrustBand,
    caps: CapsProfile,
    profile: CapsProfile,
    egress_name: Option<&str>,
) -> Decision {
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
    // (c) Constructibility. T0 (slice-4), T1 (slice-1/3), and T2 (slice-6, gVisor) build; T3 has no
    // constructor yet and FAILS CLOSED — never silently downgraded (security-model.md §7). T2 never
    // falls DOWN to T1: it is the floor for T-untrust/T-hostile.
    if effective >= Tier::T3 {
        return Decision::Refuse {
            code: 12,
            reason: format!("no-constructor-{} (slice-6: T3 pending)", effective.label()),
        };
    }
    // T2 serves the no-egress caps only. A C-net cell at T2 needs the gVisor egress plane (deferred),
    // so it fails closed here — NOT constructed via the T1 egress plane below (wrong wall).
    if effective == Tier::T2 && matches!(caps, CapsProfile::Net) {
        return Decision::Refuse {
            code: 12,
            reason: "no-gvisor-egress-plane-for-C-net-at-T2 (slice-6)".into(),
        };
    }
    // (d) Egress realization (slice-3). A ≤T1 C-net cell is now constructible IFF it names a SEALED
    // egress profile that resolves — gatekeeperd resolves the destinations from compiled-in policy
    // itself, NEVER trusting an agentd-supplied host. C-broad (unrestricted egress / secret domains)
    // has no plane and still fails closed. Non-egress caps ignore any profile name → loopback-only.
    match caps {
        CapsProfile::Broad => Decision::Refuse {
            code: 13,
            reason: "no-plane-for-C-broad (unrestricted egress)".into(),
        },
        CapsProfile::Net => match egress_name {
            None => Decision::Refuse { code: 13, reason: "C-net-requires-egress-profile".into() },
            Some(name) => match shrek_policy::egress::resolve(name) {
                Some(p) => Decision::Construct { effective, egress: Egress::Profile(p) },
                None => Decision::Refuse { code: 13, reason: format!("unknown-egress-profile={name}") },
            },
        },
        // effective ∈ {T0, T1}, caps ∈ {C-ro-nosec, C-proj-rw}: build at T1, loopback-only. A T0
        // result at T1 is a legal upward escalation until the real T0 (Landlock) constructor lands.
        CapsProfile::RoNosec | CapsProfile::ProjRw => {
            Decision::Construct { effective, egress: Egress::None }
        }
    }
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
    let mut egress_s: Option<String> = None;
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
            "--egress-profile" => { i += 1; egress_s = args.get(i).cloned(); }
            "--" => { workload = args[i + 1..].to_vec(); break; }
            other => { eprintln!("gatekeeperd/sandbox: unknown arg {other}"); return 2; }
        }
        i += 1;
    }
    if grants.is_empty() || workload.is_empty() {
        eprintln!("usage: gatekeeperd sandbox [--tier Tn --trust T --caps C [--profile C] [--egress-profile NAME]] --anchor DIR --grant NAME [...] -- WORKLOAD...");
        return 2;
    }

    // Decision-plane mode (slice-2/3): re-check the resolved request before touching the mount plane.
    // Slice-1 direct mode (no --tier) constructs loopback-only (Egress::None), unchanged.
    let mut egress = Egress::None;
    if let Some(t) = tier_s {
        let Some(requested) = Tier::parse(&t) else {
            eprintln!("SANDBOX-DECISION refused reason=bad-request-tier={t:?}");
            return 14;
        };
        // Fail-high on the caps axis; a missing profile defaults to the requested caps.
        let caps = CapsProfile::parse(caps_s.as_deref().unwrap_or(""));
        let profile = profile_s.as_deref().map(CapsProfile::parse).unwrap_or(caps);
        let egress_name = egress_s.as_deref();
        // Slice-7 (B1): the trust band is DERIVED from a measurement of the workload entrypoint, never
        // taken from the caller. `--trust` is demoted to a NON-AUTHORITATIVE proposal — recorded for
        // audit + mismatch detection ONLY, and it NEVER influences the effective band. gatekeeperd's
        // derivation is the sole authority, extending the slice-2 independent re-check to the last
        // caller-asserted input (ADV-8; docs §6). No override: an unsealed/foreign entrypoint that
        // proposes `T-first` is corrected down to `T-hostile` here, before the tier arithmetic.
        let proposed = TrustBand::parse(trust_s.as_deref().unwrap_or(""));
        let mut der = provenance_plane::derive(workload.first(), provenance_plane::sealed_root_dev());
        let trust = der.band;
        eprintln!(
            "SANDBOX-PROVENANCE derived={} proposed={} match={} entrypoint={:?} entrypoint_sealed={} domain_execution_sealed={} pinned={} exec_fd_bound={} sealed_root={:?}",
            trust.label(),
            proposed.label(),
            trust == proposed,
            der.entrypoint,
            der.evidence.entrypoint_sealed,
            der.evidence.domain_execution_sealed,
            der.evidence.pinned_digest_match,
            der.exec_fd.is_some(), // the measured fd is bound to a T-pinned derivation (slice-8)
            der.sealed_root
        );
        match recheck(requested, trust, caps, profile, egress_name) {
            Decision::Refuse { code, reason } => {
                eprintln!(
                    "SANDBOX-DECISION refused reason={reason} requested={} trust={} caps={} profile={} egress={}",
                    requested.label(), trust.label(), caps.label(), profile.label(), egress_name.unwrap_or("-")
                );
                return code;
            }
            Decision::Construct { effective, egress: e } => {
                // slice-9: a `T-pinned` static-PIE artifact now gets a T0 EXEC ISLAND — the only place
                // pinned third-party bytes may run. Preconditions (else fail closed, rc=15, I4): the
                // cell resolves to T0 (Fork B: floor(Pinned)=T0, shrek-policy frozen), gatekeeperd holds
                // the measured entrypoint fd (`der.exec_fd`), and Landlock is enforceable at a clean
                // preflight. A `T-pinned` build NEVER falls up to T1 (≥T1 pinned containment is a
                // documented Fork-B follow-up, not v1) and NEVER falls down — any miss refuses exactly
                // as slice-8 did (`pinned-exec-home-unavailable`). The island reopens NOTHING for grants
                // or /usr: mutable grants stay `MS_NOEXEC`, only the re-verified pinned inode gains exec.
                if trust == TrustBand::Pinned {
                    let exec_fd_bound = der.exec_fd.is_some();
                    let pf = if effective == Tier::T0 { Some(proc_plane::preflight()) } else { None };
                    if let (Tier::T0, Some(proc_plane::Preflight::Ready { abi }), Some(exec_fd)) =
                        (effective, pf, der.exec_fd.take())
                    {
                        eprintln!(
                            "SANDBOX-DECISION cleared construct-at=T0 island=exec effective=T0 requested={} trust={} caps={} profile={} landlock-abi={abi}",
                            requested.label(), trust.label(), caps.label(), profile.label()
                        );
                        let t0 = T0Spec {
                            id: id.clone(),
                            anchor: anchor.clone(),
                            grants: grants.clone(),
                            workload: workload.clone(),
                            abi,
                            mem_max: DEFAULT_MEM_MAX,
                            pids_max: DEFAULT_PIDS_MAX,
                            exec_island: Some(exec_fd),
                        };
                        return match proc_plane::construct(&t0) {
                            Ok(code) => code,
                            Err(e) => {
                                eprintln!("gatekeeperd/proc_plane: FAIL island construction (fail-closed, no fall-up/down): {e}");
                                3
                            }
                        };
                    }
                    eprintln!(
                        "SANDBOX-DECISION refused reason=pinned-exec-home-unavailable effective={} trust={} caps={} profile={} exec_fd_bound={exec_fd_bound}",
                        effective.label(), trust.label(), caps.label(), profile.label()
                    );
                    return 15;
                }
                egress = e;
                let egr = match e { Egress::Profile(p) => p.name, Egress::None => "none" };
                // Slice-6: an effective==T2 cell builds at genuine T2 via gVisor/runsc. No fall-DOWN:
                // T2 is the floor for T-untrust/T-hostile, so a construction failure fails CLOSED and
                // never degrades to T1. Platform (systrap/kvm) is chosen once here (both genuine T2).
                if effective == Tier::T2 {
                    let choice = t2_plane::select_platform();
                    eprintln!(
                        "SANDBOX-DECISION cleared construct-at=T2 effective=T2 platform={} why=\"{}\" requested={} trust={} caps={} profile={} egress=none",
                        choice.platform.flag(), choice.why, requested.label(), trust.label(), caps.label(), profile.label()
                    );
                    let t2 = T2Spec {
                        id: id.clone(),
                        anchor: anchor.clone(),
                        grants: grants.clone(),
                        workload: workload.clone(),
                        rootfs: t2_plane::sealed_rootfs_path(),
                        runsc: t2_plane::sealed_runsc_path(),
                        platform: choice.platform,
                        mem_max: DEFAULT_MEM_MAX,
                        pids_max: DEFAULT_PIDS_MAX,
                    };
                    return match t2_plane::construct(&t2) {
                        Ok(code) => code,
                        Err(err) => {
                            eprintln!("gatekeeperd/t2_plane: FAIL construction (fail-closed, no fall-down): {err}");
                            3
                        }
                    };
                }
                // Slice-4: an effective==T0 cell now builds at GENUINE T0 (Landlock+seccomp+ns+cgroup).
                // Fall-up to the stronger T1 wall is permitted ONLY here, from a clean Landlock
                // preflight — a legal upward escalation, never a downgrade, and loudly audited. Any
                // failure once proc_plane::construct starts fails closed (no fall-up mid-build).
                if effective == Tier::T0 {
                    match proc_plane::preflight() {
                        proc_plane::Preflight::Ready { abi } => {
                            eprintln!(
                                "SANDBOX-DECISION cleared construct-at=T0 effective=T0 requested={} trust={} caps={} profile={} landlock-abi={abi}",
                                requested.label(), trust.label(), caps.label(), profile.label()
                            );
                            let t0 = T0Spec {
                                id: id.clone(),
                                anchor: anchor.clone(),
                                grants: grants.clone(),
                                workload: workload.clone(),
                                abi,
                                mem_max: DEFAULT_MEM_MAX,
                                pids_max: DEFAULT_PIDS_MAX,
                                exec_island: None,
                            };
                            return match proc_plane::construct(&t0) {
                                Ok(code) => code,
                                Err(e) => {
                                    eprintln!("gatekeeperd/proc_plane: FAIL construction: {e}");
                                    3
                                }
                            };
                        }
                        proc_plane::Preflight::Unavailable(why) => {
                            eprintln!(
                                "SANDBOX-DECISION cleared construct-at=T1 effective=T0 fell-up reason=landlock-unavailable ({why}) requested={} trust={} caps={} profile={} egress={egr}",
                                requested.label(), trust.label(), caps.label(), profile.label()
                            );
                            // fall through: build the same cell at the stronger T1 nspawn wall.
                        }
                    }
                } else {
                    eprintln!(
                        "SANDBOX-DECISION cleared construct-at=T1 effective={} requested={} trust={} caps={} profile={} egress={egr}",
                        effective.label(), requested.label(), trust.label(), caps.label(), profile.label()
                    );
                }
                // fall through to T1 construction (effective==T1, or a T0 cell that fell up)
            }
        }
    }

    let spec = SandboxSpec { id, anchor, grants, guest_prefix, workload, egress };
    match construct(&spec) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("gatekeeperd/sandbox: FAIL construction: {e}");
            3
        }
    }
}

#[cfg(test)]
mod tests {
    //! The independent re-check arithmetic, exercised with SYNTHETIC trust bands — the pure seam for
    //! decision-plane assertions (slice-7 moved these off the production `sandbox` CLI, whose band is
    //! now DERIVED, not caller-asserted, so synthetic `T-pinned`/`T-untrust` inputs no longer reach
    //! the constructor). `recheck` is pure (no I/O), so every refusal code and the upward-only
    //! escalation are asserted here without a kernel. Mirrors the checks the old `tier-plane-repro`
    //! drove through `--trust`.
    use super::*;
    use shrek_policy::{CapsProfile::*, TrustBand::*};

    fn refusal(d: Decision) -> (i32, String) {
        match d {
            Decision::Refuse { code, reason } => (code, reason),
            Decision::Construct { .. } => panic!("expected refusal, got construct"),
        }
    }

    #[test]
    fn forged_downgrade_below_floor_is_refused() {
        // T-hostile/C-net floors at T3; a request for T0 is a forbidden downgrade (code 10).
        let (code, reason) = refusal(recheck(Tier::T0, Hostile, Net, Net, None));
        assert_eq!(code, 10);
        assert!(reason.contains("downgrade-below-floor") && reason.contains("T3"));
    }

    #[test]
    fn caps_exceed_profile_is_refused() {
        // caps C-net ⊄ profile C-proj-rw (code 11), re-checked independently of the resolver.
        let (code, _) = refusal(recheck(Tier::T1, First, Net, ProjRw, None));
        assert_eq!(code, 11);
    }

    #[test]
    fn t2_c_net_has_no_gvisor_egress_plane() {
        // T-untrust/C-net resolves to T2, which has no gVisor egress plane yet ⇒ fail closed (code 12),
        // never built at the T1 egress plane (wrong wall).
        let (code, reason) = refusal(recheck(Tier::T2, Untrust, Net, Net, None));
        assert_eq!(code, 12);
        assert!(reason.contains("gvisor-egress"));
    }

    #[test]
    fn t3_has_no_constructor() {
        let (code, reason) = refusal(recheck(Tier::T3, Hostile, ProjRw, ProjRw, None));
        assert_eq!(code, 12);
        assert!(reason.contains("no-constructor-T3"));
    }

    #[test]
    fn c_net_below_t2_requires_an_egress_profile() {
        // T-first/C-net = T1; C-net with no named egress profile fails closed (code 13).
        let (code, _) = refusal(recheck(Tier::T1, First, Net, Net, None));
        assert_eq!(code, 13);
    }

    #[test]
    fn c_broad_has_no_plane() {
        let (code, reason) = refusal(recheck(Tier::T1, First, Broad, Broad, None));
        assert_eq!(code, 13);
        assert!(reason.contains("C-broad"));
    }

    #[test]
    fn upward_escalation_is_honored_not_a_downgrade() {
        // T-first/C-ro-nosec floors at T0; a request for the STRONGER T2 wall is a legal upward
        // escalation and constructs at T2 (no min anywhere).
        match recheck(Tier::T2, First, RoNosec, RoNosec, None) {
            Decision::Construct { effective, .. } => assert_eq!(effective, Tier::T2),
            Decision::Refuse { code, reason } => panic!("unexpected refusal {code}: {reason}"),
        }
    }

    #[test]
    fn cleared_t1_constructs_loopback() {
        // The nominal cleared path: T-pinned/C-proj-rw = T1, loopback-only.
        match recheck(Tier::T1, Pinned, ProjRw, ProjRw, None) {
            Decision::Construct { effective, egress } => {
                assert_eq!(effective, Tier::T1);
                assert!(matches!(egress, Egress::None));
            }
            Decision::Refuse { code, reason } => panic!("unexpected refusal {code}: {reason}"),
        }
    }
}
