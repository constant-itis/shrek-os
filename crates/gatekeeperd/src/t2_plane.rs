//! t2_plane — the T2 (gVisor / `runsc`) sandbox constructor. Phase-5 slice-6
//! (docs/phase5-slice6-t2.md).
//!
//! T2 is the FLOOR for the two least-trusted bands — `floor(T-untrust)=T2`, `floor(T-hostile)=T2`
//! (isolation.md §5.1) — which every prior slice refused ("no-constructor ≥ T2"). This module is
//! that constructor. It serves the no-egress caps only (`C-ro-nosec`, `C-proj-rw`); a `C-net` cell at
//! T2 still fails closed until the gVisor egress plane lands (deferred, see the slice doc).
//!
//! Unlike T0/T1, gatekeeperd does NOT hand-roll the sandbox: **`runsc` is the constructor.** Its
//! Sentry is the userspace kernel the workload sees — a sandboxed `mount(2)` (or any disallowed op)
//! is answered by gVisor's own implementation, NOT a host seccomp filter. gatekeeperd's job is to
//! prepare the inputs and drive the OCI lifecycle: pick the platform, generate the bundle, place the
//! run under a bounded cgroup leaf, run it with `--network=none` (loopback-only — the deferred-egress
//! posture), and tear everything down on every exit path.
//!
//! Caps are realized exactly as at T1 — by ABSENCE, not deny: a grant is a `mounts[]` entry, the
//! vault/ungranted paths are simply omitted, so they are not present in the sandbox's (empty) mount
//! namespace (ENOENT), not merely unreadable.
//!
//! Fail-closed, no fall-DOWN: T2 is the floor, so a construction failure NEVER degrades to T1 (that
//! is below the floor). Platform selection (`systrap` vs `kvm`) is decided once at preflight; both are
//! genuine T2, so choosing between them is not a tier change. Any failure once `runsc` is invoked
//! fails closed with no workload run.

use crate::linux_uapi::{
    ioctl, open_rdwr, umount2, KVM_API_VERSION, KVM_CREATE_VM, KVM_GET_API_VERSION,
};
use crate::mount_plane::{
    enter_private_mount_ns, open_anchor, pin_beneath, relocate_rw, relocate_rw_exec, stage_tmpfs,
};
use crate::net_plane;
use shrek_policy::egress::EgressProfile;
use std::io::{self, Write};
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const MNT_DETACH: i32 = 2;

/// Default cgroup-v2 bounds — a liveness/DoS ceiling, not a security wall. Overridable per spec.
pub const DEFAULT_MEM_MAX: u64 = 512 * 1024 * 1024;
pub const DEFAULT_PIDS_MAX: u64 = 256;

/// Production paths for the verity-sealed artifacts (docs §Shipping / image/supply/gvisor.pin). The
/// `SHREK_T2_*` env overrides exist ONLY for the oracle (spike), which supplies a fetched runsc + a
/// throwaway rootfs; production reads the compiled-in defaults, which live under dm-verity `/usr`, so
/// the constructor introduces no writable authority source.
pub fn sealed_runsc_path() -> PathBuf {
    std::env::var_os("SHREK_T2_RUNSC").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/usr/lib/shrek/runsc"))
}
pub fn sealed_rootfs_path() -> PathBuf {
    std::env::var_os("SHREK_T2_ROOTFS").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/usr/lib/shrek/t2-rootfs"))
}

// -------------------------------------------------------------------------------------------------
// Platform selection — DECIDED: Option B (prefer systrap on nested/VM hosts; kvm only bare-metal).
// -------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    /// Userspace syscall trapping (seccomp SECCOMP_RET_TRAP). No virt requirement — gVisor's default
    /// and the recommended choice inside a (nested) VM. Our sealed-VM target lands here.
    Systrap,
    /// Host KVM. Faster on bare-metal, but nested KVM is slower AND adds hypervisor attack surface,
    /// so it is reserved for a genuine bare-metal host where the usability probe passes.
    Kvm,
}

impl Platform {
    /// The `--platform=` flag value runsc expects.
    pub fn flag(self) -> &'static str {
        match self {
            Platform::Systrap => "systrap",
            Platform::Kvm => "kvm",
        }
    }
}

/// The chosen platform plus a human rationale for the audit log.
pub struct PlatformChoice {
    pub platform: Platform,
    pub why: String,
}

/// Pure policy (Option B), factored out so it is unit-testable without touching hardware: KVM only
/// when we are authoritatively NOT virtualized AND the KVM probe succeeded; otherwise systrap. The
/// KVM_CREATE_VM probe already subsumes "CPU has virt extensions", so virt-ext is not a decision
/// input — only diagnostic. Both outcomes are genuine T2.
fn platform_decision(virtualized: bool, kvm_usable: bool) -> Platform {
    if !virtualized && kvm_usable {
        Platform::Kvm
    } else {
        Platform::Systrap
    }
}

/// Authoritative virtualization detection via `systemd-detect-virt` (multi-signal: DMI, CPUID,
/// container cgroups, …) — the CPUID `hypervisor` flag ALONE is insufficient (a VM can hide it, e.g.
/// `-cpu host,-hypervisor`), and trusting it could misclassify a VM as bare metal and wrongly pick
/// KVM. Returns `(virtualized, detail)`. If the detector is unavailable we return `virtualized=true`
/// — the safe default: KVM is only a bare-metal perf win, systrap is always a valid T2 wall, so when
/// we cannot CONFIRM bare metal we do not risk KVM. (Shrek ships systemd, so production has it.)
fn detect_virtualized() -> (bool, String) {
    match Command::new("systemd-detect-virt").output() {
        Ok(out) => {
            let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
            // Exit 0 + non-"none" ⇒ virtualization (vm or container) detected. Exit non-zero / "none"
            // ⇒ bare metal. Any parse ambiguity biases to virtualized (safe).
            if id.is_empty() {
                (true, "detect-virt-empty".into())
            } else if id == "none" {
                (false, "none".into())
            } else {
                (true, id)
            }
        }
        Err(_) => (true, "detect-virt-unavailable".into()),
    }
}

/// True if `/proc/cpuinfo`'s `flags` list contains `flag` as a whole token. Pure over the text.
fn cpuinfo_has_flag(cpuinfo: &str, flag: &str) -> bool {
    cpuinfo
        .lines()
        .filter(|l| l.starts_with("flags"))
        .any(|l| l.split_whitespace().any(|t| t == flag))
}

/// DIAGNOSTIC ONLY (not a decision input): does the CPU advertise KVM-usable virt extensions? Logged
/// alongside the platform choice; the actual gate is `detect_virtualized()` + the KVM_CREATE_VM probe.
fn has_virt_ext() -> bool {
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    cpuinfo_has_flag(&cpuinfo, "vmx") || cpuinfo_has_flag(&cpuinfo, "svm")
}

/// The DECISIVE KVM usability probe (docs §Platform): open `/dev/kvm` O_RDWR, require
/// `KVM_GET_API_VERSION == 12`, then require `KVM_CREATE_VM` to actually return a VM fd. A nested host
/// frequently has the device node and the right API version yet cannot create a VM — only the
/// `KVM_CREATE_VM` ioctl proves construction. Any failure ⇒ "not usable" (systrap).
fn kvm_usable() -> bool {
    let fd = match open_rdwr(c"/dev/kvm") {
        Ok(f) => f,
        Err(_) => return false, // ENOENT / EACCES — no usable KVM
    };
    use std::os::fd::AsRawFd;
    match ioctl(fd.as_raw_fd(), KVM_GET_API_VERSION, 0) {
        Ok(v) if v == KVM_API_VERSION => {}
        _ => return false, // absent, or an API version we must refuse
    }
    match ioctl(fd.as_raw_fd(), KVM_CREATE_VM, 0) {
        Ok(vmfd) if vmfd >= 0 => {
            // Close the VM fd we just created (auto-drop). Its mere creation is the proof.
            let _vm = unsafe { OwnedFd::from_raw_fd(vmfd as RawFd) };
            true
        }
        _ => false, // device present but VM creation refused (the nested-host case)
    }
}

/// Select the platform for a T2 construction and explain why. Never fails: systrap is the floor
/// (no host virt requirement). This is the ONE place the platform is decided — see the fail-closed
/// note in the module doc: no mid-construct platform switch.
pub fn select_platform() -> PlatformChoice {
    let (virt, vdetail) = detect_virtualized();
    // Only probe KVM when it could possibly be chosen (authoritatively bare metal) — `&&`
    // short-circuits, so a VM never issues the ioctls. `platform_decision` stays the single source.
    let kvm = !virt && kvm_usable();
    let platform = platform_decision(virt, kvm);
    let virt_ext = has_virt_ext(); // diagnostic only
    let why = if virt {
        format!("virtualized ({vdetail}) → systrap [virt-ext={virt_ext}]")
    } else if !kvm {
        format!("bare-metal, kvm probe failed → systrap [virt-ext={virt_ext}]")
    } else {
        format!("bare-metal ({vdetail}) + kvm usable → kvm")
    };
    PlatformChoice { platform, why }
}

// -------------------------------------------------------------------------------------------------
// cgroup-v2 leaf — bounds the runsc sandbox (the runsc CHILD joins it via pre_exec; --ignore-cgroups
// means runsc does not manage cgroups itself). Same delegation dance as proc_plane's CgroupScope:
// the base cgroup must have no internal process before it can delegate controllers, so gatekeeperd
// vacates itself into `_daemon`, enables +memory +pids, then creates the sandbox leaf.
// -------------------------------------------------------------------------------------------------

struct CgroupLeaf {
    leaf: PathBuf,
}

fn cg_write(path: &Path, val: &str) -> io::Result<()> {
    let mut f = std::fs::OpenOptions::new().write(true).open(path)?;
    f.write_all(val.as_bytes())
}

impl CgroupLeaf {
    fn create(id: &str, mem_max: u64, pids_max: u64) -> io::Result<CgroupLeaf> {
        let rel = std::fs::read_to_string("/proc/self/cgroup")?
            .lines()
            .find_map(|l| l.strip_prefix("0::").map(|s| s.to_string()))
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "no unified cgroup for self"))?;
        let base = PathBuf::from("/sys/fs/cgroup").join(rel.trim_start_matches('/'));

        // Vacate `base`: move self to base/_daemon so base can delegate controllers to its children.
        let daemon = base.join("_daemon");
        let _ = std::fs::create_dir(&daemon);
        cg_write(&daemon.join("cgroup.procs"), &format!("{}\n", std::process::id()))?;
        let _ = cg_write(&base.join("cgroup.subtree_control"), "+memory +pids");

        let leaf = base.join(format!("shrek-t2-{id}"));
        let _ = std::fs::remove_dir(&leaf);
        std::fs::create_dir(&leaf)?;
        cg_write(&leaf.join("memory.max"), &format!("{mem_max}\n"))?;
        cg_write(&leaf.join("pids.max"), &format!("{pids_max}\n"))?;
        Ok(CgroupLeaf { leaf })
    }

    fn procs_path(&self) -> PathBuf {
        self.leaf.join("cgroup.procs")
    }

    fn destroy(&self) {
        let _ = std::fs::remove_dir(&self.leaf);
    }
}

// -------------------------------------------------------------------------------------------------
// OCI bundle
// -------------------------------------------------------------------------------------------------

/// A single granted subtree mounted into the sandbox. `name` is the leaf beneath the anchor; it appears
/// inside the sandbox at `/srv/<name>` (mirroring the T1 guest_prefix). Ungranted siblings and the
/// vault are simply NOT listed → absent (ENOENT), the T1/T2 absence model. `rw` selects the OCI mount
/// mode: read-only grants (`rbind,ro`) vs the single writable project grant (`rbind,rw`, Phase-6
/// slice-1a) whose `source` is a broker-relocated, host-`noexec` bind so the workload's writes
/// write-through to the real project inode.
struct GrantMount {
    name: String,
    source: PathBuf,
    rw: bool,
}

pub struct T2Spec {
    pub id: String,
    /// Trusted anchor directory under which grants live (e.g. `/srv`).
    pub anchor: PathBuf,
    pub grants: Vec<String>,
    /// Phase-6 slice-1a: the single project grant realized WRITABLE (`C-proj-rw`). Pinned beneath the
    /// anchor, relocated with `relocate_rw` (rw + host-`noexec`), and bound `rbind,rw` so a coding
    /// workload builds/tests/edits with write-through. `None` = every grant read-only (the slice-6
    /// posture). Must NOT also appear in `grants`.
    pub rw_grant: Option<String>,
    /// Phase-6 slice-1a: the SEPARATE, narrowly-scoped build grant realized WRITABLE **and EXEC-capable**
    /// (`relocate_rw_exec`: rw + `nosuid,nodev`, NO host-`noexec`). A coding workload directs its
    /// compiler/test OUTPUT here (`CARGO_TARGET_DIR`, freshly-compiled binaries) and RUNS it — gVisor
    /// needs a non-`noexec` host mount to `mmap(PROT_EXEC)` the gofer file. The project (`rw_grant`) stays
    /// host-`noexec`, so its source bytes are never runnable in-sandbox; the exec surface is confined to
    /// this one grant. `None` = no build area. Must NOT also appear in `grants` or equal `rw_grant`.
    pub build_grant: Option<String>,
    pub workload: Vec<String>,
    /// Absolute path to the pinned, verity-sealed minimal rootfs (root.path, read-only).
    pub rootfs: PathBuf,
    /// Absolute path to the pinned, verity-sealed `runsc` binary.
    pub runsc: PathBuf,
    pub platform: Platform,
    pub mem_max: u64,
    pub pids_max: u64,
    /// Phase-6 slice-1b: the sealed egress profile for a `(T-untrust, C-net)` coding session, or
    /// `None` for loopback-only (`--network=none`, the slice-1a/slice-6 posture). `Some(profile)` ⇒
    /// gatekeeperd PRE-CREATES a per-sandbox netns (veth + addressing + the profile's nft allow-list),
    /// hands runsc that netns via the OCI `network` namespace, and runs `--network=sandbox` so gVisor's
    /// netstack egresses ONLY to the resolved endpoints. The name is sealed policy; destinations are
    /// resolved to pinned IPv4 here and written into the sandbox `/etc/hosts` (no DNS egress).
    pub egress: Option<&'static EgressProfile>,
}

/// Proper JSON string escaping: backslash, double-quote, and control characters (a raw newline/tab in
/// a workload arg would otherwise produce invalid JSON). Bundle paths are gatekeeper-controlled, but
/// workload argv can be an arbitrary script string, so escape defensively.
fn json_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

/// Build the OCI `config.json` for a minimal T2 sandbox: read-only rootfs, the workload argv, the
/// grant bind-mounts (rbind,ro), and an empty-but-for-grants mount namespace. `netns_path` (slice-1b)
/// adds a `network` namespace pointing at the gatekeeper-provisioned netns so runsc joins it and
/// `--network=sandbox` programs netstack from its veth; `None` lists no network namespace (the
/// `--network=none` loopback-only posture). Pure over the spec — unit-tested for shape.
fn build_config_json(spec: &T2Spec, grants: &[GrantMount], rootfs: &Path, netns_path: Option<&str>) -> String {
    let args = spec
        .workload
        .iter()
        .map(|a| format!("\"{}\"", json_escape(a)))
        .collect::<Vec<_>>()
        .join(",");
    let mounts = grants
        .iter()
        .map(|g| {
            let mode = if g.rw { "rw" } else { "ro" };
            format!(
                "{{\"destination\":\"/srv/{}\",\"type\":\"bind\",\"source\":\"{}\",\"options\":[\"rbind\",\"{mode}\"]}}",
                json_escape(&g.name),
                json_escape(&g.source.to_string_lossy())
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    // pid/mount/ipc/uts always; slice-1b appends a network namespace (joined to the provisioned
    // netns) ONLY when egress is granted — a no-egress session lists none and runs --network=none.
    let mut namespaces =
        String::from("{\"type\":\"pid\"},{\"type\":\"mount\"},{\"type\":\"ipc\"},{\"type\":\"uts\"}");
    if let Some(p) = netns_path {
        namespaces.push_str(&format!(",{{\"type\":\"network\",\"path\":\"{}\"}}", json_escape(p)));
    }
    format!(
        concat!(
            "{{\"ociVersion\":\"1.0.2\",",
            "\"process\":{{\"args\":[{args}],\"cwd\":\"/\",\"user\":{{\"uid\":0,\"gid\":0}},\"capabilities\":{{}}}},",
            "\"root\":{{\"path\":\"{rootfs}\",\"readonly\":true}},",
            "\"mounts\":[{mounts}],",
            "\"linux\":{{\"namespaces\":[{namespaces}]}}}}"
        ),
        args = args,
        rootfs = json_escape(&rootfs.to_string_lossy()),
        mounts = mounts,
        namespaces = namespaces,
    )
}

// -------------------------------------------------------------------------------------------------
// Construction
// -------------------------------------------------------------------------------------------------

/// Construct and run one T2 (gVisor) sandbox. Returns the workload's exit code. MUST be called as
/// host root (rootless runsc cannot do the create/start lifecycle nor netstack). Fails closed on any
/// error — never degrades to T1.
pub fn construct(spec: &T2Spec) -> io::Result<i32> {
    if spec.workload.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty workload"));
    }
    if spec.workload[0].is_empty() || !spec.workload[0].starts_with('/') {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "workload argv[0] must be absolute"));
    }
    if !spec.runsc.is_file() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("runsc not found: {}", spec.runsc.display())));
    }
    if !spec.rootfs.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("rootfs not found: {}", spec.rootfs.display())));
    }

    let bad_name = |name: &str| name.is_empty() || name.contains('/') || name == "..";

    // Resolve read-only grant sources beneath the anchor (name-only leaves; reject traversal).
    let mut grants = Vec::new();
    for name in &spec.grants {
        if bad_name(name) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("bad grant name: {name}")));
        }
        grants.push(GrantMount { name: name.clone(), source: spec.anchor.join(name), rw: false });
    }
    if let Some(rw) = &spec.rw_grant {
        if bad_name(rw) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("bad rw-grant name: {rw}")));
        }
        if spec.grants.iter().any(|g| g == rw) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("rw-grant {rw} also listed read-only")));
        }
    }
    if let Some(bg) = &spec.build_grant {
        if bad_name(bg) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("bad build-grant name: {bg}")));
        }
        if spec.grants.iter().any(|g| g == bg) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("build-grant {bg} also listed read-only")));
        }
        if spec.rw_grant.as_deref() == Some(bg.as_str()) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("build-grant {bg} also the rw project grant")));
        }
    }

    // Working dirs: state (runsc --root) + bundle (config.json). Both under /run, cleaned on exit.
    let work = PathBuf::from(format!("/run/shrek-t2/{}", spec.id));
    let bundle = work.join("bundle");
    let state = work.join("state");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&bundle)?;
    std::fs::create_dir_all(&state)?;

    // Phase-6 slice-1a — the writable grants. In a PRIVATE mount namespace (so the binds are contained +
    // non-propagating, and the runsc child inherits them), pin each grant beneath the anchor TOCTOU-safely
    // and relocate it onto a broker-owned path. The gofer serves the source rw, so the workload's writes
    // reach the real inode (write-through). Two DIFFERENT exec postures:
    //   * project (`rw_grant`)   → `relocate_rw`      : rw + host-`noexec`. The real-ELF result is that a
    //     binary written here CANNOT run in-sandbox (gVisor needs PROT_EXEC of the gofer file, denied by
    //     the noexec host mount) — so the project stays a no-exec source tree.
    //   * build   (`build_grant`)→ `relocate_rw_exec` : rw + exec. The workload directs compiler/test
    //     OUTPUT here (CARGO_TARGET_DIR / a compiled binary) and RUNS it. The exec surface is confined to
    //     this one grant; `nosuid,nodev` preserved on both.
    // Any failure fails the whole construction closed — never a silent read-only or unpinned substitute.
    //
    // Enter ONE private mount namespace for the WHOLE construction (unconditionally — was grant-only):
    // both the rootfs-staging tmpfs (below) and any writable-grant relocations live in it, contained +
    // non-propagating, and the runsc child inherits them. The staging tmpfs is needed on EVERY T2
    // construct — including the grant-less egress/no-net path — so the private ns can no longer be
    // gated on grants.
    enter_private_mount_ns()?;

    // pin+relocate each present grant within that ns.
    if spec.rw_grant.is_some() || spec.build_grant.is_some() {
        let anchor_fd = open_anchor(&spec.anchor)?;
        if let Some(rw) = &spec.rw_grant {
            let pinned = pin_beneath(&anchor_fd, rw)?;
            let target = work.join("rwgrants").join(rw);
            relocate_rw(&pinned, &target)?;
            eprintln!(
                "gatekeeperd/t2_plane: pinned+relocated-rw grant {rw} (dev={}:{} ino={}) -> {} (rw,noexec write-through)",
                pinned.ident.dev_major, pinned.ident.dev_minor, pinned.ident.ino, target.display()
            );
            grants.push(GrantMount { name: rw.clone(), source: target, rw: true });
        }
        if let Some(bg) = &spec.build_grant {
            let pinned = pin_beneath(&anchor_fd, bg)?;
            let target = work.join("rwgrants").join(bg);
            relocate_rw_exec(&pinned, &target)?;
            eprintln!(
                "gatekeeperd/t2_plane: pinned+relocated-rw-exec build grant {bg} (dev={}:{} ino={}) -> {} (rw,exec write-through)",
                pinned.ident.dev_major, pinned.ident.dev_minor, pinned.ident.ino, target.display()
            );
            grants.push(GrantMount { name: bg.clone(), source: target, rw: true });
        }
    }

    // gVisor's gofer creates each bind-mount DESTINATION dir inside the rootfs tree on the host
    // (mkdir <root.path>/srv/<name>) as part of `setupRootFS`, BEFORE any guest-visible overlay — so
    // `--overlay2` does not help. The sealed rootfs is on read-only dm-verity /usr, so that mkdir fails
    // EROFS and the sandbox never starts ("waiting for sandbox to start: EOF"). Give the gofer a
    // WRITABLE per-sandbox rootfs: copy the tiny sealed rootfs (busybox + relative applet symlinks,
    // ~2MB) into tmpfs /run. `cp -a` preserves the RELATIVE symlinks (following them would break
    // /bin/sh inside the sandbox). The copy is made fresh from the verity-authenticated source each
    // construction (integrity preserved) and discarded on teardown; the guest still sees it read-only
    // (config readonly:true). A verity-lower/tmpfs-upper overlay + warm pool is the deferred scaling
    // optimization (docs §Deferred) — a per-construct copy is right for the minimal T2 floor.
    //
    // slice-6 (P6-1c) — stage the rootfs on a FRESH, gatekeeper-owned EXEC tmpfs rather than letting it
    // inherit `/run`'s mount policy. On a hardened host (Pop!_OS/CIS default) `/run` is `noexec`, which
    // makes gVisor's setup-root remount-ro fail `EPERM` and the guest binary's `PROT_EXEC` map SIGSEGV
    // (rc139) — platform-independent, not a kvm/egress defect. A default `tmpfs` is exec (nosuid,nodev
    // preserved), so staging here decouples the guest's exec path from `/run`'s flags. Mounted in the
    // private mount ns entered above, so the runsc child inherits it and it never touches the host mount
    // table; torn down (MNT_DETACH) in the exit path before the work tree is removed.
    let run_rootfs = work.join("rootfs");
    stage_tmpfs(&run_rootfs)?;
    match Command::new("cp").arg("-a").arg(format!("{}/.", spec.rootfs.display())).arg(&run_rootfs).status() {
        Ok(s) if s.success() => {}
        Ok(s) => return Err(io::Error::new(io::ErrorKind::Other, format!("rootfs copy failed rc={:?}", s.code()))),
        Err(e) => return Err(io::Error::new(e.kind(), format!("rootfs copy exec failed: {e}"))),
    }

    // Phase-6 slice-1b — egress identity + resolution. Pure/fail-closed and BEFORE any netns is
    // created: a `(T-untrust, C-net)` session's sealed profile is resolved to pinned IPv4 here (any
    // AAAA-only/unresolvable host aborts the construct, IPv4-only), and the pinned host→IP map is
    // written into the writable rootfs `/etc/hosts` so the workload resolves through it — NO DNS
    // egress. `None` ⇒ loopback-only. The netns itself is brought UP later, as the last step before
    // spawn, so almost no fallible work follows it (tight fail-closed teardown).
    let net = spec.egress.map(|_| net_plane::SandboxNet::for_id(&spec.id));
    let resolved = match spec.egress {
        Some(profile) => Some(net_plane::resolve_profile_v4(profile)?),
        None => None,
    };
    if let Some(r) = &resolved {
        std::fs::create_dir_all(run_rootfs.join("etc"))?;
        std::fs::write(run_rootfs.join("etc/hosts"), net_plane::etc_hosts(&r.hosts))?;
    }
    let netns_path = net.as_ref().map(|n| n.ns_path());

    std::fs::write(
        bundle.join("config.json"),
        build_config_json(spec, &grants, &run_rootfs, netns_path.as_deref()),
    )?;

    let cg = CgroupLeaf::create(&spec.id, spec.mem_max, spec.pids_max)?;

    // Spike-only diagnostic (SHREK_T2_DEBUG=1): make runsc write its Sentry/boot debug log to a
    // PERSISTENT dir OUTSIDE `work` (teardown removes `work`, so the log would vanish with it) so a
    // caller — e.g. the sealed-VM S5 gate — can read WHY the sandbox failed to start. Prod never sets
    // this env, so it adds no flags and no behavior change on the shipped image.
    let debug_dir = std::env::var_os("SHREK_T2_DEBUG").map(|_| {
        let d = PathBuf::from(format!("/run/shrek-t2-debug/{}", spec.id));
        let _ = std::fs::create_dir_all(&d);
        d
    });

    // Drive runsc directly (no shim, no containerd): `runsc --root <state> --ignore-cgroups
    // --network=none --platform <p> [--debug --debug-log <dir>/] run --bundle <bundle> <id>`. The child
    // joins the bounded cgroup leaf in pre_exec (its runsc-forked sandbox+gofer inherit it;
    // --ignore-cgroups keeps runsc from moving them). `run` = create+start+wait+delete in one; we still
    // tear down state/bundle/cgroup.
    // Slice-1b — bring the egress netns UP as the LAST step before spawn (minimal fallible work
    // follows, so teardown stays tight): create the netns + veth + addressing + the profile's sealed
    // nft allow-list. runsc joins it (config.json network namespace) and netstack programs from the
    // veth. Fail-closed: any error tears the netns down, drops the cgroup + work tree, and aborts —
    // NEVER a fall-open network. An idempotent stale-clear precedes it (residue from a crashed run).
    if let (Some(n), Some(r)) = (net.as_ref(), resolved.as_ref()) {
        n.teardown();
        if let Err(e) = n.create_and_inject(&r.endpoints) {
            n.teardown();
            cg.destroy();
            let _ = std::fs::remove_dir_all(&work);
            return Err(io::Error::new(e.kind(), format!("egress plane setup failed (fail-closed, no network): {e}")));
        }
        eprintln!(
            "gatekeeperd/t2_plane: egress plane up profile={} ns={} cip={} dsts={} (netstack --network=sandbox)",
            spec.egress.map(|p| p.name).unwrap_or("none"), n.ns, n.cont_ip, r.endpoints.len()
        );
    }

    // Drive runsc directly (no shim, no containerd). Network: a granted egress session joins the
    // provisioned netns and runs `--network=sandbox` (gVisor netstack over our veth); every other
    // session is `--network=none` (loopback-only). Both are genuine T2 — the egress boundary is the
    // host-side veth + nft, independent of the platform (systrap/kvm) chosen at preflight.
    let network_flag = if net.is_some() { "--network=sandbox" } else { "--network=none" };
    let procs = cg.procs_path();
    let mut cmd = Command::new(&spec.runsc);
    cmd.arg("--root").arg(&state)
        .arg("--ignore-cgroups")
        .arg(network_flag)
        .arg(format!("--platform={}", spec.platform.flag()));
    if let Some(d) = &debug_dir {
        // Trailing slash ⇒ runsc treats it as a directory and writes per-subcommand files
        // (…boot.txt has the Sentry failure). Global flags, so they precede the `run` subcommand.
        cmd.arg("--debug").arg(format!("--debug-log={}/", d.display()));
    }
    cmd.arg("run")
        .arg("--bundle").arg(&bundle)
        .arg(&spec.id);
    unsafe {
        cmd.pre_exec(move || {
            // Place this (soon-to-be-runsc) process into the leaf so the whole sandbox tree is bounded.
            std::fs::write(&procs, format!("{}\n", std::process::id()))
        });
    }

    let status = cmd.status();
    // Fail-closed teardown regardless of outcome: force-delete any lingering container, detach the
    // writable binds + the staging tmpfs, remove the work tree, drop the cgroup leaf.
    let _ = Command::new(&spec.runsc).arg("--root").arg(&state).arg("delete").arg("--force").arg(&spec.id).status();
    // CRITICAL: detach EVERY writable bind (project + build) BEFORE removing the work tree.
    // `remove_dir_all` would otherwise recurse THROUGH a bind and unlink the workload's just-written
    // files — and any pre-existing content — from the real inode. Lazy-detach, then the removal only
    // clears the now-empty broker mountpoints, never the granted directories.
    for wg in spec.rw_grant.iter().chain(spec.build_grant.iter()) {
        let target = work.join("rwgrants").join(wg);
        let _ = umount2(&std::ffi::CString::new(target.to_string_lossy().as_bytes()).unwrap_or_default(), MNT_DETACH);
    }
    // Detach the per-sandbox staging tmpfs (mounted at work/rootfs) BEFORE the work-tree removal —
    // otherwise remove_dir_all trips on the still-mounted directory. Lazy-detach (MNT_DETACH) mirrors
    // the writable-grant binds above; the removal then clears only the now-empty broker mountpoint.
    let _ = umount2(&std::ffi::CString::new(run_rootfs.to_string_lossy().as_bytes()).unwrap_or_default(), MNT_DETACH);
    let _ = std::fs::remove_dir_all(&work);
    cg.destroy();
    // Slice-1b: tear the egress netns down (deletes the nft table + veth + netns). Leaves NO residual
    // plumbing — the fail-closed default is "no network".
    if let Some(n) = &net {
        n.teardown();
    }

    match status {
        Ok(s) => Ok(s.code().unwrap_or(128 + s.signal_or_zero())),
        Err(e) => Err(io::Error::new(e.kind(), format!("runsc invocation failed: {e}"))),
    }
}

/// Helper: 128 + signal if the process was signalled, else 0 (used only when `.code()` is None).
trait SignalOrZero {
    fn signal_or_zero(&self) -> i32;
}
impl SignalOrZero for std::process::ExitStatus {
    fn signal_or_zero(&self) -> i32 {
        use std::os::unix::process::ExitStatusExt;
        self.signal().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_prefers_systrap_unless_bare_metal_and_kvm_usable() {
        // Virtualized (our sealed-VM target) → systrap, even if the KVM probe would pass.
        assert_eq!(platform_decision(true, true), Platform::Systrap);
        assert_eq!(platform_decision(true, false), Platform::Systrap);
        // Bare metal but KVM_CREATE_VM probe failed → systrap.
        assert_eq!(platform_decision(false, false), Platform::Systrap);
        // ONLY authoritatively-bare-metal + usable KVM → kvm.
        assert_eq!(platform_decision(false, true), Platform::Kvm);
        assert_eq!(Platform::Systrap.flag(), "systrap");
        assert_eq!(Platform::Kvm.flag(), "kvm");
    }

    #[test]
    fn cpuinfo_flag_tokenizes_wholewords() {
        // The diagnostic virt-ext probe must match whole tokens, not substrings.
        let ci = "processor\t: 0\nflags\t\t: fpu vme de pse hypervisor lm\n";
        assert!(cpuinfo_has_flag(ci, "hypervisor"));
        assert!(!cpuinfo_has_flag(ci, "vmx")); // substring 'vme' must NOT match 'vmx'
        assert!(cpuinfo_has_flag("flags\t\t: fpu vmx lm\n", "vmx"));
        assert!(cpuinfo_has_flag("flags\t\t: fpu svm lm\n", "svm"));
    }

    #[test]
    fn config_json_shape_grants_present_vault_absent() {
        let spec = T2Spec {
            id: "t".into(),
            anchor: PathBuf::from("/srv"),
            grants: vec!["project".into()],
            rw_grant: None,
            build_grant: None,
            workload: vec!["/bin/echo".into(), "hi".into()],
            rootfs: PathBuf::from("/usr/lib/shrek/t2-rootfs"),
            runsc: PathBuf::from("/usr/lib/shrek/runsc"),
            platform: Platform::Systrap,
            mem_max: DEFAULT_MEM_MAX,
            pids_max: DEFAULT_PIDS_MAX,
            egress: None,
        };
        let grants = vec![GrantMount { name: "project".into(), source: PathBuf::from("/srv/project"), rw: false }];
        let j = build_config_json(&spec, &grants, &spec.rootfs, None);
        assert!(j.contains("\"readonly\":true"));
        assert!(j.contains("\"destination\":\"/srv/project\""));
        assert!(j.contains("\"source\":\"/srv/project\""));
        assert!(j.contains("\"rbind\",\"ro\""));
        assert!(j.contains("\"/bin/echo\",\"hi\""));
        // The vault is never named in a grant, so it can never appear as a mount source.
        assert!(!j.contains("vault"));
        // Slice-1b: a no-egress (netns_path=None) session lists NO network namespace — it runs
        // --network=none (loopback-only). pid/mount/ipc/uts are always present.
        assert!(!j.contains("\"type\":\"network\""), "no-egress config must omit the network ns: {j}");
        assert!(j.contains("\"type\":\"pid\"") && j.contains("\"type\":\"uts\""));
    }

    #[test]
    fn config_json_egress_adds_joined_network_namespace() {
        // Slice-1b: a granted-egress session carries a `network` namespace pointing at the
        // gatekeeper-provisioned netns, so runsc joins it and `--network=sandbox` programs netstack
        // from the veth. The path is exactly SandboxNet::ns_path for this id.
        let spec = T2Spec {
            id: "coder".into(),
            anchor: PathBuf::from("/srv"),
            grants: vec!["project".into()],
            rw_grant: None,
            build_grant: None,
            workload: vec!["/bin/true".into()],
            rootfs: PathBuf::from("/usr/lib/shrek/t2-rootfs"),
            runsc: PathBuf::from("/usr/lib/shrek/runsc"),
            platform: Platform::Systrap,
            mem_max: DEFAULT_MEM_MAX,
            pids_max: DEFAULT_PIDS_MAX,
            egress: None, // config shape is driven by netns_path, not this field
        };
        let grants = vec![GrantMount { name: "project".into(), source: PathBuf::from("/srv/project"), rw: false }];
        let ns_path = net_plane::SandboxNet::for_id("coder").ns_path();
        let j = build_config_json(&spec, &grants, &spec.rootfs, Some(&ns_path));
        assert!(
            j.contains(&format!("{{\"type\":\"network\",\"path\":\"{ns_path}\"}}")),
            "egress config must join the provisioned netns: {j}"
        );
        assert!(j.contains("/run/netns/shrek-coder"), "ns path must be the pure per-id netns: {j}");
    }

    #[test]
    fn config_json_rw_grant_is_rbind_rw_from_broker_source() {
        // The writable project grant is mounted rbind,rw from its broker-relocated source; read-only
        // grants keep rbind,ro. Guards the OCI mode selection that realizes PN1 write-through.
        let spec = T2Spec {
            id: "t".into(),
            anchor: PathBuf::from("/srv"),
            grants: vec!["deps".into()],
            rw_grant: Some("project".into()),
            build_grant: None,
            workload: vec!["/bin/sh".into()],
            rootfs: PathBuf::from("/usr/lib/shrek/t2-rootfs"),
            runsc: PathBuf::from("/usr/lib/shrek/runsc"),
            platform: Platform::Systrap,
            mem_max: DEFAULT_MEM_MAX,
            pids_max: DEFAULT_PIDS_MAX,
            egress: None,
        };
        let grants = vec![
            GrantMount { name: "deps".into(), source: PathBuf::from("/srv/deps"), rw: false },
            GrantMount { name: "project".into(), source: PathBuf::from("/run/shrek-t2/t/rwgrants/project"), rw: true },
        ];
        let j = build_config_json(&spec, &grants, &spec.rootfs, None);
        assert!(j.contains("\"destination\":\"/srv/project\""));
        assert!(j.contains("\"source\":\"/run/shrek-t2/t/rwgrants/project\""));
        assert!(j.contains("\"rbind\",\"rw\""), "project grant must be rbind,rw: {j}");
        // The read-only companion grant is unaffected.
        assert!(j.contains("\"rbind\",\"ro\""), "deps grant must stay rbind,ro: {j}");
    }

    #[test]
    fn config_json_project_and_build_grants_both_rbind_rw() {
        // The project (noexec, via relocate_rw) and the build area (exec, via relocate_rw_exec) both mount
        // rbind,rw in the OCI config — the exec DIFFERENCE lives in the host bind flags, not config.json.
        // Guards that a build grant is emitted as a writable mount from its own broker-relocated source.
        let spec = T2Spec {
            id: "t".into(),
            anchor: PathBuf::from("/srv"),
            grants: vec![],
            rw_grant: Some("project".into()),
            build_grant: Some("build".into()),
            workload: vec!["/bin/sh".into()],
            rootfs: PathBuf::from("/usr/lib/shrek/t2-rootfs"),
            runsc: PathBuf::from("/usr/lib/shrek/runsc"),
            platform: Platform::Systrap,
            mem_max: DEFAULT_MEM_MAX,
            pids_max: DEFAULT_PIDS_MAX,
            egress: None,
        };
        let grants = vec![
            GrantMount { name: "project".into(), source: PathBuf::from("/run/shrek-t2/t/rwgrants/project"), rw: true },
            GrantMount { name: "build".into(), source: PathBuf::from("/run/shrek-t2/t/rwgrants/build"), rw: true },
        ];
        let j = build_config_json(&spec, &grants, &spec.rootfs, None);
        assert!(j.contains("\"destination\":\"/srv/build\""));
        assert!(j.contains("\"source\":\"/run/shrek-t2/t/rwgrants/build\""));
        // Both writable grants are rbind,rw; neither the project nor build source is read-only.
        assert_eq!(j.matches("\"rbind\",\"rw\"").count(), 2, "both grants must be rbind,rw: {j}");
        assert!(!j.contains("\"rbind\",\"ro\""));
    }
}
