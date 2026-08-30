//! bench_plane — the Bench lifecycle supervisor (ADR-003 Part 2 step 4).
//!
//! A **Bench** is the user-authority mutable-compute plane (ADR-002): a persistent, quota-capped
//! rootless-container home. This module is its lifecycle supervisor — create / run / enter / reset /
//! quota / destroy / list, backed by the durable [`crate::bench_record`] state model on `/home`.
//!
//! **This is NOT a T2 extension.** A Bench is a SIBLING plane to `t2_plane`: both may *import*
//! `mount_plane`/`net_plane` as libraries (step 5 wires FS/egress grants), but a Bench differs from a T2
//! sandbox by AUTHORITY + PERSISTENCE, not by isolation strength. T2 = a narrowly-constructed, ephemeral
//! task under AGENT authority, built AS a `gatekeeperd` child. A Bench = a persistent, USER-authority
//! computer whose containers run under `dev`'s ROOTLESS podman — `gatekeeperd` is a separate supervisor,
//! not the container's parent (rule 3). So this module NEVER touches `t2_plane.rs`.
//!
//! Privilege split (mirrors `shrek run` → `gatekeeperd sandbox`): the privileged supervisor ops (pool
//! dir allocation, ext4 project-id + quota, the durable record) run as root here; the container ops drop
//! to `dev` via `runuser` (rootless podman). Dep-free like the rest of gatekeeperd — it shells to the
//! sealed `podman`/`setquota`/`chattr`/`runuser` binaries (as `net_plane` shells to `ip`/`nft`).
//!
//! Step-4 scope = LIFECYCLE + QUOTA + the persistent state model. FS/egress GRANTS are step 5 (the
//! `grant`/`network` verbs are explicit stubs here); the ADR-002 `promote → Workshop` path is later.

use crate::bench_record::{self, BenchRecord};
use crate::linux_uapi::umount2;
use crate::mount_plane::{open_anchor, pin_beneath, relocate_ro, relocate_rw};
use crate::net_plane;
use std::ffi::CString;
use std::io;
use std::os::unix::fs::{chown, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

/// `umount2(MNT_DETACH)` — lazy-detach a materialized grant bind (the mount table entry goes away even
/// if a stale reference lingers). Same value `t2_plane` uses.
const MNT_DETACH: i32 = 2;

/// The Bench container-storage pool root (the noexec sub-mount stood up by `shrek-bench-pool.service`).
/// Overridable for the host/container oracle via `SHREK_BENCH_POOL`.
pub fn pool_dir() -> PathBuf {
    std::env::var("SHREK_BENCH_POOL").unwrap_or_else(|_| "/home/.shrek/benches".to_string()).into()
}

/// A Bench's own quota-scoped data directory, bound into the container at `/work`. Beneath `<pool>/b/`.
fn data_dir(pool: &Path, name: &str) -> PathBuf {
    pool.join("b").join(name)
}

/// The filesystem the ext4 project quota is set on (the shrek-data `/home`). Overridable for the oracle.
fn quota_fs() -> String {
    std::env::var("SHREK_BENCH_FS").unwrap_or_else(|_| "/home".to_string())
}

/// The offline seed image a Bench runs from (step 6 ships the real Scratch/Media seed; the oracle loads
/// a stand-in). A NAME, resolved by podman against the local store only (registries.conf search is empty).
fn seed_image() -> String {
    std::env::var("SHREK_BENCH_SEED").unwrap_or_else(|_| "localhost/scratch".to_string())
}

/// The offline seed's OCI-archive, baked into the shrek-bench sysext and `podman load`ed into `dev`'s
/// rootless store on demand ([`ensure_seed`]). `podman load` is the step-6 de-risk winner: it reads a
/// plain file + writes layers to the /home graphroot (a depth-1 ext4 mount), whereas an
/// `additionalimagestores` on the already-overlayed merged /usr risks the kernel's overlay
/// stacking-depth-2 limit. Overridable for the oracle (which stages its own tar).
fn seed_tar() -> PathBuf {
    std::env::var("SHREK_BENCH_SEED_TAR")
        .unwrap_or_else(|_| "/usr/share/shrek/bench/seeds/scratch.tar".to_string())
        .into()
}

/// The trusted anchor grants are resolved strictly beneath (rule 3 / mount_plane TOCTOU model). A Bench
/// is USER-authority, so the anchor is the desktop user's home: every grant is a real dir under it,
/// hence `dev`-owned by construction (so `dev`'s rootless podman reads/writes it and writes round-trip
/// to `dev` — proven in the step-5 de-risk). Overridable for the oracle via `SHREK_BENCH_ANCHOR`.
fn anchor_dir() -> PathBuf {
    std::env::var("SHREK_BENCH_ANCHOR").unwrap_or_else(|_| format!("/home/{BENCH_USER}")).into()
}

/// Per-Bench VOLATILE runtime dir on `/run` (grants + the bench-owned hosts file). `/run` is tmpfs, so
/// everything here is rebuilt at boot by `reissue` (the durable record on `/home` is the source of truth).
/// Overridable via `SHREK_BENCH_RUN` (the oracle has no `/run/shrek`).
fn bench_run_dir(id: &str) -> PathBuf {
    let base = std::env::var("SHREK_BENCH_RUN").unwrap_or_else(|_| "/run/shrek/bench".to_string());
    PathBuf::from(base).join(id)
}

/// Where a Bench's materialized FS grants live in the HOST mount ns (`dev`'s podman binds them from
/// here). NOT a per-request private ns like T2 — a Bench's podman is a separate process tree that must
/// see the mount (step-5 de-risk (A): a host-ns relocate bind propagates into rootless podman's `-v`).
///
/// PROPAGATION INVARIANT (proven, step-5): `dev`'s rootless podman keeps a persistent *pause* process
/// whose mount ns is created on its first command. A grant bind added AFTER that (the common case:
/// `run` once, then `grant`) is only visible to the pause ns if `/` is `rshared` — then the pause ns is
/// a slave of the shared peer group and the later bind propagates in. Real systemd makes `/` rshared at
/// boot, so this holds on the sealed system; the boot `reissue` unit must therefore NOT set
/// `PrivateMounts`/`MountFlags=slave` (Fable step-5 fix 5), or its re-materialized grants would strand
/// in a dead ns.
fn grants_dir(id: &str) -> PathBuf {
    bench_run_dir(id).join("grants")
}

/// The bench-owned `/etc/hosts` a NETWORKED bench binds (Shrek's real `/etc/hosts` is a `/home` symlink
/// podman chokes on, #2816 — so a networked bench gets an explicit, sealed-profile-derived hosts file).
fn hosts_file(id: &str) -> PathBuf {
    bench_run_dir(id).join("hosts")
}

/// In-container mount point for a granted host dir: `/grants/<leaf>`.
fn grant_mountpoint(leaf: &str) -> String {
    format!("/grants/{leaf}")
}

/// A parsed grant line from a [`BenchRecord`]. Stored as dep-free line text in `record.grants`:
/// `fs-rw <canonical-dir>` / `fs-ro <canonical-dir>` (a materialized host bind) or `net <profile>` (a
/// sealed egress policy, injected per container start). Unknown forms parse to `None` (ignored, never
/// mis-applied) — the record parser is already fail-closed on truly corrupt records.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Grant {
    Fs { rw: bool, path: PathBuf },
    Net { profile: String },
}

impl Grant {
    fn parse(s: &str) -> Option<Grant> {
        let (kind, rest) = s.split_once(' ')?;
        match kind {
            "fs-rw" => Some(Grant::Fs { rw: true, path: PathBuf::from(rest) }),
            "fs-ro" => Some(Grant::Fs { rw: false, path: PathBuf::from(rest) }),
            "net" => Some(Grant::Net { profile: rest.to_string() }),
            _ => None,
        }
    }

    fn encode(&self) -> String {
        match self {
            Grant::Fs { rw: true, path } => format!("fs-rw {}", path.display()),
            Grant::Fs { rw: false, path } => format!("fs-ro {}", path.display()),
            Grant::Net { profile } => format!("net {profile}"),
        }
    }

    /// The safe single-component mount leaf (the granted dir's basename), or `None` if it is not a safe
    /// token — reuses the Bench-name validator so a grant leaf can never traverse or collide by a dotfile.
    fn fs_leaf(path: &Path) -> Option<String> {
        let leaf = path.file_name()?.to_str()?.to_string();
        bench_record::valid_bench_name(&leaf).then_some(leaf)
    }
}

/// The record's single egress policy, if any (MVP: one `net` grant per Bench).
fn egress_profile(rec: &BenchRecord) -> Option<String> {
    rec.grants.iter().find_map(|g| match Grant::parse(g) {
        Some(Grant::Net { profile }) => Some(profile),
        _ => None,
    })
}

/// Default per-Bench block quota (KiB). Generous but bounded so one Bench cannot fill `/home`.
pub const DEFAULT_QUOTA_KIB: u64 = 4 * 1024 * 1024; // 4 GiB

/// The desktop user rootless podman runs as. uid resolved from /etc/passwd; the runtime dir is its logind
/// `XDG_RUNTIME_DIR`. (Bench containers run under `dev`'s delegated `user-<uid>.slice`, proven in Bench-0.)
const BENCH_USER: &str = "dev";

fn dev_uid() -> u32 {
    std::fs::read_to_string("/etc/passwd")
        .ok()
        .and_then(|p| {
            p.lines().find_map(|l| {
                let f: Vec<&str> = l.split(':').collect();
                (f.len() >= 4 && f[0] == BENCH_USER).then(|| f[2].parse::<u32>().ok()).flatten()
            })
        })
        .unwrap_or(1000)
}

// ---- pure argv builders (unit-tested; the exec side is proven in the oracle/VM) ------------------

/// A materialized FS grant to bind into the container: the host path (under `grants_dir`) → `/grants/<leaf>`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct FsBind {
    target: PathBuf,
    leaf: String,
    rw: bool,
}

/// The inputs to one `podman run`. A struct (not 11 positional args) so the plain / networked / interactive
/// call sites stay readable and the unit tests pin the exact argv.
struct RunSpec<'a> {
    name: &'a str,
    data: &'a Path,
    image: &'a str,
    interactive: bool,
    /// `-d`: start detached (the networked path — inject egress before the workload egresses).
    detached: bool,
    /// `--rm`: auto-remove on exit (the plain foreground path; the detached path removes explicitly
    /// AFTER `podman wait` reads the exit code).
    remove: bool,
    /// Bind a bench-owned `/etc/hosts` (networked benches). Coexists with `--no-hosts` (step-5 de-risk (C)).
    hosts_bind: Option<&'a Path>,
    fs_binds: &'a [FsBind],
    workload: &'a [String],
}

/// Build the `podman run` argv for a Bench workload. `--network=none` always (rule 3: benches start with
/// NO egress; the networked path late-attaches an injected veth into this very netns). `--no-hosts`
/// sidesteps Shrek's /etc/hosts symlink (#2816); a networked bench additionally binds its own hosts file.
/// The quota-scoped data dir is bound at `/work` (cwd); each FS grant at `/grants/<leaf>` with
/// `ro/rw,noexec,nodev,nosuid` (rule 2: granted data is never executable). `crun` runtime; the container
/// is named `shrek-bench-<name>` so lifecycle verbs find it.
///
/// USERNS POSTURE (MVP): the DEFAULT rootless mapping (container-root ⇔ host-`dev`) — NOT `keep-id`. A
/// grant dir is `dev`-owned by construction (pinned beneath `dev`'s home), so a workload running as
/// container-root reads/writes it and its writes land back as `dev` on the host (proven, step-5 de-risk
/// (A) + oracle). The INVARIANT this rests on: **bench workloads run as container-root**, which every
/// Shrek offline seed does. An image that sets a non-root `USER` would map its writes to a subuid and
/// pollute `dev`'s tree (Fable step-5 fix 2) — the robust answer for that is an idmapped `-v`, deferred
/// with the arbitrary-image story (`keep-id` is the wrong tool: it makes container-root a subuid that
/// then can't write the `dev`-owned grant at all — the exact failure the oracle caught).
fn podman_run_argv(spec: &RunSpec) -> Vec<String> {
    let mut a: Vec<String> = vec!["run".into()];
    if spec.remove {
        a.push("--rm".into());
    }
    if spec.detached {
        a.push("-d".into());
    }
    a.push("--name".into());
    a.push(format!("shrek-bench-{}", spec.name));
    a.push("--network=none".into());
    a.push("--no-hosts".into());
    a.push("--runtime".into());
    a.push("crun".into());
    a.push("-v".into());
    a.push(format!("{}:/work", spec.data.display()));
    a.push("-w".into());
    a.push("/work".into());
    if let Some(h) = spec.hosts_bind {
        a.push("-v".into());
        a.push(format!("{}:/etc/hosts:ro", h.display()));
    }
    for b in spec.fs_binds {
        let mode = if b.rw { "rw" } else { "ro" };
        a.push("-v".into());
        a.push(format!("{}:{}:{mode},noexec,nodev,nosuid", b.target.display(), grant_mountpoint(&b.leaf)));
    }
    if spec.interactive && !spec.detached {
        a.push("-it".into());
    }
    a.push(spec.image.to_string());
    a.extend(spec.workload.iter().cloned());
    a
}

/// Build the `setquota` argv that caps a project id's block usage at `kib` KiB (hard), no soft/inode
/// limit. Enforced against non-root writers only (root is CAP_SYS_RESOURCE-exempt), which a Bench is.
fn setquota_argv(project: u32, kib: u64, fs: &str) -> Vec<String> {
    vec![
        "-P".into(),
        project.to_string(),
        "0".into(),
        kib.to_string(),
        "0".into(),
        "0".into(),
        fs.to_string(),
    ]
}

// ---- shelling to the sealed tools ---------------------------------------------------------------

fn run_ok(bin: &str, args: &[String]) -> io::Result<()> {
    let st = Command::new(bin).args(args).status()?;
    if st.success() {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::Other, format!("{bin} {:?} failed rc={:?}", args, st.code())))
    }
}

/// Run `podman …` AS `dev` (rootless), with the env rootless podman needs: HOME, the logind runtime dir,
/// and a minimal PATH. Returns the child exit code (propagated verbatim for `run`/`enter` fidelity).
fn podman_as_dev(args: &[String]) -> io::Result<i32> {
    let uid = dev_uid();
    let mut cmd = Command::new("runuser");
    cmd.arg("-u").arg(BENCH_USER).arg("--").arg("env")
        .arg("HOME=/home/dev")
        .arg(format!("XDG_RUNTIME_DIR=/run/user/{uid}"))
        // podman's default (systemd) cgroup manager creates the container's transient scope over the
        // user D-Bus; point at the standard session-bus socket so it is found in a real graphical session
        // AND the headless probe (which starts dev's dbus.socket at this path). Bench-0 needed this.
        .arg(format!("DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus"))
        .arg("PATH=/usr/bin:/bin")
        .arg("podman")
        .args(args);
    Ok(cmd.status()?.code().unwrap_or(-1))
}

/// Best-effort `podman rm -f` of a Bench's container (as `dev`). Idempotent — a missing container is fine.
fn podman_rm(name: &str) {
    let _ = podman_as_dev(&["rm".into(), "-f".into(), format!("shrek-bench-{name}")]);
}

/// Like [`podman_as_dev`] but CAPTURES stdout (for `podman inspect`/`podman wait`, which print the value
/// to stdout and return 0 themselves). Trimmed on the caller side.
fn podman_as_dev_stdout(args: &[String]) -> io::Result<String> {
    let uid = dev_uid();
    let out = Command::new("runuser")
        .arg("-u").arg(BENCH_USER).arg("--").arg("env")
        .arg("HOME=/home/dev")
        .arg(format!("XDG_RUNTIME_DIR=/run/user/{uid}"))
        .arg(format!("DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus"))
        .arg("PATH=/usr/bin:/bin")
        .arg("podman")
        .args(args)
        .stderr(Stdio::null())
        .output()?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Pure freshness test for the loaded seed vs the sysext archive (testable — [`ensure_seed`] shells out).
/// `want` = the sidecar `<tar>.digest` (the built image Id) or `None` if no sidecar shipped; `have` = the
/// Id of the currently-loaded seed image (empty ⇒ absent). With a sidecar, an EXACT Id match is required
/// (so an OS-shipped seed update — new archive, new Id — forces a reload); without one, load-if-absent.
fn seed_is_fresh(want: Option<&str>, have: &str) -> bool {
    match want {
        Some(w) => !w.is_empty() && have == w,
        None => !have.is_empty(),
    }
}

/// Make sure the seed image is in `dev`'s rootless store before a `run`, `podman load`ing the sysext
/// OCI-archive iff the image is absent OR stale (digest-keyed, per the seed-staleness flag: a mutable
/// `localhost/scratch` tag would otherwise pin a user to the old image after an OS update). Best-effort +
/// fail-OPEN: no baked tar (the oracle supplies the image another way) ⇒ no-op; a load error is logged but
/// `run` proceeds so podman surfaces a clear "image not found" rather than this masking it.
fn ensure_seed() {
    let tar = seed_tar();
    if !tar.exists() {
        return; // no baked seed on this host — the image is provided by other means (oracle/test).
    }
    let want = std::fs::read_to_string(format!("{}.digest", tar.display()))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let have = podman_as_dev_stdout(&[
        "image".into(), "inspect".into(), "--format".into(), "{{.Id}}".into(), seed_image(),
    ]).map(|s| s.trim().to_string()).unwrap_or_default();
    if seed_is_fresh(want.as_deref(), &have) {
        return;
    }
    if let Err(e) = podman_as_dev(&["load".into(), "-i".into(), tar.display().to_string()]) {
        eprintln!("bench run: seed load from {} failed (continuing): {e}", tar.display());
    }
}

/// The container's netns-leader pid (its init process). Valid only for a RUNNING container; `0`/unparseable
/// (created/stopped) ⇒ `None`.
fn podman_pid(name: &str) -> Option<u32> {
    let s = podman_as_dev_stdout(&[
        "inspect".into(), "--format".into(), "{{.State.Pid}}".into(), format!("shrek-bench-{name}"),
    ]).ok()?;
    match s.trim().parse::<u32>() {
        Ok(p) if p > 0 => Some(p),
        _ => None,
    }
}

/// The inode of a `/proc/<pid>/ns/net` symlink target (`net:[NNNN]` → `NNNN`). Used to prove a leader is
/// in a DISTINCT netns before we attach root plumbing to it, and that the attached ns still IS the leader's.
fn netns_inode(pid: u32) -> Option<u64> {
    let link = std::fs::read_link(format!("/proc/{pid}/ns/net")).ok()?;
    let s = link.to_str()?;
    s.strip_prefix("net:[")?.strip_suffix(']')?.parse::<u64>().ok()
}

/// Fable step-5 fix 4 (pid-recycle guard): the leader pid must live in a netns DISTINCT from gatekeeperd's
/// own before we `ip netns attach` + `route add default` against it. A recycled/dead pid that now maps to
/// a host process would otherwise let injection mutate the HOST's routing. Fail-closed if they match or
/// either is unreadable.
fn leader_in_distinct_netns(leader: u32) -> bool {
    let mine = std::fs::read_link("/proc/self/ns/net").ok()
        .and_then(|l| l.to_str()?.strip_prefix("net:[")?.strip_suffix(']')?.parse::<u64>().ok());
    match (mine, netns_inode(leader)) {
        (Some(mine), Some(theirs)) => mine != theirs,
        _ => false,
    }
}

/// Post-attach re-verification (Fable step-5 fix 4): the netns `ip netns attach` bound at `/run/netns/<ns>`
/// must still be the SAME inode as the leader's current netns — else the pid changed identity mid-attach.
fn attached_ns_matches_leader(ns: &str, leader: u32) -> bool {
    let bound = std::fs::metadata(format!("/run/netns/{ns}")).ok().map(|m| {
        use std::os::unix::fs::MetadataExt;
        m.ino()
    });
    match (bound, netns_inode(leader)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// Is `p` currently a mount point? Dep-free scan of `/proc/self/mountinfo` (field 5 = mount point). Lets
/// grant materialization be idempotent (reissue / a repeated `run` never stacks a second bind).
fn is_mountpoint(p: &Path) -> bool {
    let Ok(mi) = std::fs::read_to_string("/proc/self/mountinfo") else { return false };
    let want = p.to_string_lossy();
    mi.lines().any(|l| l.split(' ').nth(4).is_some_and(|mp| mp == want))
}

/// Create the per-Bench `/run` grant dir chain with the ProtectHome-safe perms (Fable step-5 fix 1): the
/// `<id>` and `grants` dirs are `dev`-owned `0700` — `dev` traverses them for its `-v`, but no other
/// unprivileged service can follow the bind back into `dev`'s home. The `/run/shrek/bench` container stays
/// root `0755` (just a namespace for the per-bench dirs).
fn prepare_grant_dir(id: &str) -> io::Result<()> {
    let bench = bench_run_dir(id);
    let grants = grants_dir(id);
    // parents: /run/shrek + /run/shrek/bench (root 0755).
    if let Some(container) = bench.parent() {
        std::fs::create_dir_all(container)?;
    }
    std::fs::create_dir_all(&grants)?;
    let uid = dev_uid();
    for d in [&bench, &grants] {
        std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700))?;
        let _ = chown(d, Some(uid), Some(uid));
    }
    Ok(())
}

/// Materialize ONE FS grant into the host mount ns (rule 3): pin the canonical dir strictly beneath the
/// home anchor (TOCTOU-safe — refuses any symlink/`..`), then `relocate_rw`/`relocate_ro` it onto
/// `grants_dir/<leaf>` (rw/ro, always `nosuid,nodev,NOEXEC`). Idempotent: an already-mounted target is a
/// no-op (a repeated `run` or boot `reissue` never double-binds). NOT in a private ns — `dev`'s podman
/// must see the mount.
fn materialize_one(rw: bool, src_canonical: &Path, target: &Path) -> io::Result<()> {
    let anchor_path = anchor_dir();
    let rel = src_canonical
        .strip_prefix(&anchor_path)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "grant is not beneath the anchor"))?;
    let rel_str = rel.to_str().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "non-utf8 grant path"))?;
    let anchor = open_anchor(&anchor_path)?;
    let pinned = pin_beneath(&anchor, rel_str)?;
    if !pinned.is_dir {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "grant target must be a directory"));
    }
    if is_mountpoint(target) {
        return Ok(()); // already materialized
    }
    if rw {
        relocate_rw(&pinned, target)
    } else {
        relocate_ro(&pinned, target)
    }
}

/// Ensure every FS grant in the record is materialized in the host ns (called before `run`, and by boot
/// `reissue`). Fail-closed: a grant that will not pin/relocate aborts (never a silently-missing bind).
fn ensure_grants_materialized(rec: &BenchRecord) -> io::Result<()> {
    let has_fs = rec.grants.iter().any(|g| matches!(Grant::parse(g), Some(Grant::Fs { .. })));
    if has_fs {
        prepare_grant_dir(&rec.id)?;
    }
    for g in &rec.grants {
        if let Some(Grant::Fs { rw, path }) = Grant::parse(g) {
            let Some(leaf) = Grant::fs_leaf(&path) else {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("unsafe grant leaf for {}", path.display())));
            };
            materialize_one(rw, &path, &grants_dir(&rec.id).join(&leaf))?;
        }
    }
    Ok(())
}

/// The `-v` binds for the record's materialized FS grants.
fn fs_binds_for(rec: &BenchRecord) -> Vec<FsBind> {
    let mut out = Vec::new();
    for g in &rec.grants {
        if let Some(Grant::Fs { rw, path }) = Grant::parse(g) {
            if let Some(leaf) = Grant::fs_leaf(&path) {
                out.push(FsBind { target: grants_dir(&rec.id).join(&leaf), leaf, rw });
            }
        }
    }
    out
}

/// Lazy-unmount every materialized FS grant (teardown/revoke). MNT_DETACH removes the mount-table entry;
/// the container must already be stopped (Fable step-5 fix 6 — a running container keeps its own view).
fn unmount_grants(rec: &BenchRecord) {
    for b in fs_binds_for(rec) {
        if let Ok(c) = CString::new(b.target.as_os_str().as_encoded_bytes()) {
            let _ = umount2(&c, MNT_DETACH);
        }
    }
}

/// Write the bench-owned `/etc/hosts` from a resolved egress profile (localhost + one pinned line per
/// sealed host). 0644 under the `dev`-owned `0700` bench dir. Written BEFORE `podman run` (fix 6).
fn write_hosts(id: &str, resolved: &net_plane::Resolved) -> io::Result<PathBuf> {
    prepare_grant_dir(id)?; // ensures the (dev 0700) bench dir exists
    let path = hosts_file(id);
    std::fs::write(&path, net_plane::etc_hosts(&resolved.hosts))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))?;
    let uid = dev_uid();
    let _ = chown(&path, Some(uid), Some(uid));
    Ok(path)
}

/// Assign `project` id + the inherit flag to `dir` so files created under it are quota-accounted (e2fsprogs
/// `chattr -p <projid> +P`). Root-only. Applied at create, so the Bench's data dir carries the id.
fn chattr_project(project: u32, dir: &Path) -> io::Result<()> {
    run_ok("chattr", &["-p".into(), project.to_string(), "+P".into(), dir.display().to_string()])
}

/// Set (or clear, `kib=0` → hard `0` = unlimited) the block quota for a Bench's project id on the /home fs.
fn apply_quota(project: u32, kib: u64) -> io::Result<()> {
    run_ok("setquota", &setquota_argv(project, kib, &quota_fs()))
}

fn chown_dev(path: &Path) -> io::Result<()> {
    let uid = dev_uid();
    run_ok("chown", &["-R".into(), format!("{uid}:{uid}"), path.display().to_string()])
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

// ---- lifecycle verbs ----------------------------------------------------------------------------

fn records() -> Vec<BenchRecord> {
    bench_record::list_records(&bench_record::records_dir())
}

/// create <name> [--quota KiB]: allocate a project id, make the Bench data dir on the pool, tag it with
/// the project id, cap it, chown to `dev`, and write the durable record. Fails if the Bench exists.
fn create(name: &str, quota_kib: u64) -> i32 {
    if !bench_record::valid_bench_name(name) {
        eprintln!("bench: invalid name {name:?}");
        return 2;
    }
    let rdir = bench_record::records_dir();
    if bench_record::load_record(&rdir, name).is_some() {
        eprintln!("bench: {name} already exists");
        return 1;
    }
    let project = bench_record::next_project_id(&records());
    let pool = pool_dir();
    let data = data_dir(&pool, name);
    let build = || -> io::Result<()> {
        std::fs::create_dir_all(&data)?;
        chattr_project(project, &data)?;
        apply_quota(project, quota_kib)?;
        chown_dev(&data)?;
        let rec = BenchRecord {
            name: name.to_string(),
            id: name.to_string(),
            project,
            quota_kib,
            created: now_secs(),
            state: "created".into(),
            grants: vec![],
        };
        bench_record::write_record(&rdir, &rec)?;
        Ok(())
    };
    match build() {
        Ok(()) => {
            println!("bench: created {name} (project={project} quota_kib={quota_kib} data={})", data.display());
            0
        }
        Err(e) => {
            // Fail-closed cleanup: a half-built Bench leaves no record + no quota id in use.
            let _ = apply_quota(project, 0);
            let _ = std::fs::remove_dir_all(&data);
            eprintln!("bench: create {name} failed: {e}");
            1
        }
    }
}

/// run <name> [-i] -- <workload>: run a container in the Bench (rootless). FS grants are bound at
/// `/grants/<leaf>`; if the Bench has an egress policy, the run goes through [`run_networked`] (detached →
/// inject → wait). Otherwise it is the plain foreground `--network=none` path. `-i` = interactive shell.
fn run(name: &str, interactive: bool, workload: &[String]) -> i32 {
    let rdir = bench_record::records_dir();
    let Some(mut rec) = bench_record::load_record(&rdir, name) else {
        eprintln!("bench: no such bench {name}");
        return 4;
    };
    // Ensure the offline seed is loaded into dev's store (load-if-absent-or-stale from the sysext archive).
    ensure_seed();
    // Materialize FS grants in the host ns (idempotent — no-op if already mounted). Fail-closed.
    if let Err(e) = ensure_grants_materialized(&rec) {
        eprintln!("bench run: FS grant materialization failed (fail-closed): {e}");
        return 1;
    }
    let fs_binds = fs_binds_for(&rec);
    let data = data_dir(&pool_dir(), name);
    let wl = if interactive && workload.is_empty() { vec!["/bin/sh".to_string()] } else { workload.to_vec() };

    if let Some(profile) = egress_profile(&rec) {
        return run_networked(&mut rec, interactive, &data, &profile, &fs_binds, &wl);
    }

    let spec = RunSpec {
        name,
        data: &data,
        image: &seed_image(),
        interactive,
        detached: false,
        remove: true,
        hosts_bind: None,
        fs_binds: &fs_binds,
        workload: &wl,
    };
    let argv = podman_run_argv(&spec);
    rec.state = "running".into();
    let _ = bench_record::write_record(&rdir, &rec);
    let code = podman_as_dev(&argv).unwrap_or(-1);
    rec.state = "stopped".into();
    let _ = bench_record::write_record(&rdir, &rec);
    code
}

/// The egress path (rule 3, late-attach). A rootless `--network=none` container cannot join a
/// root-created netns (create_and_inject is structurally unavailable), so: resolve the sealed profile +
/// write the bench hosts file FIRST (fix 6), start the container DETACHED, discover its netns leader,
/// verify the leader is in a distinct netns (fix 4), then root `gatekeeperd` `inject()`s the veth + sealed
/// nft allow-list into that netns and re-verifies identity. Only THEN does the workload's egress become
/// possible — and only to the allow-list. The start→inject window is fail-SAFE (the container has ZERO
/// egress until injection; it never has MORE than granted), so no rendezvous barrier is needed for a
/// user-authority Bench (Fable step-5 ruling 3). Interactive `enter` holds the container with `sleep
/// infinity` so the shell has egress from its first command; non-interactive `run` starts the workload
/// and blocks on `podman wait`.
fn run_networked(rec: &mut BenchRecord, interactive: bool, data: &Path, profile: &str, fs_binds: &[FsBind], workload: &[String]) -> i32 {
    let rdir = bench_record::records_dir();
    let name = rec.name.clone();
    let ctr = format!("shrek-bench-{name}");

    let Some(profile_ref) = shrek_policy::egress::resolve(profile) else {
        eprintln!("bench run: recorded egress profile {profile:?} is not in sealed policy — refusing (fail-closed)");
        return 1;
    };
    let resolved = match net_plane::resolve_profiles_v4(&[profile_ref]) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("bench run: egress profile resolve failed (fail-closed, no network): {e}");
            return 1;
        }
    };
    let hosts_path = match write_hosts(&rec.id, &resolved) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("bench run: bench hosts file write failed: {e}");
            return 1;
        }
    };

    // Distinct per-bench network identity (fix 5: prefix so a Bench never collides with a T2 id's /30).
    let net = net_plane::SandboxNet::for_id(&format!("bench-{}", rec.id));
    net.teardown(); // clear any crash residue

    let holder = vec!["sleep".to_string(), "infinity".to_string()];
    let start_wl = if interactive { &holder } else { workload };
    let spec = RunSpec {
        name: &name,
        data,
        image: &seed_image(),
        interactive: false,
        detached: true,
        remove: false,
        hosts_bind: Some(&hosts_path),
        fs_binds,
        workload: start_wl,
    };
    rec.state = "running".into();
    let _ = bench_record::write_record(&rdir, rec);

    let fail = |net: &net_plane::SandboxNet, msg: String| -> i32 {
        eprintln!("{msg}");
        net.teardown();
        podman_rm(&name);
        -1
    };

    if podman_as_dev(&podman_run_argv(&spec)).map(|c| c != 0).unwrap_or(true) {
        let rc = fail(&net, "bench run: detached container start failed".into());
        rec.state = "stopped".into();
        let _ = bench_record::write_record(&rdir, rec);
        return rc;
    }
    let Some(leader) = podman_pid(&name) else {
        let rc = fail(&net, "bench run: could not discover the container netns leader".into());
        rec.state = "stopped".into();
        let _ = bench_record::write_record(&rdir, rec);
        return rc;
    };
    if !leader_in_distinct_netns(leader) {
        let rc = fail(&net, format!("bench run: leader {leader} is not in a distinct netns — refusing inject (fail-closed)"));
        rec.state = "stopped".into();
        let _ = bench_record::write_record(&rdir, rec);
        return rc;
    }
    if let Err(e) = net.inject(leader, &resolved.endpoints, &resolved.no_masquerade_ips()) {
        let rc = fail(&net, format!("bench run: egress inject failed (fail-closed, no network): {e}"));
        rec.state = "stopped".into();
        let _ = bench_record::write_record(&rdir, rec);
        return rc;
    }
    if !attached_ns_matches_leader(&net.ns, leader) {
        let rc = fail(&net, "bench run: netns identity drift after attach — tearing down (fail-closed)".into());
        rec.state = "stopped".into();
        let _ = bench_record::write_record(&rdir, rec);
        return rc;
    }
    eprintln!(
        "gatekeeperd/bench_plane: egress up bench={} profile={} ns={} cip={} dsts={}",
        rec.id, profile, net.ns, net.cont_ip, resolved.endpoints.len()
    );

    let code = if interactive {
        let _ = podman_as_dev(&["exec".into(), "-it".into(), ctr.clone(), "/bin/sh".into()]);
        let _ = podman_as_dev(&["stop".into(), "-t".into(), "2".into(), ctr.clone()]);
        0
    } else {
        podman_as_dev_stdout(&["wait".into(), ctr.clone()])
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok())
            .unwrap_or(-1)
    };

    net.teardown();
    podman_rm(&name);
    rec.state = "stopped".into();
    let _ = bench_record::write_record(&rdir, rec);
    code
}

/// grant <name> <path> --rw|--ro: route a filesystem grant through the Gatekeeper (rule 3). The path must
/// be an existing DIRECTORY beneath the home anchor (so it is `dev`-owned). It is pinned TOCTOU-safely and
/// relocated (rw/ro, always noexec) into the host-ns `grants_dir`, and recorded durably. Idempotent-safe:
/// a duplicate path or a basename that collides with an existing grant's mount leaf is refused clearly.
fn grant(name: &str, path_str: &str, rw: bool) -> i32 {
    let rdir = bench_record::records_dir();
    let Some(mut rec) = bench_record::load_record(&rdir, name) else {
        eprintln!("bench: no such bench {name}");
        return 4;
    };
    let canonical = match std::fs::canonicalize(path_str) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("bench grant: cannot resolve {path_str:?}: {e}");
            return 2;
        }
    };
    let anchor = anchor_dir();
    if canonical == anchor || !canonical.starts_with(&anchor) {
        eprintln!("bench grant: {} must be a directory strictly beneath {} (user-authority anchor)", canonical.display(), anchor.display());
        return 2;
    }
    if !std::fs::metadata(&canonical).map(|m| m.is_dir()).unwrap_or(false) {
        eprintln!("bench grant: {} is not a directory (only directory grants are supported)", canonical.display());
        return 2;
    }
    let Some(leaf) = Grant::fs_leaf(&canonical) else {
        eprintln!("bench grant: {} has an unsafe basename (need alnum/._- , no leading dot)", canonical.display());
        return 2;
    };
    // Ensure the per-bench /run grant dir exists with the ProtectHome-safe perms (dev-owned 0700) BEFORE
    // relocate creates the leaf under it — else relocate's create_dir_all leaves the parent root-owned.
    if let Err(e) = prepare_grant_dir(&rec.id) {
        eprintln!("bench grant: could not prepare the grant dir: {e}");
        return 1;
    }
    // Collision / duplicate checks against the existing grant set.
    for g in &rec.grants {
        if let Some(Grant::Fs { path, .. }) = Grant::parse(g) {
            if path == canonical {
                eprintln!("bench grant: {} is already granted to {name}", canonical.display());
                return 1;
            }
            if Grant::fs_leaf(&path).as_deref() == Some(leaf.as_str()) {
                eprintln!("bench grant: mount leaf {leaf:?} already used by another grant — grant the parent or rename");
                return 1;
            }
        }
    }
    if let Err(e) = materialize_one(rw, &canonical, &grants_dir(&rec.id).join(&leaf)) {
        eprintln!("bench grant: materialization failed (fail-closed): {e}");
        return 1;
    }
    rec.grants.push(Grant::Fs { rw, path: canonical.clone() }.encode());
    if let Err(e) = bench_record::write_record(&rdir, &rec) {
        eprintln!("bench grant: record write failed: {e}");
        return 1;
    }
    println!("bench: granted {} to {name} at {} ({})", canonical.display(), grant_mountpoint(&leaf), if rw { "rw" } else { "ro" });
    0
}

/// network <name> <profile>: attach a SEALED egress policy to a Bench (rule 3). The profile name is
/// validated against the compiled-in `shrek_policy::egress` table (default-deny: an unknown name is
/// refused, never "allow all"). `none` REVOKES egress. The policy is recorded and injected per container
/// start (a rootless netns dies on every stop). No live container is touched here.
fn network(name: &str, profile: &str) -> i32 {
    let rdir = bench_record::records_dir();
    let Some(mut rec) = bench_record::load_record(&rdir, name) else {
        eprintln!("bench: no such bench {name}");
        return 4;
    };
    let revoke = profile == "none";
    if !revoke && shrek_policy::egress::resolve(profile).is_none() {
        eprintln!("bench network: {profile:?} is not a sealed egress profile — refused (policy is default-deny)");
        return 2;
    }
    // One egress policy per Bench (MVP): drop any existing net grant, then add (unless revoking).
    rec.grants.retain(|g| !matches!(Grant::parse(g), Some(Grant::Net { .. })));
    if !revoke {
        rec.grants.push(Grant::Net { profile: profile.to_string() }.encode());
    }
    if let Err(e) = bench_record::write_record(&rdir, &rec) {
        eprintln!("bench network: record write failed: {e}");
        return 1;
    }
    if revoke {
        println!("bench: {name} egress revoked (runs offline)");
    } else {
        println!("bench: {name} egress policy set to {profile} (injected per run)");
    }
    0
}

/// reset <name>: wipe the Bench's mutable data (its /work dir) but KEEP its identity, project id, quota,
/// grants, and record — a "clean the workbench, keep the bench" operation.
fn reset(name: &str) -> i32 {
    let rdir = bench_record::records_dir();
    let Some(rec) = bench_record::load_record(&rdir, name) else {
        eprintln!("bench: no such bench {name}");
        return 4;
    };
    podman_rm(name);
    // Tear down any live egress plumbing for this bench (best-effort; the container is now gone).
    net_plane::SandboxNet::for_id(&format!("bench-{}", rec.id)).teardown();
    let data = data_dir(&pool_dir(), name);
    // Remove the contents, not the dir itself (the dir keeps its project-id tag for the quota).
    if let Ok(rd) = std::fs::read_dir(&data) {
        for ent in rd.flatten() {
            let _ = std::fs::remove_dir_all(ent.path()).or_else(|_| std::fs::remove_file(ent.path()));
        }
    }
    let _ = chown_dev(&data);
    println!("bench: reset {name} (data cleared; identity + quota kept)");
    0
}

/// quota <name> [KiB]: no arg → print the current usage/limit; an arg → set a new hard cap.
fn quota(name: &str, kib: Option<u64>) -> i32 {
    let rdir = bench_record::records_dir();
    let Some(mut rec) = bench_record::load_record(&rdir, name) else {
        eprintln!("bench: no such bench {name}");
        return 4;
    };
    match kib {
        Some(k) => {
            if let Err(e) = apply_quota(rec.project, k) {
                eprintln!("bench: set quota failed: {e}");
                return 1;
            }
            rec.quota_kib = k;
            let _ = bench_record::write_record(&rdir, &rec);
            println!("bench: {name} quota set to {k} KiB (project={})", rec.project);
            0
        }
        None => {
            println!("bench: {name} project={} quota_kib={} state={}", rec.project, rec.quota_kib, rec.state);
            // Best-effort live usage line from repquota (non-fatal).
            let _ = Command::new("repquota").args(["-P", &quota_fs()]).status();
            0
        }
    }
}

/// destroy <name>: tear the Bench down entirely — remove its container, free its project quota, delete
/// its data dir, and remove the record. Idempotent-ish (a missing bench is reported, not fatal-crashed).
fn destroy(name: &str) -> i32 {
    let rdir = bench_record::records_dir();
    let Some(rec) = bench_record::load_record(&rdir, name) else {
        eprintln!("bench: no such bench {name}");
        return 4;
    };
    // Order matters (Fable step-5 fix 6): STOP the container first so no live view survives, THEN tear
    // down egress plumbing and lazy-unmount the FS grants, THEN remove the /run bench dir.
    podman_rm(name);
    net_plane::SandboxNet::for_id(&format!("bench-{}", rec.id)).teardown();
    unmount_grants(&rec);
    let _ = std::fs::remove_dir_all(bench_run_dir(&rec.id));
    let _ = apply_quota(rec.project, 0); // free the project's cap (id becomes reusable once the record is gone)
    let _ = std::fs::remove_dir_all(data_dir(&pool_dir(), name));
    let _ = bench_record::remove_record(&rdir, name);
    println!("bench: destroyed {name}");
    0
}

/// list: print every Bench and its state/quota.
fn list() -> i32 {
    let recs = records();
    if recs.is_empty() {
        println!("bench: no benches");
        return 0;
    }
    println!("NAME                 STATE      PROJECT   QUOTA_KIB");
    for r in recs {
        println!("{:<20} {:<10} {:<9} {}", r.name, r.state, r.project, r.quota_kib);
    }
    0
}

/// reissue: boot-time re-application of the volatile state the records are the source of truth for. `/home`
/// is durable but `/run` is not, so at boot the supervisor re-applies each Bench's project quota (and,
/// step 5, its FS/egress grants). Idempotent. Driven by a `shrek-bench-reissue` oneshot after the pool mount.
fn reissue() -> i32 {
    let recs = records();
    let mut applied = 0;
    let mut grants = 0;
    for r in &recs {
        if r.project != 0 {
            if apply_quota(r.project, r.quota_kib).is_ok() {
                applied += 1;
            }
            // Re-tag the data dir in case the project inherit flag was lost (defensive; cheap).
            let _ = chattr_project(r.project, &data_dir(&pool_dir(), &r.name));
        }
        // /run is volatile: re-materialize the FS grants (host-ns binds) the durable record is the source
        // of truth for (Fable step-5 fix 5 — this is why reissue runs After=home.mount + the pool mount).
        match ensure_grants_materialized(r) {
            Ok(()) => grants += r.grants.iter().filter(|g| matches!(Grant::parse(g), Some(Grant::Fs { .. }))).count(),
            Err(e) => eprintln!("bench reissue: {} FS grant re-materialize failed: {e}", r.name),
        }
        // Sweep any egress plumbing left over from a crash (there is no live container at boot; a granted
        // profile is re-injected on the next `run`, never persisted as a live netns).
        net_plane::SandboxNet::for_id(&format!("bench-{}", r.id)).teardown();
    }
    println!("bench: reissued {applied}/{} bench quota(s), {grants} FS grant(s)", recs.len());
    0
}

// ---- CLI ----------------------------------------------------------------------------------------

/// `gatekeeperd bench <verb> …` — the privileged Bench supervisor. Run as root (the proofs + the shrek
/// front door invoke it privileged); container ops drop to `dev` internally. Returns a process exit code.
pub fn cli(args: &[String]) -> i32 {
    let verb = args.first().map(String::as_str).unwrap_or("");
    let rest = &args[args.len().min(1)..];
    match verb {
        "create" => {
            let mut name = None;
            let mut quota = DEFAULT_QUOTA_KIB;
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--quota" => { i += 1; quota = rest.get(i).and_then(|s| s.parse().ok()).unwrap_or(quota); }
                    other if !other.starts_with('-') && name.is_none() => name = Some(other.to_string()),
                    other => { eprintln!("bench create: unexpected arg {other}"); return 2; }
                }
                i += 1;
            }
            match name {
                Some(n) => create(&n, quota),
                None => { eprintln!("usage: gatekeeperd bench create <name> [--quota KiB]"); 2 }
            }
        }
        "run" | "enter" => {
            let interactive = verb == "enter";
            let mut name = None;
            let mut workload: Vec<String> = Vec::new();
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--" => { workload = rest[i + 1..].to_vec(); break; }
                    other if name.is_none() && !other.starts_with('-') => name = Some(other.to_string()),
                    other => { eprintln!("bench {verb}: unexpected arg {other}"); return 2; }
                }
                i += 1;
            }
            match name {
                Some(n) => run(&n, interactive, &workload),
                None => { eprintln!("usage: gatekeeperd bench {verb} <name> [-- WORKLOAD...]"); 2 }
            }
        }
        "reset" | "destroy" => {
            match rest.first() {
                Some(n) => if verb == "reset" { reset(n) } else { destroy(n) },
                None => { eprintln!("usage: gatekeeperd bench {verb} <name>"); 2 }
            }
        }
        "quota" => {
            match rest.first() {
                Some(n) => quota(n, rest.get(1).and_then(|s| s.parse().ok())),
                None => { eprintln!("usage: gatekeeperd bench quota <name> [KiB]"); 2 }
            }
        }
        "list" => list(),
        "reissue" => reissue(),
        "grant" => {
            // grant <name> <path> [--rw|--ro]  (default ro — least authority)
            let mut name = None;
            let mut path = None;
            let mut rw = false;
            for a in rest {
                match a.as_str() {
                    "--rw" => rw = true,
                    "--ro" => rw = false,
                    other if !other.starts_with('-') && name.is_none() => name = Some(other.to_string()),
                    other if !other.starts_with('-') && path.is_none() => path = Some(other.to_string()),
                    other => { eprintln!("bench grant: unexpected arg {other}"); return 2; }
                }
            }
            match (name, path) {
                (Some(n), Some(p)) => grant(&n, &p, rw),
                _ => { eprintln!("usage: gatekeeperd bench grant <name> <path> [--rw|--ro]"); 2 }
            }
        }
        "network" => {
            match (rest.first(), rest.get(1)) {
                (Some(n), Some(p)) => network(n, p),
                _ => { eprintln!("usage: gatekeeperd bench network <name> <profile|none>"); 2 }
            }
        }
        // The ADR-002 promote path is a later MVP step. Explicit stub so the verb surface is stable and a
        // caller gets a clear "not yet", never a silent success.
        "promote" => {
            eprintln!("bench promote: deferred to a later MVP step (ADR-002 promote → Workshop)");
            3
        }
        "" => { eprintln!("usage: gatekeeperd bench <create|run|enter|grant|network|reset|quota|destroy|list|reissue> …"); 2 }
        other => { eprintln!("bench: unknown verb {other}"); 2 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec<'a>(name: &'a str, data: &'a Path, image: &'a str, interactive: bool, binds: &'a [FsBind], wl: &'a [String]) -> RunSpec<'a> {
        RunSpec {
            name, data, image, interactive,
            detached: false, remove: true,
            hosts_bind: None,
            fs_binds: binds, workload: wl,
        }
    }

    #[test]
    fn podman_run_argv_is_noexec_no_net_workdir_bound() {
        let wl = vec!["ffmpeg".to_string(), "-i".to_string()];
        let a = podman_run_argv(&spec("media", Path::new("/home/.shrek/benches/b/media"), "localhost/scratch", false, &[], &wl));
        let s = a.join(" ");
        assert!(s.contains("--network=none"), "benches start with no egress: {s}");
        assert!(s.contains("--no-hosts"), "avoid the /etc/hosts symlink: {s}");
        assert!(s.contains("--name shrek-bench-media"));
        assert!(s.contains("-v /home/.shrek/benches/b/media:/work"));
        assert!(s.contains("-w /work"));
        assert!(s.ends_with("localhost/scratch ffmpeg -i"));
        assert!(!a.contains(&"-it".to_string()), "non-interactive run has no -it");
    }

    #[test]
    fn seed_freshness_digest_keyed() {
        // sidecar present: only an EXACT id match is fresh (a stale/absent load must re-pull).
        assert!(seed_is_fresh(Some("abc123"), "abc123"), "matching id is fresh");
        assert!(!seed_is_fresh(Some("abc123"), "def456"), "mismatched id (OS-shipped update) is stale");
        assert!(!seed_is_fresh(Some("abc123"), ""), "absent image is not fresh");
        assert!(!seed_is_fresh(Some(""), "abc123"), "empty sidecar never certifies freshness");
        // no sidecar shipped: fall back to load-if-absent.
        assert!(seed_is_fresh(None, "anything"), "no sidecar → any loaded image counts as present");
        assert!(!seed_is_fresh(None, ""), "no sidecar + absent → must load");
    }

    #[test]
    fn seed_tar_default_and_override() {
        // default lives in the sysext seeds dir; SHREK_BENCH_SEED_TAR steers the oracle's staged archive.
        std::env::remove_var("SHREK_BENCH_SEED_TAR");
        assert_eq!(seed_tar(), PathBuf::from("/usr/share/shrek/bench/seeds/scratch.tar"));
        std::env::set_var("SHREK_BENCH_SEED_TAR", "/tmp/x/scratch.tar");
        assert_eq!(seed_tar(), PathBuf::from("/tmp/x/scratch.tar"));
        std::env::remove_var("SHREK_BENCH_SEED_TAR");
    }

    #[test]
    fn enter_argv_is_interactive() {
        let wl = vec!["/bin/sh".to_string()];
        let a = podman_run_argv(&spec("b", Path::new("/p/b"), "img", true, &[], &wl));
        assert!(a.contains(&"-it".to_string()), "enter is interactive");
    }

    #[test]
    fn run_argv_binds_grants_ro_rw_noexec_default_userns() {
        let binds = vec![
            FsBind { target: PathBuf::from("/run/shrek/bench/media/grants/in"), leaf: "in".into(), rw: false },
            FsBind { target: PathBuf::from("/run/shrek/bench/media/grants/out"), leaf: "out".into(), rw: true },
        ];
        let wl = vec!["ffmpeg".to_string()];
        let a = podman_run_argv(&spec("media", Path::new("/d"), "img", false, &binds, &wl));
        let s = a.join(" ");
        // Default rootless mapping (container-root ⇔ host-dev) — NOT keep-id (which makes container-root a
        // subuid that can't write the dev-owned grant; the oracle caught exactly that).
        assert!(!s.contains("--userns"), "granted bench uses default userns, not keep-id: {s}");
        // ro grant is ro,noexec,nodev,nosuid at /grants/<leaf>; rw grant is rw + the same hardening.
        assert!(s.contains("-v /run/shrek/bench/media/grants/in:/grants/in:ro,noexec,nodev,nosuid"), "{s}");
        assert!(s.contains("-v /run/shrek/bench/media/grants/out:/grants/out:rw,noexec,nodev,nosuid"), "{s}");
    }

    #[test]
    fn networked_run_argv_is_detached_no_rm_and_binds_bench_hosts() {
        let hosts = PathBuf::from("/run/shrek/bench/media/hosts");
        let wl = vec!["curl".to_string()];
        let rs = RunSpec {
            name: "media", data: Path::new("/d"), image: "img", interactive: false,
            detached: true, remove: false, hosts_bind: Some(&hosts),
            fs_binds: &[], workload: &wl,
        };
        let a = podman_run_argv(&rs);
        let s = a.join(" ");
        assert!(a.contains(&"-d".to_string()), "networked run is detached (inject before egress): {s}");
        assert!(!a.contains(&"--rm".to_string()), "detached run keeps the container until `podman wait` reads rc");
        assert!(s.contains("--network=none"), "still --network=none — egress is INJECTED into this netns");
        assert!(s.contains("-v /run/shrek/bench/media/hosts:/etc/hosts:ro"), "bench-owned hosts bind: {s}");
    }

    #[test]
    fn grant_roundtrips_and_unknown_grant_forms_are_ignored() {
        let g = Grant::Fs { rw: true, path: PathBuf::from("/home/dev/videos") };
        assert_eq!(Grant::parse(&g.encode()), Some(g));
        let n = Grant::Net { profile: "github-https".into() };
        assert_eq!(Grant::parse(&n.encode()), Some(n));
        assert_eq!(Grant::parse("fs-ro /home/dev/in"), Some(Grant::Fs { rw: false, path: PathBuf::from("/home/dev/in") }));
        assert_eq!(Grant::parse("bogus x"), None); // unknown kind ⇒ ignored, never mis-applied
        assert_eq!(Grant::parse("net"), None); // no value
    }

    #[test]
    fn fs_leaf_rejects_unsafe_basenames() {
        assert_eq!(Grant::fs_leaf(Path::new("/home/dev/videos")).as_deref(), Some("videos"));
        assert_eq!(Grant::fs_leaf(Path::new("/home/dev/.ssh")), None); // leading dot reserved
        assert_eq!(Grant::fs_leaf(Path::new("/home/dev")), Some("dev".to_string())); // basename token
    }

    #[test]
    fn only_sealed_egress_profiles_validate() {
        // network verb gates on this exact lookup — default-deny for anything not compiled into policy.
        assert!(shrek_policy::egress::resolve("github-https").is_some());
        assert!(shrek_policy::egress::resolve("model-anthropic").is_some());
        assert!(shrek_policy::egress::resolve("allow-all").is_none());
        assert!(shrek_policy::egress::resolve("").is_none());
    }

    #[test]
    fn egress_profile_reads_the_single_net_grant() {
        let mut r = BenchRecord {
            name: "b".into(), id: "b".into(), project: 100_000, quota_kib: 1024,
            created: 0, state: "created".into(),
            grants: vec!["fs-ro /home/dev/in".into(), "net github-https".into()],
        };
        assert_eq!(egress_profile(&r), Some("github-https".to_string()));
        r.grants = vec!["fs-rw /home/dev/out".into()];
        assert_eq!(egress_profile(&r), None);
    }

    #[test]
    fn setquota_argv_sets_only_the_block_hard_limit() {
        // -P <proj> block-soft(0) block-hard(kib) inode-soft(0) inode-hard(0) <fs>
        assert_eq!(
            setquota_argv(100_000, 4096, "/home"),
            vec!["-P", "100000", "0", "4096", "0", "0", "/home"]
        );
        // kib=0 clears the cap (hard 0 = unlimited) — the destroy/free path.
        assert_eq!(setquota_argv(100_000, 0, "/home")[3], "0");
    }
}
