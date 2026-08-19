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

/// A single granted subtree, mounted read-only into the sandbox. `name` is the leaf beneath the
/// anchor; it appears inside the sandbox at `/srv/<name>` (mirroring the T1 guest_prefix). Ungranted
/// siblings and the vault are simply NOT listed → absent (ENOENT), the T1/T2 absence model.
struct GrantMount {
    name: String,
    source: PathBuf,
}

pub struct T2Spec {
    pub id: String,
    /// Trusted anchor directory under which grants live (e.g. `/srv`).
    pub anchor: PathBuf,
    pub grants: Vec<String>,
    pub workload: Vec<String>,
    /// Absolute path to the pinned, verity-sealed minimal rootfs (root.path, read-only).
    pub rootfs: PathBuf,
    /// Absolute path to the pinned, verity-sealed `runsc` binary.
    pub runsc: PathBuf,
    pub platform: Platform,
    pub mem_max: u64,
    pub pids_max: u64,
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
/// grant bind-mounts (rbind,ro), and an empty-but-for-grants mount namespace. Network is set by the
/// runsc `--network=none` flag, not here. Pure over the spec — unit-tested for shape.
fn build_config_json(spec: &T2Spec, grants: &[GrantMount]) -> String {
    let args = spec
        .workload
        .iter()
        .map(|a| format!("\"{}\"", json_escape(a)))
        .collect::<Vec<_>>()
        .join(",");
    let mounts = grants
        .iter()
        .map(|g| {
            format!(
                "{{\"destination\":\"/srv/{}\",\"type\":\"bind\",\"source\":\"{}\",\"options\":[\"rbind\",\"ro\"]}}",
                json_escape(&g.name),
                json_escape(&g.source.to_string_lossy())
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        concat!(
            "{{\"ociVersion\":\"1.0.2\",",
            "\"process\":{{\"args\":[{args}],\"cwd\":\"/\",\"user\":{{\"uid\":0,\"gid\":0}},\"capabilities\":{{}}}},",
            "\"root\":{{\"path\":\"{rootfs}\",\"readonly\":true}},",
            "\"mounts\":[{mounts}],",
            "\"linux\":{{\"namespaces\":[{{\"type\":\"pid\"}},{{\"type\":\"mount\"}},{{\"type\":\"ipc\"}},{{\"type\":\"uts\"}}]}}}}"
        ),
        args = args,
        rootfs = json_escape(&spec.rootfs.to_string_lossy()),
        mounts = mounts
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

    // Resolve grant sources beneath the anchor (name-only leaves; reject traversal defensively).
    let mut grants = Vec::new();
    for name in &spec.grants {
        if name.is_empty() || name.contains('/') || name == ".." {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("bad grant name: {name}")));
        }
        grants.push(GrantMount { name: name.clone(), source: spec.anchor.join(name) });
    }

    // Working dirs: state (runsc --root) + bundle (config.json). Both under /run, cleaned on exit.
    let work = PathBuf::from(format!("/run/shrek-t2/{}", spec.id));
    let bundle = work.join("bundle");
    let state = work.join("state");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&bundle)?;
    std::fs::create_dir_all(&state)?;
    std::fs::write(bundle.join("config.json"), build_config_json(spec, &grants))?;

    let cg = CgroupLeaf::create(&spec.id, spec.mem_max, spec.pids_max)?;

    // Drive runsc directly (no shim, no containerd): `runsc --root <state> --ignore-cgroups
    // --network=none --platform <p> run --bundle <bundle> <id>`. The child joins the bounded cgroup
    // leaf in pre_exec (its runsc-forked sandbox+gofer inherit it; --ignore-cgroups keeps runsc from
    // moving them). `run` = create+start+wait+delete in one; we still tear down state/bundle/cgroup.
    let procs = cg.procs_path();
    let mut cmd = Command::new(&spec.runsc);
    cmd.arg("--root").arg(&state)
        .arg("--ignore-cgroups")
        .arg("--network=none")
        .arg(format!("--platform={}", spec.platform.flag()))
        .arg("run")
        .arg("--bundle").arg(&bundle)
        .arg(&spec.id);
    unsafe {
        cmd.pre_exec(move || {
            // Place this (soon-to-be-runsc) process into the leaf so the whole sandbox tree is bounded.
            std::fs::write(&procs, format!("{}\n", std::process::id()))
        });
    }

    let status = cmd.status();
    // Fail-closed teardown regardless of outcome: force-delete any lingering container, remove the
    // work tree, drop the cgroup leaf. (`umount2` guards against a rootfs bind if one is ever added.)
    let _ = Command::new(&spec.runsc).arg("--root").arg(&state).arg("delete").arg("--force").arg(&spec.id).status();
    let _ = umount2(&std::ffi::CString::new(bundle.join("rootfs").to_string_lossy().as_bytes()).unwrap_or_default(), MNT_DETACH);
    let _ = std::fs::remove_dir_all(&work);
    cg.destroy();

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
            workload: vec!["/bin/echo".into(), "hi".into()],
            rootfs: PathBuf::from("/usr/lib/shrek/t2-rootfs"),
            runsc: PathBuf::from("/usr/lib/shrek/runsc"),
            platform: Platform::Systrap,
            mem_max: DEFAULT_MEM_MAX,
            pids_max: DEFAULT_PIDS_MAX,
        };
        let grants = vec![GrantMount { name: "project".into(), source: PathBuf::from("/srv/project") }];
        let j = build_config_json(&spec, &grants);
        assert!(j.contains("\"readonly\":true"));
        assert!(j.contains("\"destination\":\"/srv/project\""));
        assert!(j.contains("\"source\":\"/srv/project\""));
        assert!(j.contains("\"rbind\",\"ro\""));
        assert!(j.contains("\"/bin/echo\",\"hi\""));
        // The vault is never named in a grant, so it can never appear as a mount source.
        assert!(!j.contains("vault"));
    }
}
