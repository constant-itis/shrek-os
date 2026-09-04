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
use crate::linux_uapi::{umount2, Ucred};
use crate::mount_plane::{open_anchor, pin_beneath, relocate_ro, relocate_rw, Ident};
use crate::net_plane;
use crate::workshop_cache::{self, CacheRecord};
use crate::workshop_record::{self, valid_pkg_token, WorkshopRecord};
use std::ffi::CString;
use std::io;
use std::io::Write as _;
use std::os::fd::RawFd;
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
    bench_record::bench_env("SHREK_BENCH_POOL").unwrap_or_else(|| "/home/.shrek/benches".to_string()).into()
}

/// A Bench's own quota-scoped data directory, bound into the container at `/work`. Beneath `<pool>/b/`.
fn data_dir(pool: &Path, name: &str) -> PathBuf {
    pool.join("b").join(name)
}

/// The filesystem the ext4 project quota is set on (the shrek-data `/home`). Overridable for the oracle.
fn quota_fs() -> String {
    bench_record::bench_env("SHREK_BENCH_FS").unwrap_or_else(|| "/home".to_string())
}

/// One entry in the SEALED seed catalog: the fixed set of Bench base images the OS ships, each an offline
/// OCI-archive baked into the shrek-bench sysext. A Bench NAMES one at `create --seed <name>` (default
/// `scratch`); the name is validated against this list exactly like an egress profile name — an unknown
/// seed is REFUSED, never substituted. `image` = the fully-qualified local tag podman resolves against the
/// local store only (registries.conf search is empty); `tar` = the archive basename `ensure_seed` loads.
struct Seed {
    name: &'static str,
    image: &'static str,
    tar: &'static str,
}

/// The sealed catalog. `scratch` = the tiny Alpine/media proof seed (musl); `debian` = the apt+pip WORKSHOP
/// seed (glibc + apt over `debian-apt` + python3/pip/venv over `pypi-https`, composed via the repeatable
/// `network` verb). Extend HERE; a user can never add a seed, only pick a shipped one. A dedicated python
/// seed was considered and rejected: the debian seed already carries apt, so `apt` + `pip` in ONE bench is
/// the natural workshop base (and the only way to `pip install` an sdist that needs an apt-installed
/// compiler, since containers are `--rm` per run — apt state is per-session).
const SEED_CATALOG: &[Seed] = &[
    Seed { name: "scratch", image: "localhost/scratch", tar: "scratch.tar" },
    Seed { name: "debian", image: "localhost/debian", tar: "debian.tar" },
];

fn seed_lookup(name: &str) -> Option<&'static Seed> {
    SEED_CATALOG.iter().find(|s| s.name == name)
}

/// Is `name` a shipped seed? `create --seed` validates against this, fail-closed (unknown ⇒ refused).
pub(crate) fn valid_seed(name: &str) -> bool {
    seed_lookup(name).is_some()
}

/// The dir holding the baked seed OCI-archives. Overridable for the oracle (which stages its own tars)
/// via `SHREK_BENCH_SEED_DIR` in the `oracle-env` build only.
fn seeds_dir() -> PathBuf {
    bench_record::bench_env("SHREK_BENCH_SEED_DIR")
        .unwrap_or_else(|| "/usr/share/shrek/bench/seeds".to_string())
        .into()
}

/// The offline seed image a given Bench runs from (a NAME resolved by podman against the local store only).
/// From the sealed catalog; the oracle may override a single seed's tag via `SHREK_BENCH_SEED_IMG_<NAME>`.
fn seed_image(seed: &str) -> String {
    if let Some(o) = bench_record::bench_env(&format!("SHREK_BENCH_SEED_IMG_{}", seed.to_uppercase())) {
        return o;
    }
    seed_lookup(seed).map(|s| s.image.to_string()).unwrap_or_else(|| "localhost/scratch".to_string())
}

/// A seed's OCI-archive, baked into the shrek-bench sysext and `podman load`ed into `dev`'s rootless store
/// on demand ([`ensure_seed`]). `podman load` is the step-6 de-risk winner: it reads a plain file + writes
/// layers to the /home graphroot (a depth-1 ext4 mount), whereas an `additionalimagestores` on the
/// already-overlayed merged /usr risks the kernel's overlay stacking-depth-2 limit.
fn seed_tar(seed: &str) -> PathBuf {
    let base = seed_lookup(seed).map(|s| s.tar).unwrap_or("scratch.tar");
    seeds_dir().join(base)
}

/// The trusted anchor grants are resolved strictly beneath (rule 3 / mount_plane TOCTOU model). A Bench
/// is USER-authority, so the anchor is the desktop user's home: every grant is a real dir under it,
/// hence `dev`-owned by construction (so `dev`'s rootless podman reads/writes it and writes round-trip
/// to `dev` — proven in the step-5 de-risk). Overridable for the oracle via `SHREK_BENCH_ANCHOR`.
fn anchor_dir() -> PathBuf {
    bench_record::bench_env("SHREK_BENCH_ANCHOR").unwrap_or_else(|| format!("/home/{BENCH_USER}")).into()
}

/// Per-Bench VOLATILE runtime dir on `/run` (grants + the bench-owned hosts file). `/run` is tmpfs, so
/// everything here is rebuilt at boot by `reissue` (the durable record on `/home` is the source of truth).
/// Overridable via `SHREK_BENCH_RUN` (the oracle has no `/run/shrek`).
fn bench_run_dir(id: &str) -> PathBuf {
    let base = bench_record::bench_env("SHREK_BENCH_RUN").unwrap_or_else(|| "/run/shrek/bench".to_string());
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

// ---- exports (step 7): the constrained .desktop launcher plane ------------------------------------

/// %-escape the bytes that would break the space-delimited one-line record wire form (space, `%`, and any
/// control char) so a label/argv arg round-trips FAITHFULLY (Fable step-7 fix 3: a space-join is lossy).
/// Also the per-arg encoding for the count-framed `BENCH` socket request (one newline-free line per arg).
pub fn pct_encode(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '%' || c == ' ' || c.is_control() {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                o.push_str(&format!("%{b:02X}"));
            }
        } else {
            o.push(c);
        }
    }
    o
}

/// Inverse of [`pct_encode`]. Unknown/short `%` sequences pass through literally (never panics).
pub fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut o = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 3 <= b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                o.push(v);
                i += 3;
                continue;
            }
        }
        o.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&o).into_owned()
}

/// A launcher export: a fixed KEY (mirrors shrek-menu's baked provider key — the untrusted `.desktop`
/// carries the key, the trusted root-owned record carries the command). `file` is the exact `.desktop`
/// basename written (recorded so unexport/destroy delete precisely it — Fable step-7 fix 4). `label`/`cmd`
/// are `%`-escaped on the wire (fix 3). Wire: `<key> <file> <icon> <label%> <arg0%> [arg1% …]`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Export {
    key: String,
    file: String,
    icon: String,
    label: String,
    cmd: Vec<String>,
}

impl Export {
    fn parse(s: &str) -> Option<Export> {
        let mut it = s.split(' ');
        let key = it.next()?.to_string();
        let file = it.next()?.to_string();
        let icon = it.next()?.to_string();
        let label = pct_decode(it.next()?);
        let cmd: Vec<String> = it.map(pct_decode).collect();
        if cmd.is_empty() {
            return None;
        }
        Some(Export { key, file, icon, label, cmd })
    }

    fn encode(&self) -> String {
        let mut parts = vec![self.key.clone(), self.file.clone(), self.icon.clone(), pct_encode(&self.label)];
        parts.extend(self.cmd.iter().map(|a| pct_encode(a)));
        parts.join(" ")
    }
}

/// An icon is a freedesktop icon NAME (never a path — an icon path under a bench-writable grant would let a
/// workload control launcher-rendered pixels = UI spoofing, Fable step-7 fix 3-icon). Safe token, no `/`.
fn valid_icon(i: &str) -> bool {
    !i.is_empty()
        && i.len() <= 128
        && !i.starts_with('.')
        && i.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// A `.desktop` basename is safe to `rm`/`mv` (no traversal/space) — defence in depth even though the name
/// is generated from validated tokens and stored in the root-owned record.
fn valid_desktop_file(f: &str) -> bool {
    f.ends_with(".desktop")
        && f.len() <= 200
        && !f.contains("..")
        && f.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Sanitize a user-supplied `.desktop` label: drop control chars (no line/entry forging), cap length, trim.
/// `None` (absent/empty) ⇒ the caller uses a computed default.
fn sanitize_label(opt: Option<&str>) -> Option<String> {
    let s = opt?;
    let cleaned: String = s.chars().filter(|c| !c.is_control()).take(64).collect();
    let t = cleaned.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Dev's XDG applications dir — where launchers (DMS + shrek-menu apps provider, #2827) discover entries.
/// Under the user-authority anchor (dev's home), so the per-user exports sit beside nothing sealed.
fn desktop_apps_dir() -> PathBuf {
    anchor_dir().join(".local/share/applications")
}

/// The constrained `.desktop` body: absolute `Exec` to the baked wrapper with exactly two charset-validated
/// tokens, NO field codes (`%f`/`%u`/… — so launchers pass no file args), `Terminal=false`, a GenericName
/// badge so an export can't silently masquerade as a baked app. `bench`/`key` are safe tokens (no space/
/// newline) ⇒ Exec injection is impossible by construction. `label` backslash-escaped per the Desktop spec.
fn desktop_content(bench: &str, key: &str, label: &str, icon: &str) -> String {
    let name = label.replace('\\', "\\\\");
    format!(
        "[Desktop Entry]\n\
         Version=1.0\n\
         Type=Application\n\
         Name={name}\n\
         GenericName=Shrek Bench app\n\
         Comment=Runs in the {bench} Bench (Shrek OS)\n\
         Exec=/usr/bin/shrek-bench-run {bench} {key}\n\
         Icon={icon}\n\
         Terminal=false\n\
         Categories=Utility;\n\
         X-Shrek-Bench={bench}\n"
    )
}

/// Write a `.desktop` into dev's applications dir AS DEV — never as root: root creating a file inside a
/// dev-controlled dir is a symlink-redirect root-write gadget (Fable step-7 fix 2). Refuses to overwrite an
/// existing file (⇒ `AlreadyExists`, the cross-bench filename-collision guard, fix 4). Content flows over
/// stdin so it never transits argv; the dir + basename are positional args (validated, no interpolation).
fn write_desktop_as_dev(file: &str, content: &str) -> io::Result<()> {
    let dir = desktop_apps_dir();
    let uid = dev_uid();
    let script = r#"d="$1"; f="$2"; mkdir -p "$d" || exit 1; [ -e "$d/$f" ] && exit 3; t="$d/.$f.tmp.$$"; cat > "$t" || exit 1; chmod 644 "$t" || exit 1; mv "$t" "$d/$f""#;
    let mut child = Command::new("runuser")
        .arg("-u").arg(BENCH_USER).arg("--")
        .arg("env").arg("HOME=/home/dev").arg(format!("XDG_RUNTIME_DIR=/run/user/{uid}")).arg("PATH=/usr/bin:/bin")
        .arg("sh").arg("-c").arg(script)
        .arg("sh").arg(dir.to_string_lossy().to_string()).arg(file)
        .stdin(Stdio::piped())
        .spawn()?;
    child.stdin.take().unwrap().write_all(content.as_bytes())?;
    match child.wait()?.code() {
        Some(0) => Ok(()),
        Some(3) => Err(io::Error::new(io::ErrorKind::AlreadyExists, "a .desktop with this name already exists (bench/key collision)")),
        other => Err(io::Error::new(io::ErrorKind::Other, format!("desktop write (as dev) failed (rc {other:?})"))),
    }
}

/// Remove an exported `.desktop` AS DEV (idempotent). Same trust reason as the writer: never root-unlink in
/// dev's dir. The basename is validated + came from the root-owned record.
fn remove_desktop_as_dev(file: &str) {
    if !valid_desktop_file(file) {
        return;
    }
    let dir = desktop_apps_dir();
    let uid = dev_uid();
    let _ = Command::new("runuser")
        .arg("-u").arg(BENCH_USER).arg("--")
        .arg("env").arg("HOME=/home/dev").arg(format!("XDG_RUNTIME_DIR=/run/user/{uid}")).arg("PATH=/usr/bin:/bin")
        .arg("rm").arg("-f").arg(dir.join(file))
        .status();
}

/// The record's egress policy SET (may be empty). Multiple `net` grants COMPOSE — the bench reaches the
/// UNION of their sealed destinations (resolved by [`net_plane::resolve_profiles_v4`]). Ordered as
/// recorded. This replaces the old first-wins accessor: a first-wins reader on a multi-`net` record would
/// inject only profile #1 while the record — and the human's consent — name several (Fable item-1 fix 3),
/// a silent under-injection. Every call site now takes the whole set.
fn egress_profiles(rec: &BenchRecord) -> Vec<String> {
    rec.grants
        .iter()
        .filter_map(|g| match Grant::parse(g) {
            Some(Grant::Net { profile }) => Some(profile),
            _ => None,
        })
        .collect()
}

/// Validate a declarative egress-profile SET (the argument list to the `network` verb). Returns the
/// canonical set (input order preserved) or a human-facing error. Single source of truth shared by the
/// root `cli()` path ([`network`]) and the consent path ([`precheck_network`]) so they can never drift.
/// Rules: `none` ALONE ⇒ the empty set (revoke-all); `none` mixed with any real profile is REFUSED;
/// every real name must resolve in sealed policy (fail-closed on the first unknown, changing nothing);
/// duplicates are REFUSED (clear error, never a silent dedup); a `-`-prefixed token is refused (the set
/// holds profile names, not flags). An empty input is a usage error (use `none` to revoke).
fn validate_profile_set(profiles: &[String]) -> Result<Vec<String>, String> {
    if profiles.is_empty() {
        return Err("usage: network <name> <profile...|none>".into());
    }
    if profiles.iter().any(|p| p == "none") {
        if profiles.len() != 1 {
            return Err("`none` REVOKES all egress and cannot be combined with a profile".into());
        }
        return Ok(Vec::new());
    }
    let mut set: Vec<String> = Vec::with_capacity(profiles.len());
    for p in profiles {
        if p.starts_with('-') {
            return Err(format!("{p:?} is not a profile name (no flags in the egress set)"));
        }
        if shrek_policy::egress::resolve(p).is_none() {
            return Err(format!("{p:?} is not a sealed egress profile — refused (policy is default-deny)"));
        }
        if set.iter().any(|q| q == p) {
            return Err(format!("duplicate profile {p:?} in the set"));
        }
        set.push(p.clone());
    }
    Ok(set)
}

/// The consent diff rows naming a network SET: ONE row per profile (never a single joined string — the
/// consent renderer sanitizes per value, and a name buried mid-line in a joined string could be lost;
/// Fable item-1 fix 4). The caller appends the I/O-derived endpoint count + any "Replaces egress" row. Pure.
fn network_profile_rows(set: &[String]) -> Vec<(String, String)> {
    set.iter().map(|p| ("Egress profile".to_string(), p.clone())).collect()
}

/// Default per-Bench block quota (KiB). Generous but bounded so one Bench cannot fill `/home`.
pub const DEFAULT_QUOTA_KIB: u64 = 4 * 1024 * 1024; // 4 GiB

/// The desktop user rootless podman runs as. uid resolved from /etc/passwd; the runtime dir is its logind
/// `XDG_RUNTIME_DIR`. (Bench containers run under `dev`'s delegated `user-<uid>.slice`, proven in Bench-0.)
const BENCH_USER: &str = "dev";

/// The uid the Bench plane belongs to (`dev`) — the only non-root uid allowed to drive an
/// authority-increasing bench verb through the consent ceremony (`shrek` is refused; root uses `cli()`).
pub(crate) fn bench_user_uid() -> u32 {
    dev_uid()
}

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

/// `dev`'s PRIMARY gid (passwd field 4), NOT assumed equal to its uid. The grant-dir traverse story
/// (mycelium #2982 hole 3) rests on group-owner == dev's real login group, so a bad guess would strip
/// dev's group `--x` and brick every `podman -v` (fail-closed). Parsed from /etc/passwd like [`dev_uid`].
fn dev_gid() -> u32 {
    std::fs::read_to_string("/etc/passwd")
        .ok()
        .and_then(|p| {
            p.lines().find_map(|l| {
                let f: Vec<&str> = l.split(':').collect();
                (f.len() >= 4 && f[0] == BENCH_USER).then(|| f[3].parse::<u32>().ok()).flatten()
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
    /// `--rm`: auto-remove on exit (the plain foreground path; the detached holder+exec path removes
    /// explicitly AFTER the workload exec returns).
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

/// Build the `podman exec` argv for the holder+exec networked path (Fable item-2). Every flag comes
/// STRICTLY BEFORE the container name: podman stops parsing options at the first positional (the ctr), so a
/// workload arg like `-c` survives as the command, not as an exec flag. `--workdir /work` pins the cwd to
/// the run's `-w /work` (podman exec already INHERITS the container's configured workdir, so this is a
/// drift-pin, not a fix); `-it` is added for the interactive shell only.
fn podman_exec_argv(ctr: &str, interactive: bool, workload: &[String]) -> Vec<String> {
    let mut a: Vec<String> = vec!["exec".into()];
    if interactive {
        a.push("-it".into());
    }
    a.push("--workdir".into());
    a.push("/work".into());
    a.push(ctr.to_string());
    a.extend(workload.iter().cloned());
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
fn podman_cmd_as_dev(args: &[String]) -> Command {
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
    cmd
}

fn podman_as_dev(args: &[String]) -> io::Result<i32> {
    Ok(podman_cmd_as_dev(args).status()?.code().unwrap_or(-1))
}

/// Like [`podman_as_dev`] but DISCARDS the child's stdout (stderr still inherits, for diagnostics). Used for
/// podman sub-commands whose stdout is control NOISE the caller must never see — `run -d` echoes the new
/// container ID, `rm` echoes the container name — so that on the holder+exec networked path the ONLY thing
/// on the caller's stdout is the workload exec's own output (clean stdout streaming, Fable item-2).
fn podman_as_dev_quiet(args: &[String]) -> io::Result<i32> {
    Ok(podman_cmd_as_dev(args).stdout(std::process::Stdio::null()).status()?.code().unwrap_or(-1))
}

/// Best-effort force-remove of a Bench's container (as `dev`). `-t 0` = SIGKILL IMMEDIATELY: the networked
/// path's PID1 is a `sleep infinity` holder that IGNORES SIGTERM from an ancestor pidns, so the default
/// `rm -f` (SIGTERM → wait the 10s StopSignal timeout → SIGKILL) would stall every teardown 10s. Quiet so
/// the removed-container name never lands on the caller's stdout. Idempotent — a missing container is fine.
fn podman_rm(name: &str) {
    let _ = podman_as_dev_quiet(&["rm".into(), "-f".into(), "-t".into(), "0".into(), format!("shrek-bench-{name}")]);
}

/// Like [`podman_as_dev`] but CAPTURES stdout (for `podman inspect`, which prints the value
/// to stdout and returns 0 itself). Trimmed on the caller side.
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
fn ensure_seed(seed: &str) {
    let tar = seed_tar(seed);
    if !tar.exists() {
        return; // no baked seed on this host — the image is provided by other means (oracle/test).
    }
    let want = std::fs::read_to_string(format!("{}.digest", tar.display()))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let have = podman_as_dev_stdout(&[
        "image".into(), "inspect".into(), "--format".into(), "{{.Id}}".into(), seed_image(seed),
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

// ---- Tool Shed cache helpers (ADR-003 Commit 3) -------------------------------------------------
//
// The cache KEY hashes derivation INPUTS (seed image Id + declared apt/pip sets + egress set); a HIT
// additionally requires the live podman image Id to equal the Id the supervisor recorded at store time.
// Trust lives in the root-owned index record, never in the image bytes (mycelium #2994, workshop_cache).

/// SHA-256 (lowercase hex) of `input`, computed by shelling the sealed `sha256sum` over STDIN (there is no
/// dep-free hash crate in gatekeeperd, and the whole daemon shells to sealed binaries — `podman`/`ip`/`nft`
/// — by design). STDIN, never argv, so the hashed bytes can carry newlines and are never a shell/flag
/// surface. Fail-closed: any spawn/write/parse error ⇒ `None` (the caller then skips caching and re-derives
/// a transient — a hash failure must never fabricate or collide a key).
fn sha256_hex_stdin(input: &str) -> Option<String> {
    let mut child = Command::new("sha256sum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(input.as_bytes()).ok()?;
    let out = child.wait_with_output().ok()?;
    if !out.status.success() {
        return None;
    }
    // `sha256sum` prints `<64hex>  -`; take the first whitespace-delimited field.
    let hex = String::from_utf8_lossy(&out.stdout);
    let key = hex.split_whitespace().next()?.to_string();
    workshop_cache::valid_cache_key(&key).then_some(key)
}

/// The podman image Id of a tag (as `dev`, whose rootless store holds the bytes). `None` if the image is
/// absent or podman prints nothing — so an absent cached image reads as a MISS, and an absent seed image
/// makes the key uncomputable (fail-closed) rather than keying on a guessed value.
fn podman_image_id(tag: &str) -> Option<String> {
    let id = podman_as_dev_stdout(&[
        "image".into(), "inspect".into(), "--format".into(), "{{.Id}}".into(), tag.to_string(),
    ]).ok()?.trim().to_string();
    (!id.is_empty()).then_some(id)
}

/// The seed's live image Id, the way the cache key must see it: `ensure_seed` first (so an OS-shipped seed
/// update is loaded and the Id reflects the CURRENT seed), then inspect. `None` (fail-closed) if the seed
/// image can't be resolved — the launch then derives a transient without caching rather than key on nothing.
fn seed_image_id(seed: &str) -> Option<String> {
    ensure_seed(seed);
    podman_image_id(&seed_image(seed))
}

/// The HIT check (mycelium #2994): a cache entry is a HIT iff (1) a root-owned index record exists at `key`,
/// AND (2) the live podman image Id equals the Id that record captured. A missing/malformed record ⇒ MISS.
/// An Id mismatch or an absent image ⇒ MISS **and** the stale index record is deleted (self-healing: the
/// bytes it pointed at are gone or were replaced, so the record is worthless — the next launch re-derives
/// and re-stores). Returns the record's image tag to run the workload from, on a HIT.
fn cache_lookup(key: &str) -> Option<CacheRecord> {
    let dir = workshop_cache::cache_dir();
    let rec = workshop_cache::load_record(&dir, key)?;
    match podman_image_id(&rec.image) {
        Some(live) if live == rec.image_id => Some(rec),
        other => {
            eprintln!(
                "workshop launch: cache Id {} for {} — evicting stale index record (fail-closed → re-derive)",
                if other.is_some() { "mismatch" } else { "absent (image gone)" },
                rec.image,
            );
            let _ = workshop_cache::remove_record(&dir, key);
            None
        }
    }
}

/// Write the Tool Shed index record for a freshly derived+committed image at `tag`. The recorded `image_id`
/// is the just-committed image's live Id (what a future HIT re-verifies against). Best-effort: a failure to
/// read the Id or write the record logs and leaves the derivation UN-cached (the next launch re-derives) —
/// a cache write must never fail an otherwise-successful launch.
fn cache_store(key: &str, tag: &str, seed_id: &str, apt_resolved: Vec<String>, pip_resolved: Vec<String>) {
    let Some(image_id) = podman_image_id(tag) else {
        eprintln!("workshop launch: could not read derived image Id for {tag} — leaving un-cached");
        return;
    };
    let rec = CacheRecord {
        key: key.to_string(),
        image: tag.to_string(),
        image_id,
        seed_id: seed_id.to_string(),
        created: now_secs(),
        apt_resolved,
        pip_resolved,
    };
    match workshop_cache::write_record(&workshop_cache::cache_dir(), &rec) {
        Ok(_) => eprintln!("gatekeeperd/bench_plane: cache STORE key={} image={tag}", &key[..12]),
        Err(e) => eprintln!("workshop launch: cache index write failed (continuing, un-cached): {e}"),
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

/// Create the per-Bench `/run` grant dir chain, hardened against the FS-grant redirect (mycelium #2982
/// hole 3). Both the `<id>` and `grants` dirs are `root:dev` mode `0710`: root OWNS them, so `dev` can
/// neither plant a symlink leaf inside `grants` nor `rename(2)` `grants`/`<id>` aside to substitute a
/// forged one — either would let `relocate_*`'s symlink-following `create_dir_all`+`mount` redirect the
/// bind onto an ungranted system target (e.g. /etc). `dev` (primary group `dev`) still gets group `--x`
/// to TRAVERSE both dirs for its rootless-podman `-v`, but NO write; `other ---` preserves Fable step-5
/// fix-1 (no OTHER unprivileged service can follow the bind into `dev`'s home). The `/run/shrek/bench`
/// container stays root `0755` (just a namespace; dev can't rename `<id>` out of it either).
fn prepare_grant_dir(id: &str) -> io::Result<()> {
    use std::fs::DirBuilder;
    use std::os::unix::fs::DirBuilderExt;
    let bench = bench_run_dir(id);
    let grants = grants_dir(id);
    // parents: /run/shrek + /run/shrek/bench (root 0755).
    if let Some(container) = bench.parent() {
        std::fs::create_dir_all(container)?;
    }
    let gid = dev_gid();
    for d in [&bench, &grants] {
        // Create root-owned + mode 0710 FROM BIRTH (DirBuilder::mode, not create_dir_all + a later chmod):
        // until the chown below the group is root's gid 0, so `other ---` gives `dev` ZERO access — there
        // is no umask-0 window in which `dev` could plant a symlink leaf before the perms tighten. On
        // reissue the dir already exists (AlreadyExists ⇒ ignore); the set_permissions + chown then
        // re-harden it. Group-write is never set at any point (0710), so the chown ordering is race-safe.
        match DirBuilder::new().mode(0o710).create(d) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o710))?;
        // root owner, `dev` GROUP (dev's primary gid gets the `--x` traverse bit). NEVER dev-owned.
        let _ = chown(d, Some(0), Some(gid));
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
/// sealed host). 0644 under the `root:dev 0710` bench dir (root writes it; dev traverses to read it).
/// Written BEFORE `podman run` (fix 6).
fn write_hosts(id: &str, resolved: &net_plane::Resolved) -> io::Result<PathBuf> {
    prepare_grant_dir(id)?; // ensures the root:dev 0710 bench dir exists (dev cannot swap it)
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

/// create <name> [--quota KiB] [--seed NAME]: allocate a project id, make the Bench data dir on the pool,
/// tag it with the project id, cap it, chown to `dev`, and write the durable record. Fails if the Bench
/// exists or the seed name is not in the sealed catalog. `seed` defaults to `scratch`.
fn create(name: &str, quota_kib: u64, seed: &str) -> i32 {
    if !bench_record::valid_bench_name(name) {
        eprintln!("bench: invalid name {name:?}");
        return 2;
    }
    if !valid_seed(seed) {
        let known: Vec<&str> = SEED_CATALOG.iter().map(|s| s.name).collect();
        eprintln!("bench: unknown seed {seed:?} — the OS ships {known:?} (a seed is a sealed base image, not user-chosen)");
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
            seed: seed.to_string(),
            grants: vec![],
            exports: vec![],
        };
        bench_record::write_record(&rdir, &rec)?;
        Ok(())
    };
    match build() {
        Ok(()) => {
            println!("bench: created {name} (project={project} quota_kib={quota_kib} seed={seed} data={})", data.display());
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
    run_from(name, interactive, workload, None)
}

/// The `run` core, parameterized by an optional IMAGE OVERRIDE. `None` = the Bench's sealed seed (the
/// normal path). `Some(tag)` = run FROM a specific local image instead of the seed — used by `workshop
/// launch` to run the workload container from its already-derived (installed) image while the record's
/// grants/egress/quota still apply. Everything else (grant materialization, holder+exec egress, teardown)
/// is identical, so the two callers share one implementation.
fn run_from(name: &str, interactive: bool, workload: &[String], image_override: Option<&str>) -> i32 {
    let rdir = bench_record::records_dir();
    let Some(mut rec) = bench_record::load_record(&rdir, name) else {
        eprintln!("bench: no such bench {name}");
        return 4;
    };
    // Ensure the Bench's offline seed is loaded into dev's store (load-if-absent-or-stale from its archive).
    // With an image override the workload runs from the derived image, but the seed is still ensured (the
    // override was derived FROM it, and a plain-path fallback would need it) — cheap + idempotent.
    ensure_seed(&rec.seed);
    let image = image_override.map(|s| s.to_string()).unwrap_or_else(|| seed_image(&rec.seed));
    // Materialize FS grants in the host ns (idempotent — no-op if already mounted). Fail-closed.
    if let Err(e) = ensure_grants_materialized(&rec) {
        eprintln!("bench run: FS grant materialization failed (fail-closed): {e}");
        return 1;
    }
    let fs_binds = fs_binds_for(&rec);
    let data = data_dir(&pool_dir(), name);
    let wl = if interactive && workload.is_empty() { vec!["/bin/sh".to_string()] } else { workload.to_vec() };

    let profiles = egress_profiles(&rec);
    if !profiles.is_empty() {
        // The networked path is holder+exec (egress-before-workload): it starts a `sleep infinity` holder,
        // injects egress, then EXECs the workload into the now-networked container. `podman exec` with NO
        // command is a usage error (125) AFTER the netns/veth/nft are built, so refuse an empty
        // non-interactive workload up front (Fable item-2 fix 1). Interactive `wl` already defaulted to
        // /bin/sh above, so only the non-interactive path can be empty here. (The PLAIN path below still
        // runs the image default for an empty workload — unchanged.)
        if !interactive && wl.is_empty() {
            eprintln!("bench run: a networked run requires a workload after `--`");
            return 2;
        }
        return run_networked(&mut rec, interactive, &data, &image, &profiles, &fs_binds, &wl);
    }

    let spec = RunSpec {
        name,
        data: &data,
        image: &image,
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
/// user-authority Bench (Fable step-5 ruling 3).
///
/// HOLDER+EXEC (Fable item-2, egress-before-workload): BOTH interactive and non-interactive start a `sleep
/// infinity` HOLDER as PID1, then — only after egress is up and re-verified — `podman exec` the workload
/// (or a shell) INTO the networked container. So the workload has egress from its FIRST instruction (no
/// late-attach race, no retry-until-egress wrapper). Because the holder is a stable PID1 that never exits
/// on its own, the leader/netns identity stays fixed across discover→inject→re-verify→exec — the
/// drift/pid-recycle guards are STRENGTHENED (the untrusted workload is never in that window). The
/// non-interactive rc is the workload's own exec status (126/127/128+n pass through; a podman-INFRA failure
/// maps to the -1 sentinel; 125 is ambiguous — podman-error vs a workload exiting 125 — identical to the
/// plain foreground path, so the contract is uniform across both run paths).
fn run_networked(rec: &mut BenchRecord, interactive: bool, data: &Path, image: &str, profiles: &[String], fs_binds: &[FsBind], workload: &[String]) -> i32 {
    let rdir = bench_record::records_dir();
    let name = rec.name.clone();
    let ctr = format!("shrek-bench-{name}");

    // Resolve EVERY recorded profile against sealed policy BEFORE any container start: a single unknown
    // recorded name refuses the whole run (fail-closed), exactly the single-profile behavior generalized
    // (Fable item-1 fix 3). The union itself is [`net_plane::resolve_profiles_v4`]'s job (endpoints deduped
    // by (ip,proto,port), /etc/hosts by name) — a single-element set is byte-for-byte the legacy result.
    let mut profile_refs = Vec::with_capacity(profiles.len());
    for p in profiles {
        match shrek_policy::egress::resolve(p) {
            Some(r) => profile_refs.push(r),
            None => {
                eprintln!("bench run: recorded egress profile {p:?} is not in sealed policy — refusing (fail-closed)");
                return 1;
            }
        }
    }
    let resolved = match net_plane::resolve_profiles_v4(&profile_refs) {
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
    // ALWAYS start the holder as PID1 (Fable item-2): the untrusted workload is exec'd AFTER egress is up,
    // never run as PID1 during the no-egress window.
    let start_wl: &[String] = &holder;
    let spec = RunSpec {
        name: &name,
        data,
        image,
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

    // Quiet: `podman run -d` echoes the new container ID to stdout, which we DON'T use (the leader is found
    // via `podman_pid` inspect) — discard it so it never pollutes the workload exec's stdout on the caller.
    if podman_as_dev_quiet(&podman_run_argv(&spec)).map(|c| c != 0).unwrap_or(true) {
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
        "gatekeeperd/bench_plane: egress up bench={} profiles=[{}] ns={} cip={} dsts={}",
        rec.id, profiles.join(","), net.ns, net.cont_ip, resolved.endpoints.len()
    );

    // Egress is UP and the container alive: exec the workload (or a shell) into the networked container.
    let code = if interactive {
        // The shell's rc is not meaningful; success = the session ran (parity with the proven `enter`,
        // now with an explicit --workdir /work). `stop -t 2` stays here — the interactive path is proven,
        // and Fable ruled touching it is not required (the shared teardown SIGKILLs the holder either way).
        let _ = podman_as_dev(&podman_exec_argv(&ctr, true, &["/bin/sh".to_string()]));
        let _ = podman_as_dev(&["stop".into(), "-t".into(), "2".into(), ctr.clone()]);
        0
    } else {
        // `podman exec` propagates the WORKLOAD's exit status directly (vs the old `podman wait` on PID1):
        // 126/127/128+n pass through verbatim; a podman-INFRA failure (Err, or a signal-killed podman whose
        // .code() is None → -1 via podman_as_dev) maps to the -1 sentinel, never a fabricated 0/125.
        match podman_as_dev(&podman_exec_argv(&ctr, false, workload)) {
            Ok(c) => c,
            Err(_) => -1,
        }
    };

    // No `podman stop` on the non-interactive path (Fable item-2 fix 3): a `sleep infinity` PID1 IGNORES
    // SIGTERM from an ancestor pid-namespace (the kernel discards default-disposition signals to a pidns
    // init), so `stop -t N` would stall the full N seconds then SIGKILL anyway. Tearing the veth down FIRST
    // severs egress instantly for any straggler the workload backgrounded (they reparent to the holder),
    // then `podman rm -f` SIGKILLs the holder. teardown+rm run unconditionally, regardless of `code`.
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
    // Reject any grant whose anchor-relative path has a DOT-LEADING component (kills ~/.local, ~/.config,
    // ~/.ssh, …). Granting e.g. ~/.local/share/applications would let a bench workload plant an
    // UNCONSTRAINED .desktop (arbitrary Exec), collapsing the constrained-export story — and dotdirs hold
    // secrets/config a Bench must start with none of (rule 4). `fs_leaf` already rejects a dot-leading
    // BASENAME; this extends it to EVERY component of the resolved path (ADR-003 step-7 must-fix 5).
    if let Ok(rel) = canonical.strip_prefix(&anchor) {
        if rel.components().any(|c| matches!(c, std::path::Component::Normal(s) if s.to_string_lossy().starts_with('.'))) {
            eprintln!("bench grant: {} has a dot-leading path component — refused (no ~/.config, ~/.local, ~/.ssh, …: a workload could plant an unconstrained .desktop or read secrets)", canonical.display());
            return 2;
        }
    }
    if !std::fs::metadata(&canonical).map(|m| m.is_dir()).unwrap_or(false) {
        eprintln!("bench grant: {} is not a directory (only directory grants are supported)", canonical.display());
        return 2;
    }
    let Some(leaf) = Grant::fs_leaf(&canonical) else {
        eprintln!("bench grant: {} has an unsafe basename (need alnum/._- , no leading dot)", canonical.display());
        return 2;
    };
    // Ensure the per-bench /run grant dir exists root:dev 0710 (mycelium #2982 hole 3) BEFORE relocate
    // creates the leaf under it — so the grants dir dev cannot write is the parent of every bind target.
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

/// network <name> <profile...>: SET a Bench's sealed egress to EXACTLY the named set (rule 3). DECLARATIVE
/// and REPEATABLE — this REPLACES whatever set was there, so `debian-apt pypi-https` composes apt + pip on
/// one bench and the consent screen always shows the complete resulting reachability (Fable item-1). Each
/// name is validated against the compiled-in `shrek_policy::egress` table (default-deny: any unknown name
/// refuses the WHOLE call, changing nothing). `none` alone REVOKES all egress. The set is recorded and its
/// UNION injected per container start (a rootless netns dies on every stop). No live container is touched.
fn network(name: &str, profiles: &[String]) -> i32 {
    let rdir = bench_record::records_dir();
    let Some(mut rec) = bench_record::load_record(&rdir, name) else {
        eprintln!("bench: no such bench {name}");
        return 4;
    };
    let set = match validate_profile_set(profiles) {
        Ok(s) => s,
        Err(msg) => {
            eprintln!("bench network: {msg}");
            return 2;
        }
    };
    // Declarative replace: drop every existing net grant, then add the validated set (empty set = revoke).
    rec.grants.retain(|g| !matches!(Grant::parse(g), Some(Grant::Net { .. })));
    for p in &set {
        rec.grants.push(Grant::Net { profile: p.clone() }.encode());
    }
    if let Err(e) = bench_record::write_record(&rdir, &rec) {
        eprintln!("bench network: record write failed: {e}");
        return 1;
    }
    if set.is_empty() {
        println!("bench: {name} egress revoked (runs offline)");
    } else {
        println!("bench: {name} egress policy set to [{}] (injected per run)", set.join(", "));
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
    // Sweep this Bench's exported launcher .desktop files (step 7) — as dev, from the record (fix 2/4).
    for e in &rec.exports {
        if let Some(x) = Export::parse(e) {
            remove_desktop_as_dev(&x.file);
        }
    }
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

/// export <name> <key> [--label L] [--icon I] -- <workload...>: register a launcher app for a Bench (step
/// 7). Records the key→workload map in the ROOT-OWNED record (untamperable by the .desktop author) and
/// writes a CONSTRAINED .desktop (Exec=`/usr/bin/shrek-bench-run <name> <key>`, no command) AS DEV. The
/// fixed-baked-key mirror of shrek-menu: the .desktop carries only a key; the command lives in trusted state.
fn export(name: &str, key: &str, label_opt: Option<&str>, icon_opt: Option<&str>, workload: &[String]) -> i32 {
    let rdir = bench_record::records_dir();
    let Some(mut rec) = bench_record::load_record(&rdir, name) else {
        eprintln!("bench: no such bench {name}");
        return 4;
    };
    if !bench_record::valid_bench_name(key) {
        eprintln!("bench export: key {key:?} must be a safe token (alnum ._- , no leading dot, <=64)");
        return 2;
    }
    if workload.is_empty() {
        eprintln!("bench export: an empty workload is not allowed (give the command after --)");
        return 2;
    }
    if workload.iter().any(|a| a.contains('\n')) {
        eprintln!("bench export: a workload arg contains a newline");
        return 2;
    }
    if rec.exports.iter().filter_map(|e| Export::parse(e)).any(|e| e.key == key) {
        eprintln!("bench export: key {key:?} is already exported from {name} (unexport it first)");
        return 1;
    }
    let label = sanitize_label(label_opt).unwrap_or_else(|| format!("{name}: {key}"));
    let icon = icon_opt.filter(|i| valid_icon(i)).unwrap_or("application-x-executable").to_string();
    let file = format!("shrek-bench-{name}-{key}.desktop");
    if !valid_desktop_file(&file) {
        eprintln!("bench export: generated .desktop name {file:?} is unsafe");
        return 2;
    }
    // Write the .desktop AS DEV first — this catches a cross-bench filename collision (AlreadyExists)
    // BEFORE we mutate the record, so a refused export leaves no orphan record line.
    if let Err(e) = write_desktop_as_dev(&file, &desktop_content(name, key, &label, &icon)) {
        eprintln!("bench export: .desktop write failed: {e}");
        return 1;
    }
    let exp = Export { key: key.to_string(), file: file.clone(), icon, label, cmd: workload.to_vec() };
    rec.exports.push(exp.encode());
    if let Err(e) = bench_record::write_record(&rdir, &rec) {
        eprintln!("bench export: record write failed: {e}");
        remove_desktop_as_dev(&file); // roll back the orphan .desktop
        return 1;
    }
    println!("bench: exported {name}:{key} -> [{}] (launcher app {file})", workload.join(" "));
    0
}

/// run-export <name> <key>: the wrapper target. Resolve the key against the ROOT-OWNED record (server-side,
/// so a forged .desktop can pass only a key, never a command) and run that workload in the Bench via the
/// normal `run` path (FS/egress grants apply). An unregistered key is REFUSED — the whole trust anchor.
fn run_export(name: &str, key: &str) -> i32 {
    if !bench_record::valid_bench_name(key) {
        eprintln!("bench run-export: invalid key {key:?}");
        return 2;
    }
    let rdir = bench_record::records_dir();
    let Some(rec) = bench_record::load_record(&rdir, name) else {
        eprintln!("bench: no such bench {name}");
        return 4;
    };
    let Some(exp) = rec.exports.iter().filter_map(|e| Export::parse(e)).find(|e| e.key == key) else {
        eprintln!("bench run-export: {key:?} is not a registered export of {name} — refusing (the launcher key is unknown)");
        return 4;
    };
    run(name, false, &exp.cmd)
}

/// unexport <name> <key>: drop the export from the record and remove its .desktop (as dev). Idempotent-ish:
/// a missing key is an error (rc 4), a missing .desktop is fine.
fn unexport(name: &str, key: &str) -> i32 {
    let rdir = bench_record::records_dir();
    let Some(mut rec) = bench_record::load_record(&rdir, name) else {
        eprintln!("bench: no such bench {name}");
        return 4;
    };
    let before = rec.exports.len();
    let mut removed_file = None;
    rec.exports.retain(|e| match Export::parse(e) {
        Some(x) if x.key == key => {
            removed_file = Some(x.file);
            false
        }
        _ => true,
    });
    if rec.exports.len() == before {
        eprintln!("bench unexport: no export {key:?} in {name}");
        return 4;
    }
    if let Some(f) = &removed_file {
        remove_desktop_as_dev(f);
    }
    if let Err(e) = bench_record::write_record(&rdir, &rec) {
        eprintln!("bench unexport: record write failed: {e}");
        return 1;
    }
    println!("bench: unexported {name}:{key}");
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

/// workshop-list: print every Workshop recipe and its seed / declared-package counts / source bench.
/// Read-only — reads the root-owned world-readable recipes directly (no authority, no ceremony).
fn workshop_list() -> i32 {
    let recs = workshop_record::list_records(&workshop_record::workshops_dir());
    if recs.is_empty() {
        println!("workshop: no recipes");
        return 0;
    }
    println!("NAME                 SEED       APT  PIP  FROM");
    for r in recs {
        println!("{:<20} {:<10} {:<4} {:<4} {}", r.name, r.seed, r.apt.len(), r.pip.len(), r.source);
    }
    0
}

/// workshop-show <name>: print one recipe in full — the base seed, provenance, declared package sets, the
/// declared-maximum grant requests, and the carried launchers. Read-only. `rc 4` if the recipe is absent.
fn workshop_show(name: &str) -> i32 {
    let Some(r) = workshop_record::load_record(&workshop_record::workshops_dir(), name) else {
        eprintln!("workshop: no such recipe {name}");
        return 4;
    };
    println!("workshop {}", r.name);
    println!("  seed     {}", r.seed);
    println!("  source   {} (bench)", r.source);
    println!("  created  {}", r.created);
    if !r.apt.is_empty() {
        println!("  apt      {}", r.apt.join(" "));
    }
    if !r.pip.is_empty() {
        println!("  pip      {}", r.pip.join(" "));
    }
    for g in &r.grants {
        println!("  declares grant {g}");
    }
    for e in &r.exports {
        if let Some(x) = Export::parse(e) {
            println!("  launcher {} -> {}", x.key, x.cmd.join(" "));
        }
    }
    // Tool Shed status (Commit 3): compute the SAME content-address key a launch would, and surface the
    // cached derivation's resolved versions if a live entry exists. Read-only — unlike the launch's
    // `cache_lookup`, a stale (Id-mismatched) entry is REPORTED here, never evicted.
    if !r.apt.is_empty() || !r.pip.is_empty() {
        let net = egress_profiles_from(&r.grants);
        let key = seed_image_id(&r.seed)
            .and_then(|sid| sha256_hex_stdin(&workshop_cache::canonical_key_input(&sid, &r.apt, &r.pip, &net)));
        match key.as_deref().and_then(|k| workshop_cache::load_record(&workshop_cache::cache_dir(), k).map(|rec| (k, rec))) {
            Some((k, cr)) if podman_image_id(&cr.image).as_deref() == Some(cr.image_id.as_str()) => {
                println!("  cached   yes (key {} image {})", &k[..12], cr.image);
                for l in &cr.apt_resolved {
                    println!("    apt-resolved {l}");
                }
                for l in &cr.pip_resolved {
                    println!("    pip-resolved {l}");
                }
            }
            Some(_) => println!("  cached   stale (image changed since derive; next launch re-derives)"),
            None => println!("  cached   no (next launch derives + caches)"),
        }
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

// ---- authority-increasing verbs: consent-ceremony precheck + commit (ADR-003 Part 2 step 3) -----
//
// The three authority-INCREASING verbs (grant / network-to-a-profile / export) are gated behind the
// console consent ceremony (docs/bench-authz-consent-slice.md) when driven by a NON-root socket peer.
// The ceremony (crate::consent) owns the human OK; THIS owns the FS/policy logic, split into:
//   * precheck_authority — runs EVERY validator and resolves the request to an AuthorityPlan BEFORE any
//     human is asked (invariant 1: a request that fails validation denies, the human never prompted);
//   * commit_authority   — applies the plan, RE-VERIFYING the target's object identity at apply time
//     (invariant 5: the swap defense — an approved dir cannot be swapped for a symlink/rename between
//     the OK and the apply). The root in-process `cli()` path keeps using grant()/network()/export()
//     directly (boot / proofs / reissue run as root, no ceremony); this is the second, gated path.

/// A validated, server-resolved plan for one authority-increasing request. `diff_rows` are RAW
/// (possibly untrusted) label/value pairs — the consent renderer SANITIZES them at the output boundary;
/// nothing here trusts them for control flow.
pub(crate) struct AuthorityPlan {
    pub bench: String,
    pub diff_rows: Vec<(String, String)>,
    pub trifecta: bool,
    kind: CommitKind,
}

impl AuthorityPlan {
    /// Higher-authority verbs demand a typed confirmation code (not a bare `y`): a read-WRITE grant, an
    /// export (which mints a durable, ceremony-free launcher), or a promote (which mints a durable,
    /// reusable authority TEMPLATE). A read-only grant / egress attach take `y`.
    pub(crate) fn high_authority(&self) -> bool {
        matches!(
            &self.kind,
            CommitKind::Export { .. }
                | CommitKind::Grant { rw: true, .. }
                | CommitKind::Promote { .. }
                | CommitKind::Launch { .. }
                | CommitKind::DesktopEgress { .. }
        )
    }

    /// Human-facing one-line action summary for the ceremony header (safe: verb + validated bench name).
    pub(crate) fn action(&self) -> String {
        match &self.kind {
            CommitKind::Grant { rw, .. } => format!("GRANT a host folder ({}) to bench '{}'", if *rw { "read-write" } else { "read-only" }, self.bench),
            CommitKind::Network { .. } => format!("SET the network egress on bench '{}'", self.bench),
            CommitKind::Export { .. } => format!("EXPORT a desktop launcher for bench '{}'", self.bench),
            CommitKind::Promote { workshop, .. } => format!("PROMOTE bench '{}' to the Workshop recipe '{}'", self.bench, workshop),
            CommitKind::Launch { .. } => format!("LAUNCH the Workshop recipe '{}' (re-derive its environment + run)", self.bench),
            CommitKind::DesktopEgress(op) => crate::desktop_egress::action(op),
        }
    }
}

/// Build a validated desktop-egress ceremony plan (ADR-007 S4). Lives here because `CommitKind` is
/// private to this module; the desktop-egress module supplies the op + rendered diff and this wraps it in
/// the shared `AuthorityPlan` the consent renderer consumes. `subject` is the wire/audit label (the
/// profile name or the raw triple), NOT a bench.
pub(crate) fn desktop_egress_plan(
    op: crate::desktop_egress::Op,
    subject: String,
    diff_rows: Vec<(String, String)>,
    trifecta: bool,
) -> AuthorityPlan {
    AuthorityPlan { bench: subject, diff_rows, trifecta, kind: CommitKind::DesktopEgress(op) }
}

enum CommitKind {
    Grant { canonical: PathBuf, leaf: String, rw: bool, ident: Ident },
    Network { profiles: Vec<String> },
    /// ADR-007 S4 — a console-ceremony desktop-egress change (`web-browsing` bless/unbless or a raw
    /// `host:proto:port` add/remove). Commit execs the root-only `egressd confirmed-*` verb.
    DesktopEgress(crate::desktop_egress::Op),
    Export { key: String, file: String, icon: String, label: String, cmd: Vec<String> },
    /// A validated bench→Workshop recipe. Carries the CONSENTED snapshot (declared package sets + the
    /// grants/exports copied from the source bench at precheck time) so `commit_promote` writes exactly
    /// what the human saw — not a fresh re-read that could have drifted since the OK. `self.bench` is the
    /// source bench (build provenance).
    Promote { workshop: String, seed: String, apt: Vec<String>, pip: Vec<String>, grants: Vec<String>, exports: Vec<String> },
    /// A validated Workshop `launch`: the CONSENTED recipe snapshot (seed + declared package sets + the
    /// declared-max grant lines + the workload to run) plus the two launch modifiers. `self.bench` is the
    /// workshop name. Commit resolves the environment via the Tool Shed cache (HIT reuses a derived image;
    /// MISS derives+stores one), runs the workload in a fresh bench from it, then tears the bench down (the
    /// cached image persists; the recipe stays the sole authoritative state). `refresh` forces a re-derive;
    /// `offline` reduces the workload's runtime egress to none and refuses to derive (HIT-only).
    Launch { seed: String, apt: Vec<String>, pip: Vec<String>, grants: Vec<String>, workload: Vec<String>, refresh: bool, offline: bool },
}

#[cfg(test)]
impl AuthorityPlan {
    /// Test-only constructor for the consent-module orchestration/binding tests (which drive a mock
    /// console and must not touch a live record). Not compiled into any non-test build.
    pub(crate) fn test_plan(verb: &'static str, bench: &str, rw: bool, trifecta: bool) -> AuthorityPlan {
        let kind = match verb {
            "grant" => CommitKind::Grant {
                canonical: PathBuf::from("/home/dev/x"),
                leaf: "x".into(),
                rw,
                ident: Ident { dev_major: 0, dev_minor: 0, ino: 1 },
            },
            "network" => CommitKind::Network { profiles: vec!["p".into()] },
            "promote" => CommitKind::Promote {
                workshop: "ws".into(),
                seed: "debian".into(),
                apt: vec!["sl".into()],
                pip: vec![],
                grants: vec![],
                exports: vec![],
            },
            "launch" => CommitKind::Launch {
                seed: "debian".into(),
                apt: vec!["sl".into()],
                pip: vec![],
                grants: vec![],
                workload: vec!["true".into()],
                refresh: false,
                offline: false,
            },
            _ => CommitKind::Export {
                key: "k".into(),
                file: "shrek-bench-media-k.desktop".into(),
                icon: "i".into(),
                label: "l".into(),
                cmd: vec!["c".into()],
            },
        };
        AuthorityPlan {
            bench: bench.to_string(),
            diff_rows: vec![("Grant path".into(), "/home/dev/x".into())],
            trifecta,
            kind,
        }
    }
}

/// The lethal-trifecta predicate for bench semantics: warn when the bench, AFTER this change, composes
/// untrusted filesystem read WITH network egress (either direction). Pure — unit-tested.
pub(crate) fn trifecta_after(existing_fs: bool, existing_net: bool, adding_fs: bool, adding_net: bool) -> bool {
    (existing_fs || adding_fs) && (existing_net || adding_net)
}

fn record_has_fs(rec: &BenchRecord) -> bool {
    rec.grants.iter().any(|g| matches!(Grant::parse(g), Some(Grant::Fs { .. })))
}

/// Validate + resolve an authority-increasing request to an [`AuthorityPlan`]. `Err((rc, msg))` on ANY
/// validation failure — the human is never asked. `rest` is the decoded argv tail (no subverb).
pub(crate) fn precheck_authority(verb: &str, rest: &[String]) -> Result<AuthorityPlan, (i32, String)> {
    match verb {
        "grant" => precheck_grant(rest),
        "network" => precheck_network(rest),
        "export" => precheck_export(rest),
        "promote" => precheck_promote(rest),
        "launch" => precheck_launch(rest),
        other => Err((2, format!("not an authority verb: {other}"))),
    }
}

/// The sealed egress profiles a declared package set REQUIRES at launch: apt needs `debian-apt`, pip needs
/// `pypi-https`. A recipe that declares packages but whose copied egress set lacks the matching profile can
/// NEVER install them (launch re-consents ≤ the recipe's declared max — it cannot add egress), so promote
/// let it through but launch refuses it fail-closed with a clear error rather than a mid-install network hang.
const APT_EGRESS_PROFILE: &str = "debian-apt";
const PIP_EGRESS_PROFILE: &str = "pypi-https";

/// Validate + resolve a `workshop launch <ws> [-- workload...]` request to an [`AuthorityPlan`]. Launch is
/// authority-MATERIALIZING (it activates the recipe's declared-maximum grants/egress and runs a workload),
/// so it re-consents per launch (HIGH-authority composite ceremony) and re-validates the recipe against the
/// sealed catalog/policy fail-closed — a recipe on disk is data, never trusted for the seed/egress decision.
fn precheck_launch(rest: &[String]) -> Result<AuthorityPlan, (i32, String)> {
    let mut ws = None;
    let mut workload: Vec<String> = Vec::new();
    let mut refresh = false;
    let mut offline = false;
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--" => { workload = rest[i + 1..].to_vec(); break; }
            // `--refresh` forces a cache re-derive; `--offline` runs the workload with no egress and refuses
            // to derive (a HIT-only launch). Both are authority-NEUTRAL/REDUCING modifiers, not new authority.
            "--refresh" => refresh = true,
            "--offline" => offline = true,
            other if !other.starts_with('-') && ws.is_none() => ws = Some(other.to_string()),
            other => return Err((2, format!("unexpected arg {other}"))),
        }
        i += 1;
    }
    let Some(ws) = ws else {
        return Err((2, "usage: workshop launch <workshop> [-- workload...]".into()));
    };
    if workload.iter().any(|a| a.contains('\n')) {
        return Err((2, "a workload arg contains a newline".into()));
    }
    let Some(rec) = workshop_record::load_record(&workshop_record::workshops_dir(), &ws) else {
        return Err((4, format!("no such workshop {ws}")));
    };
    // Re-validate the seed against the sealed catalog (the recipe is data — never trust its string).
    if !valid_seed(&rec.seed) {
        return Err((2, format!("recipe {ws} names an unknown seed {:?} — refusing (sealed catalog, fail-closed)", rec.seed)));
    }
    // Re-validate EVERY declared package token (defensive — promote validated them, but a hand-edited or
    // corrupt recipe must never reach apt/pip with a flag-shaped token).
    for p in rec.apt.iter().chain(rec.pip.iter()) {
        if !valid_pkg_token(p) {
            return Err((2, format!("recipe {ws} holds an invalid package token {p:?} — refusing")));
        }
    }
    // Re-validate every egress profile in the recipe against sealed policy, fail-closed (a single unknown
    // name refuses the whole launch before any container starts) — the run path re-checks too, but catching
    // it here keeps the human from consenting to an un-launchable recipe.
    let net: Vec<String> = egress_profiles_from(&rec.grants);
    for p in &net {
        if shrek_policy::egress::resolve(p).is_none() {
            return Err((2, format!("recipe {ws} names egress profile {p:?} not in sealed policy — refusing (fail-closed)")));
        }
    }
    // Coherence (Fable must-fix 4): declared packages need their install egress in the recipe's set, or the
    // derivation can never fetch them.
    if !rec.apt.is_empty() && !net.iter().any(|p| p == APT_EGRESS_PROFILE) {
        return Err((2, format!("recipe {ws} declares apt packages but its egress set has no {APT_EGRESS_PROFILE:?} — un-launchable")));
    }
    if !rec.pip.is_empty() && !net.iter().any(|p| p == PIP_EGRESS_PROFILE) {
        return Err((2, format!("recipe {ws} declares pip packages but its egress set has no {PIP_EGRESS_PROFILE:?} — un-launchable")));
    }
    let has_fs = rec.grants.iter().any(|g| matches!(Grant::parse(g), Some(Grant::Fs { .. })));
    let trifecta = trifecta_after(has_fs, !net.is_empty(), false, false);
    // Diff rows: the COMPLETE activation the human consents to — the recipe's env + the workload to run in it.
    let mut rows = vec![
        ("Base seed".to_string(), rec.seed.clone()),
        ("apt packages".to_string(), if rec.apt.is_empty() { "(none)".into() } else { rec.apt.join(" ") }),
        ("pip packages".to_string(), if rec.pip.is_empty() { "(none)".into() } else { rec.pip.join(" ") }),
    ];
    for g in &rec.grants {
        rows.push(("Activates grant".to_string(), g.clone()));
    }
    rows.push(("Runs".to_string(), if workload.is_empty() { "(recipe default)".into() } else { workload.join(" ") }));
    if refresh {
        rows.push(("Cache".to_string(), "--refresh (force re-derive)".into()));
    }
    if offline {
        rows.push(("Cache".to_string(), "--offline (no egress; cached derivation only)".into()));
    }
    Ok(AuthorityPlan {
        bench: ws,
        diff_rows: rows,
        trifecta,
        kind: CommitKind::Launch { seed: rec.seed, apt: rec.apt, pip: rec.pip, grants: rec.grants, workload, refresh, offline },
    })
}

/// The egress profile SET encoded in a recipe/bench `grants` vec (the `net <profile>` lines), in order.
fn egress_profiles_from(grants: &[String]) -> Vec<String> {
    grants
        .iter()
        .filter_map(|g| match Grant::parse(g) {
            Some(Grant::Net { profile }) => Some(profile),
            _ => None,
        })
        .collect()
}

/// Validate + resolve a `promote <bench> --name <workshop> [--apt pkg]... [--pip pkg]...` request to an
/// [`AuthorityPlan`]. Promote reads the SOURCE bench's declared authority (its `fs`/`net` grants + exported
/// launchers) and, together with the human-declared package sets, resolves the reproducible recipe the
/// human is about to persist. It is authority-DECLARING (mints a durable, reusable template), so it takes
/// the HIGH-authority typed-code path and the full diff of the resulting recipe. `Err((rc, msg))` on ANY
/// validation failure — the human is never asked.
fn precheck_promote(rest: &[String]) -> Result<AuthorityPlan, (i32, String)> {
    let (mut bench, mut ws) = (None, None);
    let mut apt: Vec<String> = Vec::new();
    let mut pip: Vec<String> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--name" => { i += 1; ws = rest.get(i).cloned(); }
            "--apt" => { i += 1; match rest.get(i) { Some(p) => apt.push(p.clone()), None => return Err((2, "--apt needs a package".into())) } }
            "--pip" => { i += 1; match rest.get(i) { Some(p) => pip.push(p.clone()), None => return Err((2, "--pip needs a package".into())) } }
            other if !other.starts_with('-') && bench.is_none() => bench = Some(other.to_string()),
            other => return Err((2, format!("unexpected arg {other}"))),
        }
        i += 1;
    }
    let (bench, ws) = match (bench, ws) {
        (Some(b), Some(w)) => (b, w),
        _ => return Err((2, "usage: promote <bench> --name <workshop> [--apt pkg]... [--pip pkg]...".into())),
    };
    if !bench_record::valid_bench_name(&ws) {
        return Err((2, format!("workshop name {ws:?} must be a safe token (alnum ._- , no leading dot, <=64)")));
    }
    // Every declared package must satisfy the token grammar BEFORE any human is asked — a leading `-` or
    // whitespace token could otherwise reach `apt`/`pip` as a flag/second-arg at launch (must-fix 5).
    for p in apt.iter().chain(pip.iter()) {
        if !valid_pkg_token(p) {
            return Err((2, format!("package {p:?} is not a valid token (no leading '-', no whitespace; '=' / '==' pins ok)")));
        }
    }
    // Duplicate tokens in a set are REFUSED (a clear error, never a silent dedup) — mirrors the egress set.
    for (label, set) in [("apt", &apt), ("pip", &pip)] {
        for (j, p) in set.iter().enumerate() {
            if set[..j].iter().any(|q| q == p) {
                return Err((1, format!("duplicate {label} package {p:?}")));
            }
        }
    }
    let rdir = bench_record::records_dir();
    let Some(rec) = bench_record::load_record(&rdir, &bench) else {
        return Err((4, format!("no such bench {bench}")));
    };
    // The bench's seed must still be a sealed catalog name (create validated it; re-check fail-closed so a
    // recipe never records a seed the catalog cannot re-derive from).
    if !valid_seed(&rec.seed) {
        return Err((2, format!("bench {bench} runs an unknown seed {:?} — cannot promote (sealed catalog, fail-closed)", rec.seed)));
    }
    // Copy the DECLARED-MAXIMUM authority (fs + net grants) and the exported launchers FROM the bench —
    // the recipe REQUESTS this maximum (ADR-002 §5); launch (commit 2) re-consents ≤ it per run.
    let grants = rec.grants.clone();
    let exports = rec.exports.clone();
    // Trifecta: on launch this recipe will compose the bench's untrusted FS read WITH egress if the bench
    // already has both — surface it in the ceremony exactly as grant/network do (nothing added here).
    let trifecta = trifecta_after(record_has_fs(&rec), !egress_profiles(&rec).is_empty(), false, false);
    // Diff rows: the COMPLETE recipe the human consents to persist as a reusable authority template.
    let mut rows = vec![
        ("From bench".to_string(), bench.clone()),
        ("Base seed".to_string(), rec.seed.clone()),
        ("apt packages".to_string(), if apt.is_empty() { "(none)".into() } else { apt.join(" ") }),
        ("pip packages".to_string(), if pip.is_empty() { "(none)".into() } else { pip.join(" ") }),
    ];
    for g in &grants {
        rows.push(("Declares grant".to_string(), g.clone()));
    }
    for e in &exports {
        if let Some(x) = Export::parse(e) {
            rows.push(("Carries launcher".to_string(), format!("{} -> {}", x.key, x.cmd.join(" "))));
        }
    }
    // A re-promote atomically REPLACES an existing recipe of the same name — surface it (transparency; the
    // overwrite is not itself a gate, but the human should see they are redefining an existing recipe).
    if workshop_record::load_record(&workshop_record::workshops_dir(), &ws).is_some() {
        rows.push(("Replaces recipe".to_string(), ws.clone()));
    }
    Ok(AuthorityPlan {
        bench,
        diff_rows: rows,
        trifecta,
        kind: CommitKind::Promote { workshop: ws, seed: rec.seed, apt, pip, grants, exports },
    })
}

fn precheck_grant(rest: &[String]) -> Result<AuthorityPlan, (i32, String)> {
    let (mut name, mut path, mut rw) = (None, None, false);
    for a in rest {
        match a.as_str() {
            "--rw" => rw = true,
            "--ro" => rw = false,
            other if !other.starts_with('-') && name.is_none() => name = Some(other.to_string()),
            other if !other.starts_with('-') && path.is_none() => path = Some(other.to_string()),
            other => return Err((2, format!("unexpected arg {other}"))),
        }
    }
    let (name, path_str) = match (name, path) {
        (Some(n), Some(p)) => (n, p),
        _ => return Err((2, "usage: grant <name> <path> [--rw|--ro]".into())),
    };
    let rdir = bench_record::records_dir();
    let Some(rec) = bench_record::load_record(&rdir, &name) else {
        return Err((4, format!("no such bench {name}")));
    };
    let canonical = std::fs::canonicalize(&path_str).map_err(|e| (2, format!("cannot resolve {path_str:?}: {e}")))?;
    let anchor = anchor_dir();
    if canonical == anchor || !canonical.starts_with(&anchor) {
        return Err((2, format!("{} must be a directory strictly beneath {}", canonical.display(), anchor.display())));
    }
    if let Ok(rel) = canonical.strip_prefix(&anchor) {
        if rel.components().any(|c| matches!(c, std::path::Component::Normal(s) if s.to_string_lossy().starts_with('.'))) {
            return Err((2, format!("{} has a dot-leading path component — refused (no ~/.config, ~/.ssh, …)", canonical.display())));
        }
    }
    if !std::fs::metadata(&canonical).map(|m| m.is_dir()).unwrap_or(false) {
        return Err((2, format!("{} is not a directory", canonical.display())));
    }
    let Some(leaf) = Grant::fs_leaf(&canonical) else {
        return Err((2, format!("{} has an unsafe basename", canonical.display())));
    };
    for g in &rec.grants {
        if let Some(Grant::Fs { path, .. }) = Grant::parse(g) {
            if path == canonical {
                return Err((1, format!("{} is already granted to {name}", canonical.display())));
            }
            if Grant::fs_leaf(&path).as_deref() == Some(leaf.as_str()) {
                return Err((1, format!("mount leaf {leaf:?} already used by another grant")));
            }
        }
    }
    // Pin the target beneath the anchor NOW to capture its object identity (TOCTOU-safe openat2 walk);
    // commit re-pins and requires the SAME identity — the swap defense.
    let rel = canonical.strip_prefix(&anchor).map_err(|_| (2, "grant not beneath anchor".to_string()))?;
    let rel_str = rel.to_str().ok_or_else(|| (2, "non-utf8 grant path".to_string()))?;
    let anchor_fd = open_anchor(&anchor).map_err(|e| (1, format!("anchor open failed: {e}")))?;
    let pinned = pin_beneath(&anchor_fd, rel_str).map_err(|e| (2, format!("pin failed: {e}")))?;
    if !pinned.is_dir {
        return Err((2, "grant target must be a directory".to_string()));
    }
    let ident = pinned.ident;
    let trifecta = trifecta_after(record_has_fs(&rec), !egress_profiles(&rec).is_empty(), true, false);
    let rows = vec![
        ("Grant path".to_string(), canonical.display().to_string()),
        ("Access".to_string(), if rw { "READ-WRITE".into() } else { "read-only".into() }),
        ("Mounts in bench at".to_string(), grant_mountpoint(&leaf)),
    ];
    Ok(AuthorityPlan { bench: name, diff_rows: rows, trifecta, kind: CommitKind::Grant { canonical, leaf, rw, ident } })
}

fn precheck_network(rest: &[String]) -> Result<AuthorityPlan, (i32, String)> {
    let Some((name, profiles)) = rest.split_first() else {
        return Err((2, "usage: network <name> <profile...>".into()));
    };
    // `none` (revoke) is reducing — it is routed ceremony-free by the front ends and must never reach the
    // consent path; if it does (e.g. mixed into a set), refuse before prompting any human.
    if profiles.iter().any(|p| p == "none") {
        return Err((2, "network none REVOKES egress (reducing) — ceremony-free, not a consent request; it cannot be mixed with a profile".into()));
    }
    // Validate the WHOLE declarative set up front (dup/unknown/flag all fail here, before any VT is shown —
    // consent invariant 1). `none` is already excluded, so the returned set is non-empty.
    let set = validate_profile_set(profiles).map_err(|m| (2, m))?;
    let rdir = bench_record::records_dir();
    let Some(rec) = bench_record::load_record(&rdir, name) else {
        return Err((4, format!("no such bench {name}")));
    };
    // Resolve each name (validated Some, but re-resolve fail-closed rather than unwrap in a daemon).
    let mut refs = Vec::with_capacity(set.len());
    for p in &set {
        match shrek_policy::egress::resolve(p) {
            Some(r) => refs.push(r),
            None => return Err((2, format!("{p:?} is not a sealed egress profile (policy is default-deny)"))),
        }
    }
    // One row PER profile (never a joined string — the consent renderer sanitizes per value, and a name in
    // the middle of a long joined line could be visually lost). The screen always shows the COMPLETE
    // resulting set — the property that keeps declarative-replace consent-safe (Fable item-1 fix 4).
    let mut rows = network_profile_rows(&set);
    if let Ok(r) = net_plane::resolve_profiles_v4(&refs) {
        rows.push(("Allowed endpoints".to_string(), r.endpoints.len().to_string()));
    }
    // Replace semantics: show the set being displaced when the bench already has egress (clarity — the
    // human sees this is a REPLACE, not an add). Removal is reducing, so this is transparency, not a gate.
    let prior = egress_profiles(&rec);
    if !prior.is_empty() {
        rows.push(("Replaces egress".to_string(), prior.join(", ")));
    }
    let trifecta = trifecta_after(record_has_fs(&rec), false, false, true);
    Ok(AuthorityPlan { bench: name.clone(), diff_rows: rows, trifecta, kind: CommitKind::Network { profiles: set } })
}

fn precheck_export(rest: &[String]) -> Result<AuthorityPlan, (i32, String)> {
    let (mut name, mut key, mut label, mut icon) = (None, None, None, None);
    let mut workload: Vec<String> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--" => { workload = rest[i + 1..].to_vec(); break; }
            "--label" => { i += 1; label = rest.get(i).cloned(); }
            "--icon" => { i += 1; icon = rest.get(i).cloned(); }
            other if !other.starts_with('-') && name.is_none() => name = Some(other.to_string()),
            other if !other.starts_with('-') && key.is_none() => key = Some(other.to_string()),
            other => return Err((2, format!("unexpected arg {other}"))),
        }
        i += 1;
    }
    let (name, key) = match (name, key) {
        (Some(n), Some(k)) => (n, k),
        _ => return Err((2, "usage: export <name> <key> [--label L] [--icon I] -- <cmd...>".into())),
    };
    let rdir = bench_record::records_dir();
    let Some(rec) = bench_record::load_record(&rdir, &name) else {
        return Err((4, format!("no such bench {name}")));
    };
    if !bench_record::valid_bench_name(&key) {
        return Err((2, format!("key {key:?} must be a safe token (alnum ._- , no leading dot, <=64)")));
    }
    if workload.is_empty() {
        return Err((2, "an empty workload is not allowed (give the command after --)".into()));
    }
    if workload.iter().any(|a| a.contains('\n')) {
        return Err((2, "a workload arg contains a newline".into()));
    }
    if rec.exports.iter().filter_map(|e| Export::parse(e)).any(|e| e.key == key) {
        return Err((1, format!("key {key:?} is already exported from {name} (unexport it first)")));
    }
    let label = sanitize_label(label.as_deref()).unwrap_or_else(|| format!("{name}: {key}"));
    let icon = icon.filter(|i| valid_icon(i)).unwrap_or_else(|| "application-x-executable".to_string());
    let file = format!("shrek-bench-{name}-{key}.desktop");
    if !valid_desktop_file(&file) {
        return Err((2, format!("generated .desktop name {file:?} is unsafe")));
    }
    let trifecta = trifecta_after(record_has_fs(&rec), !egress_profiles(&rec).is_empty(), false, false);
    let rows = vec![
        ("Launcher key".to_string(), key.clone()),
        ("Runs command".to_string(), workload.join(" ")),
        ("Shown as".to_string(), label.clone()),
    ];
    Ok(AuthorityPlan { bench: name, diff_rows: rows, trifecta, kind: CommitKind::Export { key, file, icon, label, cmd: workload } })
}

/// Apply a pre-checked plan (called ONLY after the ceremony approves). Re-verifies target identity for
/// the grant path (the swap defense) and re-checks policy/collisions defensively. Returns the verb rc.
pub(crate) fn commit_authority(plan: &AuthorityPlan) -> i32 {
    match &plan.kind {
        CommitKind::Grant { canonical, leaf, rw, ident } => commit_grant(&plan.bench, canonical, leaf, *rw, *ident),
        CommitKind::Network { profiles } => commit_network(&plan.bench, profiles),
        CommitKind::Export { key, file, icon, label, cmd } => commit_export(&plan.bench, key, file, icon, label, cmd),
        CommitKind::Promote { workshop, seed, apt, pip, grants, exports } => commit_promote(&plan.bench, workshop, seed, apt, pip, grants, exports),
        CommitKind::Launch { seed, apt, pip, grants, workload, refresh, offline } => commit_launch(&plan.bench, seed, apt, pip, grants, workload, *refresh, *offline),
        CommitKind::DesktopEgress(op) => crate::desktop_egress::commit(op),
    }
}

/// Apply a pre-checked promote (called ONLY after the ceremony approves, or inline by root's `cli()`).
/// Writes the consented recipe snapshot to the root-owned workshops dir (atomic temp+rename; a re-promote
/// REPLACES). Re-validates the seed against the sealed catalog at apply time (defensive — commit never
/// trusts the plan's string for the catalog decision, mirroring `commit_network`). `source` = the bench.
#[allow(clippy::too_many_arguments)]
fn commit_promote(source: &str, workshop: &str, seed: &str, apt: &[String], pip: &[String], grants: &[String], exports: &[String]) -> i32 {
    if !valid_seed(seed) {
        eprintln!("bench promote: {seed:?} is not a sealed seed (re-check) — refused");
        return 2;
    }
    let rec = WorkshopRecord {
        name: workshop.to_string(),
        seed: seed.to_string(),
        created: now_secs(),
        source: source.to_string(),
        apt: apt.to_vec(),
        pip: pip.to_vec(),
        grants: grants.to_vec(),
        exports: exports.to_vec(),
    };
    if let Err(e) = workshop_record::write_record(&workshop_record::workshops_dir(), &rec) {
        eprintln!("bench promote: recipe write failed: {e}");
        return 1;
    }
    println!("bench: promoted {source} -> Workshop recipe '{workshop}' (seed={seed} apt={} pip={})", apt.len(), pip.len());
    0
}

/// The `podman run` argv for a PRISTINE derivation container (Commit-2 launch, Phase A). Deliberately NOT
/// [`podman_run_argv`]: a derivation container has NO `/work` bind and NO FS grants and NO exports — only
/// the seed rootfs + a bench-owned `/etc/hosts` for egress. It runs a `sleep infinity` holder as PID1 so
/// egress can be injected BEFORE the install execs (item-2 egress-before-install), and so that the bytes it
/// commits have never seen a user grant, `/work`, or a workload secret (the no-secrets invariant that makes
/// a derived image safe to reuse).
fn derive_run_argv(name: &str, image: &str, hosts: &Path) -> Vec<String> {
    vec![
        "run".into(), "-d".into(),
        "--name".into(), format!("shrek-bench-{name}"),
        "--network=none".into(), "--no-hosts".into(),
        "--runtime".into(), "crun".into(),
        "-v".into(), format!("{}:/etc/hosts:ro", hosts.display()),
        image.to_string(),
        "sleep".into(), "infinity".into(),
    ]
}

/// A `podman exec` argv for the derivation install phase: the container name is the FIRST positional, so
/// every token after it (incl. the install command's own `-y`/`--no-install-recommends` and the charset-
/// validated package names) is the command, never an `exec` flag. No `--workdir` (a pristine container has
/// no `/work`); the install runs in the image's default cwd.
fn derive_exec_argv(ctr: &str, cmd: &[String]) -> Vec<String> {
    let mut a: Vec<String> = vec!["exec".into(), ctr.to_string()];
    a.extend(cmd.iter().cloned());
    a
}

/// Phase A of `workshop launch`: build a PRISTINE derivation container from the recipe's sealed seed, bring
/// the recipe's install egress up (holder+exec), install the declared apt/pip packages as ARGV vectors,
/// scrub, `podman commit` to `derived_tag`, and tear the container down. On success returns the RESOLVED
/// versions the derivation actually installed (`dpkg-query` / `pip freeze` for the declared packages) — the
/// caller records them as cache provenance so `workshop show` surfaces exactly what a cached (offline)
/// launch reuses. NOTHING here touches a user grant, `/work`, an export, or a workload — so the committed
/// bytes are software-only by construction (the no-secrets invariant #2994). The IMAGE at `derived_tag`
/// persists past this call: Commit 2's caller deletes it (transient); Commit 3 keeps it under the cache tag
/// and writes an index record instead.
fn derive_pristine(
    workshop: &str,
    seed: &str,
    apt: &[String],
    pip: &[String],
    net_profiles: &[String],
    derived_tag: &str,
) -> Result<(Vec<String>, Vec<String>), ()> {
    ensure_seed(seed);
    let dname = format!("derive-{workshop}");
    let dctr = format!("shrek-bench-{dname}");

    // Resolve the recipe's egress against sealed policy (a single unknown name fails the whole derivation).
    let mut refs = Vec::with_capacity(net_profiles.len());
    for p in net_profiles {
        match shrek_policy::egress::resolve(p) {
            Some(r) => refs.push(r),
            None => {
                eprintln!("workshop launch: derivation egress {p:?} not in sealed policy — refusing (fail-closed)");
                return Err(());
            }
        }
    }
    let resolved = match net_plane::resolve_profiles_v4(&refs) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("workshop launch: derivation egress resolve failed (fail-closed, no network): {e}");
            return Err(());
        }
    };
    let hosts_path = match write_hosts(&dname, &resolved) {
        Ok(p) => p,
        Err(e) => { eprintln!("workshop launch: derivation hosts write failed: {e}"); return Err(()); }
    };

    let net = net_plane::SandboxNet::for_id(&format!("bench-{dname}"));
    net.teardown();
    let cleanup = |n: &net_plane::SandboxNet| {
        n.teardown();
        podman_rm(&dname);
        let _ = std::fs::remove_dir_all(bench_run_dir(&dname));
    };
    let fail = |n: &net_plane::SandboxNet, msg: String| -> Result<(Vec<String>, Vec<String>), ()> {
        eprintln!("{msg}");
        cleanup(n);
        Err(())
    };

    // Start the pristine holder DETACHED (quiet: `run -d` echoes the container id).
    if podman_as_dev_quiet(&derive_run_argv(&dname, &seed_image(seed), &hosts_path)).map(|c| c != 0).unwrap_or(true) {
        return fail(&net, "workshop launch: pristine derivation container start failed".into());
    }
    let Some(leader) = podman_pid(&dname) else {
        return fail(&net, "workshop launch: could not discover the derivation netns leader".into());
    };
    if !leader_in_distinct_netns(leader) {
        return fail(&net, format!("workshop launch: derivation leader {leader} not in a distinct netns — refusing inject"));
    }
    if let Err(e) = net.inject(leader, &resolved.endpoints, &resolved.no_masquerade_ips()) {
        return fail(&net, format!("workshop launch: derivation egress inject failed (fail-closed): {e}"));
    }
    if !attached_ns_matches_leader(&net.ns, leader) {
        return fail(&net, "workshop launch: derivation netns identity drift after attach — tearing down".into());
    }
    eprintln!("gatekeeperd/bench_plane: derivation egress up ws={workshop} ns={} dsts={}", net.ns, resolved.endpoints.len());

    // INSTALL PHASE (egress is up): apt then pip, each as an ARGV vector via `podman exec` — never a shell
    // string, so a package token can only ever be an argv element, not a flag or a `;`-chained command.
    if !apt.is_empty() {
        if podman_as_dev(&derive_exec_argv(&dctr, &["apt-get".into(), "update".into()])).unwrap_or(-1) != 0 {
            return fail(&net, "workshop launch: apt-get update failed in the derivation".into());
        }
        let mut install = vec!["apt-get".into(), "install".into(), "-y".into(), "--no-install-recommends".into()];
        install.extend(apt.iter().cloned());
        if podman_as_dev(&derive_exec_argv(&dctr, &install)).unwrap_or(-1) != 0 {
            return fail(&net, "workshop launch: apt install failed in the derivation".into());
        }
        let _ = podman_as_dev(&derive_exec_argv(&dctr, &["apt-get".into(), "clean".into()])); // scrub the cache
    }
    if !pip.is_empty() {
        // System-wide install (PEP 668 needs --break-system-packages) so the packages bake into the image
        // layer; --no-cache-dir keeps no wheel cache in the committed bytes.
        let mut install = vec![
            "python3".into(), "-m".into(), "pip".into(), "install".into(),
            "--no-cache-dir".into(), "--break-system-packages".into(),
        ];
        install.extend(pip.iter().cloned());
        if podman_as_dev(&derive_exec_argv(&dctr, &install)).unwrap_or(-1) != 0 {
            return fail(&net, "workshop launch: pip install failed in the derivation".into());
        }
    }

    // Capture the RESOLVED versions the install actually landed (cache provenance) while the container is
    // still up — best-effort: a capture failure leaves the field empty, it never fails the derivation.
    let apt_resolved: Vec<String> = if apt.is_empty() {
        Vec::new()
    } else {
        let mut q = vec!["dpkg-query".into(), "-W".into(), "-f".into(), "${Package}=${Version}\\n".into()];
        q.extend(apt.iter().map(|t| apt_base_name(t)));
        podman_as_dev_stdout(&derive_exec_argv(&dctr, &q))
            .map(|s| s.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
            .unwrap_or_default()
    };
    let pip_resolved: Vec<String> = if pip.is_empty() {
        Vec::new()
    } else {
        let bases: std::collections::HashSet<String> = pip.iter().map(|t| pip_norm_name(t)).collect();
        podman_as_dev_stdout(&derive_exec_argv(&dctr, &["python3".into(), "-m".into(), "pip".into(), "freeze".into()]))
            .map(|s| {
                s.lines()
                    .map(str::trim)
                    .filter(|l| bases.contains(&pip_norm_name(l)))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };

    // Commit the PRISTINE container (before any user workload has ever run in it) to the derived tag.
    if podman_as_dev_quiet(&["commit".into(), dctr.clone(), derived_tag.to_string()]).map(|c| c != 0).unwrap_or(true) {
        return fail(&net, "workshop launch: podman commit of the derivation failed".into());
    }
    // Tear the derivation container down — the derived IMAGE persists at `derived_tag` for the workload phase.
    cleanup(&net);
    Ok((apt_resolved, pip_resolved))
}

/// The bare package name of an apt token for a `dpkg-query` provenance lookup: everything before the first
/// `=` (an apt version pin `sl=3.03-17` → `sl`). The token is already [`valid_pkg_token`], so this only ever
/// strips a pin, never smuggles a flag.
fn apt_base_name(token: &str) -> String {
    token.split('=').next().unwrap_or(token).to_string()
}

/// The NORMALIZED distribution name of a pip token or a `pip freeze` line: the name portion (before any
/// `=<>~!@` version/marker punctuation or whitespace), lowercased with `_`→`-` (PEP 503 normalization is
/// coarser but this matches freeze output well enough to pair a declared token with its resolved line).
fn pip_norm_name(s: &str) -> String {
    let name: String = s
        .chars()
        .take_while(|c| !matches!(c, '=' | '<' | '>' | '~' | '!' | '@' | ' ' | '\t' | '['))
        .collect();
    name.trim().to_ascii_lowercase().replace('_', "-")
}

/// Commit a pre-checked `workshop launch` (called after the ceremony approves, or inline by root's `cli()`).
/// Two-container flow (owner decision #2997) with the Tool Shed cache (Commit 3): (A) resolve the derivation
/// IMAGE — a content-address cache HIT reuses a previously derived+verified image with NO egress; a MISS
/// derives a PRISTINE image (holder+exec, install-egress only) and RETAINS+indexes it in the Tool Shed; (B)
/// a FRESH ephemeral bench materialized from the recipe runs the workload FROM that image with the recipe's
/// grants/egress + a clean `/work`; (C) the bench is torn down but the cached image PERSISTS (a transient
/// image — only produced when the key can't be computed — is deleted). `refresh` forces a MISS (re-derive +
/// overwrite the entry); `offline` reduces the workload's runtime egress to none AND refuses to derive (an
/// offline launch is only valid on a cache HIT). The recipe stays the sole authoritative state; the cache is
/// a disposable optimization (mycelium #2994). Returns the workload's rc (or a non-zero derivation failure).
fn commit_launch(workshop: &str, seed: &str, apt: &[String], pip: &[String], grants: &[String], workload: &[String], refresh: bool, offline: bool) -> i32 {
    if !valid_seed(seed) {
        eprintln!("workshop launch: {seed:?} is not a sealed seed (re-check) — refused");
        return 2;
    }
    if workload.is_empty() {
        eprintln!("workshop launch: a workload is required after `--` (interactive launch is not offered over the socket)");
        return 2;
    }
    let lname = format!("wl-{workshop}");
    if !bench_record::valid_bench_name(&lname) {
        eprintln!("workshop launch: derived bench name {lname:?} is not a safe token (workshop name too long)");
        return 2;
    }
    let net = egress_profiles_from(grants);
    let has_packages = !apt.is_empty() || !pip.is_empty();

    // Phase A: resolve the environment image. A package-free recipe has no derivation — the seed IS the
    // environment, so there is nothing to cache and `--offline` is trivially satisfiable (empty net below).
    let mut transient_tag: Option<String> = None;
    let derived: Option<String> = if has_packages {
        // The cache key hashes the derivation INPUTS the recipe approves (seed image Id + declared apt/pip +
        // egress set). Fail-OPEN on the cache infra: if the seed Id or the hash can't be produced we still
        // launch — just without caching — rather than fail a launch on a cache hiccup.
        let seed_id = seed_image_id(seed);
        let key = seed_id.as_deref().and_then(|sid| sha256_hex_stdin(&workshop_cache::canonical_key_input(sid, apt, pip, &net)));

        // `--refresh` evicts any existing entry (index + bytes) so the re-derive below doesn't leave a
        // dangling image, then treats it as a MISS.
        if refresh {
            if let Some(k) = &key {
                if let Some(old) = workshop_cache::load_record(&workshop_cache::cache_dir(), k) {
                    let _ = podman_as_dev_quiet(&["rmi".into(), "-f".into(), old.image.clone()]);
                }
                let _ = workshop_cache::remove_record(&workshop_cache::cache_dir(), k);
            }
        }

        let hit = if refresh { None } else { key.as_deref().and_then(cache_lookup) };
        match hit {
            Some(rec) => {
                eprintln!("gatekeeperd/bench_plane: cache HIT ws={workshop} key={} image={}", &rec.key[..12], rec.image);
                Some(rec.image)
            }
            None => {
                // MISS. `--offline` refuses to touch the network to derive: an offline launch is valid ONLY
                // on a HIT (assembly needs no egress) — with no cached derivation there is nothing to run.
                if offline {
                    eprintln!("workshop launch: --offline but no cached derivation for {workshop} — re-run online to derive it first (fail-closed)");
                    return 1;
                }
                // With a key: derive straight to the cache tag and RETAIN+index it. Without a key (the hash
                // infra failed): derive to a transient tag we delete after the run (Commit-2 behaviour).
                let tag = match &key {
                    Some(k) => workshop_cache::image_tag(k),
                    None => format!("localhost/shrek-derive-{workshop}"),
                };
                let (apt_resolved, pip_resolved) = match derive_pristine(workshop, seed, apt, pip, &net, &tag) {
                    Ok(r) => r,
                    Err(()) => return 1,
                };
                match &key {
                    // With a key, RETAIN the image and index it in the Tool Shed for reuse.
                    Some(k) => cache_store(k, &tag, seed_id.as_deref().unwrap_or_default(), apt_resolved, pip_resolved),
                    // No key (hash infra failed): the image is transient — delete it after the run.
                    None => transient_tag = Some(tag.clone()),
                }
                Some(tag)
            }
        }
    } else {
        None
    };

    // Phase B: a fresh ephemeral bench from the recipe, running the workload FROM the derived image. Under
    // `--offline` the workload runs with an EMPTY net set (an authority REDUCTION, ceremony-consistent): the
    // assembled env needs no egress and the human already consented to ≤ the recipe's declared maximum.
    let run_grants: Vec<String> = if offline {
        grants.iter().filter(|g| !matches!(Grant::parse(g), Some(Grant::Net { .. }))).cloned().collect()
    } else {
        grants.to_vec()
    };
    let _ = destroy(&lname); // clear any stale instance from a crashed prior launch (idempotent)
    let rc = run_launch_workload(&lname, seed, &run_grants, derived.as_deref(), workload);
    let _ = destroy(&lname);

    // Phase C: delete ONLY a transient derived image — a cached image PERSISTS in the Tool Shed for reuse.
    if let Some(tag) = &transient_tag {
        let _ = podman_as_dev_quiet(&["rmi".into(), "-f".into(), tag.clone()]);
    }
    rc
}

/// Phase B helper: create the ephemeral launch bench, apply the recipe's declared grants (post-consent, so
/// no re-ceremony), and run the workload FROM the derived image with a fresh `/work` + the recipe's egress.
fn run_launch_workload(lname: &str, seed: &str, grants: &[String], image_override: Option<&str>, workload: &[String]) -> i32 {
    let crc = create(lname, DEFAULT_QUOTA_KIB, seed);
    if crc != 0 {
        eprintln!("workshop launch: could not create the ephemeral launch bench");
        return crc;
    }
    let rdir = bench_record::records_dir();
    let Some(mut rec) = bench_record::load_record(&rdir, lname) else {
        eprintln!("workshop launch: launch bench vanished after create");
        return 1;
    };
    // Apply the recipe's declared grants (fs + net) directly — the ceremony already approved them; this is
    // the activation, not a fresh authority request. (Exports are launcher .desktops, irrelevant to a
    // one-shot run — a persisted launched workshop is deferred to the cache.)
    rec.grants = grants.to_vec();
    if let Err(e) = bench_record::write_record(&rdir, &rec) {
        eprintln!("workshop launch: could not apply the recipe grants to the launch bench: {e}");
        return 1;
    }
    // Run the workload FROM the derived image (grant materialization + holder+exec egress happen in `run`).
    run_from(lname, false, workload, image_override)
}

fn commit_grant(bench: &str, canonical: &Path, leaf: &str, rw: bool, expected: Ident) -> i32 {
    let rdir = bench_record::records_dir();
    let Some(mut rec) = bench_record::load_record(&rdir, bench) else {
        eprintln!("bench grant: no such bench {bench}");
        return 4;
    };
    let anchor = anchor_dir();
    let Ok(rel) = canonical.strip_prefix(&anchor) else { return 2 };
    let Some(rel_str) = rel.to_str() else { return 2 };
    let anchor_fd = match open_anchor(&anchor) {
        Ok(f) => f,
        Err(e) => { eprintln!("bench grant: anchor open failed: {e}"); return 1; }
    };
    // RE-PIN at apply time and require the SAME object identity approved — the swap defense (invariant
    // 5). A name→symlink swap fails the NO_SYMLINKS openat2 walk (ELOOP); a rename to a different real
    // dir yields a different inode → identity mismatch. Either way: refuse, never apply what was not seen.
    let pinned = match pin_beneath(&anchor_fd, rel_str) {
        Ok(p) => p,
        Err(e) => { eprintln!("bench grant: re-pin failed at apply (swap defense): {e}"); return 1; }
    };
    if !pinned.is_dir || pinned.ident != expected {
        eprintln!("bench grant: target object identity changed since approval — refusing (swap defense)");
        return 1;
    }
    if let Err(e) = prepare_grant_dir(&rec.id) {
        eprintln!("bench grant: could not prepare the grant dir: {e}");
        return 1;
    }
    let target = grants_dir(&rec.id).join(leaf);
    if !is_mountpoint(&target) {
        let r = if rw { relocate_rw(&pinned, &target) } else { relocate_ro(&pinned, &target) };
        if let Err(e) = r {
            eprintln!("bench grant: materialization failed (fail-closed): {e}");
            return 1;
        }
    }
    rec.grants.push(Grant::Fs { rw, path: canonical.to_path_buf() }.encode());
    if let Err(e) = bench_record::write_record(&rdir, &rec) {
        eprintln!("bench grant: record write failed: {e}");
        return 1;
    }
    println!("bench: granted {} to {bench} at {} ({})", canonical.display(), grant_mountpoint(leaf), if rw { "rw" } else { "ro" });
    0
}

fn commit_network(bench: &str, profiles: &[String]) -> i32 {
    let rdir = bench_record::records_dir();
    let Some(mut rec) = bench_record::load_record(&rdir, bench) else {
        eprintln!("bench network: no such bench {bench}");
        return 4;
    };
    // Re-validate the WHOLE set against sealed policy at apply time (fail-closed on any unknown). The plan
    // could only have been built from valid names, but re-checking keeps commit self-sufficient — it never
    // trusts the plan's strings for the policy decision (Fable item-1 fix 5).
    for p in profiles {
        if shrek_policy::egress::resolve(p).is_none() {
            eprintln!("bench network: {p:?} not a sealed egress profile (re-check) — refused");
            return 2;
        }
    }
    // Declarative replace: drop all existing net grants, then write the validated set in order. The record
    // write is atomic (temp+rename) so no partial multi-`net` record is ever observable.
    rec.grants.retain(|g| !matches!(Grant::parse(g), Some(Grant::Net { .. })));
    for p in profiles {
        rec.grants.push(Grant::Net { profile: p.clone() }.encode());
    }
    if let Err(e) = bench_record::write_record(&rdir, &rec) {
        eprintln!("bench network: record write failed: {e}");
        return 1;
    }
    println!("bench: {bench} egress policy set to [{}] (injected per run)", profiles.join(", "));
    0
}

fn commit_export(bench: &str, key: &str, file: &str, icon: &str, label: &str, cmd: &[String]) -> i32 {
    let rdir = bench_record::records_dir();
    let Some(mut rec) = bench_record::load_record(&rdir, bench) else {
        eprintln!("bench export: no such bench {bench}");
        return 4;
    };
    if rec.exports.iter().filter_map(|e| Export::parse(e)).any(|e| e.key == key) {
        eprintln!("bench export: key {key:?} is already exported from {bench}");
        return 1;
    }
    if let Err(e) = write_desktop_as_dev(file, &desktop_content(bench, key, label, icon)) {
        eprintln!("bench export: .desktop write failed: {e}");
        return 1;
    }
    let exp = Export { key: key.to_string(), file: file.to_string(), icon: icon.to_string(), label: label.to_string(), cmd: cmd.to_vec() };
    rec.exports.push(exp.encode());
    if let Err(e) = bench_record::write_record(&rdir, &rec) {
        eprintln!("bench export: record write failed: {e}");
        remove_desktop_as_dev(file);
        return 1;
    }
    println!("bench: exported {bench}:{key} -> [{}] (launcher app {file})", cmd.join(" "));
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
            let mut seed = "scratch".to_string();
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--quota" => { i += 1; quota = rest.get(i).and_then(|s| s.parse().ok()).unwrap_or(quota); }
                    "--seed" => { i += 1; match rest.get(i) { Some(s) => seed = s.clone(), None => { eprintln!("bench create: --seed needs a value"); return 2; } } }
                    other if !other.starts_with('-') && name.is_none() => name = Some(other.to_string()),
                    other => { eprintln!("bench create: unexpected arg {other}"); return 2; }
                }
                i += 1;
            }
            match name {
                Some(n) => create(&n, quota, &seed),
                None => { eprintln!("usage: gatekeeperd bench create <name> [--quota KiB] [--seed NAME]"); 2 }
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
            // Declarative SET: `network <name> <profile...|none>`. `network` (root/cli) validates the whole
            // list itself; an empty tail is a usage error inside `network` → validate_profile_set.
            match rest.split_first() {
                Some((name, profiles)) => network(name, profiles),
                None => { eprintln!("usage: gatekeeperd bench network <name> <profile...|none>"); 2 }
            }
        }
        "export" => {
            // export <name> <key> [--label L] [--icon I] -- <workload...>
            let mut name = None;
            let mut key = None;
            let mut label = None;
            let mut icon = None;
            let mut workload: Vec<String> = Vec::new();
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--" => { workload = rest[i + 1..].to_vec(); break; }
                    "--label" => { i += 1; label = rest.get(i).cloned(); }
                    "--icon" => { i += 1; icon = rest.get(i).cloned(); }
                    other if !other.starts_with('-') && name.is_none() => name = Some(other.to_string()),
                    other if !other.starts_with('-') && key.is_none() => key = Some(other.to_string()),
                    other => { eprintln!("bench export: unexpected arg {other}"); return 2; }
                }
                i += 1;
            }
            match (name, key) {
                (Some(n), Some(k)) => export(&n, &k, label.as_deref(), icon.as_deref(), &workload),
                _ => { eprintln!("usage: gatekeeperd bench export <name> <key> [--label L] [--icon I] -- <cmd...>"); 2 }
            }
        }
        "run-export" => {
            match (rest.first(), rest.get(1)) {
                (Some(n), Some(k)) => run_export(n, k),
                _ => { eprintln!("usage: gatekeeperd bench run-export <name> <key>"); 2 }
            }
        }
        "unexport" => {
            match (rest.first(), rest.get(1)) {
                (Some(n), Some(k)) => unexport(n, k),
                _ => { eprintln!("usage: gatekeeperd bench unexport <name> <key>"); 2 }
            }
        }
        "promote" => {
            // ROOT runs no ceremony (no VT peer): it drives the SAME precheck+commit the socket ceremony
            // wraps, so root's in-process promote and a dev-over-socket promote write a byte-identical
            // recipe (the proofs/boot path stays root, no console — mirrors grant/network/export).
            match precheck_authority("promote", rest) {
                Ok(plan) => commit_authority(&plan),
                Err((rc, msg)) => { eprintln!("bench promote: {msg}"); rc }
            }
        }
        "workshop-list" => workshop_list(),
        "workshop-show" => match rest.first() {
            Some(n) => workshop_show(n),
            None => { eprintln!("usage: gatekeeperd bench workshop-show <name>"); 2 }
        },
        "workshop-launch" => {
            // Authority-MATERIALIZING; root drives the SAME precheck+commit the socket ceremony wraps.
            match precheck_authority("launch", rest) {
                Ok(plan) => commit_authority(&plan),
                Err((rc, msg)) => { eprintln!("workshop launch: {msg}"); rc }
            }
        }
        "" => { eprintln!("usage: gatekeeperd bench <create|run|enter|grant|network|export|run-export|unexport|promote|workshop-list|workshop-show|workshop-launch|reset|quota|destroy|list|reissue> …"); 2 }
        other => { eprintln!("bench: unknown verb {other}"); 2 }
    }
}

/// The socket front-end for bench verbs — the authenticated `/run/shrek-gk.sock` path (ADR-003 Part 2
/// authorization slice). [`cli`] stays the ROOT-ONLY in-process boot path (`shrek-bench-reissue.service`
/// runs `gatekeeperd bench reissue` directly, as root, with no socket peer); this is the SECOND front end,
/// driven by the daemon's `handle_conn` AFTER the SO_PEERCRED gate admits an allowlisted peer. `argv` is the
/// already-`pct_decode`d `[subverb, arg0, arg1, …]` from the count-framed request. Returns `(rc, RESULT
/// lines)` for the `RESULT …`/`END <rc>` wire framing — one implementation of every verb, two front ends.
///
/// STEP 3 (consent ceremony): every NEUTRAL / REDUCING / read-only verb runs directly. The three
/// AUTHORITY-INCREASING verbs (`grant`, `network` to a profile, `export`) route through the console
/// consent ceremony ([`crate::consent::run_socket_consent`]) — a human OK on a kernel-owned VT the
/// session cannot spoof, applied only on an exact bound-tuple match. `network <name> none` REVOKES
/// egress (reducing authority) and stays ceremony-free. `cred`/`peer_fd` are the SO_PEERCRED identity
/// and the connection fd (for peer binding + disconnect detection). Interactive `enter` (podman `-it`,
/// needs a pty) is a non-goal for this request/response socket.
pub fn dispatch_socket(cred: Ucred, peer_fd: RawFd, argv: &[String]) -> (i32, Vec<String>) {
    let verb = argv.first().map(String::as_str).unwrap_or("");
    let rest = &argv[argv.len().min(1)..];
    // One summary RESULT line per mutating verb (state parity is asserted against the record/mount table,
    // not this text — see the oracle). rc is propagated verbatim to the client via `END <rc>`.
    let summ = |v: &str, name: &str, rc: i32| {
        vec![format!("RESULT bench-{v} {name} {}", if rc == 0 { "ok" } else { "fail" })]
    };
    match verb {
        "create" => {
            let mut name = None;
            let mut quota = DEFAULT_QUOTA_KIB;
            let mut seed = "scratch".to_string();
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--quota" => { i += 1; quota = rest.get(i).and_then(|s| s.parse().ok()).unwrap_or(quota); }
                    "--seed" => { i += 1; match rest.get(i) { Some(s) => seed = s.clone(), None => return (2, vec!["RESULT bench-create - refused seed-needs-value".into()]) } }
                    other if !other.starts_with('-') && name.is_none() => name = Some(other.to_string()),
                    other => return (2, vec![format!("RESULT bench-create - refused bad-arg-{other}")]),
                }
                i += 1;
            }
            match name {
                Some(n) => { let rc = create(&n, quota, &seed); (rc, summ("create", &n, rc)) }
                None => (2, vec!["RESULT bench-create - usage".into()]),
            }
        }
        "run" => {
            // NON-INTERACTIVE only over the socket; `enter` (`-it`) needs a pty (deferred).
            let mut name = None;
            let mut workload: Vec<String> = Vec::new();
            let mut i = 0;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--" => { workload = rest[i + 1..].to_vec(); break; }
                    other if name.is_none() && !other.starts_with('-') => name = Some(other.to_string()),
                    other => return (2, vec![format!("RESULT bench-run - refused bad-arg-{other}")]),
                }
                i += 1;
            }
            match name {
                Some(n) => { let rc = run(&n, false, &workload); (rc, summ("run", &n, rc)) }
                None => (2, vec!["RESULT bench-run - usage".into()]),
            }
        }
        "enter" => (2, vec!["RESULT bench-enter - refused interactive-not-over-socket".into()]),
        "run-export" => match (rest.first(), rest.get(1)) {
            (Some(n), Some(k)) => { let rc = run_export(n, k); (rc, summ("run-export", n, rc)) }
            _ => (2, vec!["RESULT bench-run-export - usage".into()]),
        },
        "unexport" => match (rest.first(), rest.get(1)) {
            (Some(n), Some(k)) => { let rc = unexport(n, k); (rc, summ("unexport", n, rc)) }
            _ => (2, vec!["RESULT bench-unexport - usage".into()]),
        },
        "reset" => match rest.first() {
            Some(n) => { let rc = reset(n); (rc, summ("reset", n, rc)) }
            None => (2, vec!["RESULT bench-reset - usage".into()]),
        },
        "destroy" => match rest.first() {
            Some(n) => { let rc = destroy(n); (rc, summ("destroy", n, rc)) }
            None => (2, vec!["RESULT bench-destroy - usage".into()]),
        },
        "quota" => match rest.first() {
            Some(n) => {
                let kib = rest.get(1).and_then(|s| s.parse::<u64>().ok());
                let rc = quota(n, kib);
                let val = bench_record::load_record(&bench_record::records_dir(), n)
                    .map(|r| r.quota_kib.to_string())
                    .unwrap_or_else(|| "-".into());
                (rc, vec![format!("RESULT bench-quota {n} quota_kib={val}")])
            }
            None => (2, vec!["RESULT bench-quota - usage".into()]),
        },
        "list" => {
            let mut lines: Vec<String> = records()
                .into_iter()
                .map(|r| format!("RESULT bench {} state={} project={} quota_kib={}", r.name, r.state, r.project, r.quota_kib))
                .collect();
            if lines.is_empty() {
                lines.push("RESULT bench - none".into());
            }
            (0, lines)
        }
        "reissue" => { let rc = reissue(); (rc, summ("reissue", "-", rc)) }
        // Authority-INCREASING verbs go through the console consent ceremony (step 3). `network <name>
        // none` REVOKES egress (reducing authority) → ceremony-free; a profile is authority-increasing.
        "grant" | "export" => crate::consent::run_socket_consent(cred, peer_fd, verb, rest),
        "network" => {
            // Ceremony-free ONLY for the EXACT reducing request `<name> none` with nothing after it. Any
            // other tail — including `<name> none <profile>` — is authority-bearing or malformed and must
            // NOT silently drop args into a revoke (Fable item-1 fix 2). Route everything else through the
            // ceremony, whose precheck rejects a mixed/dup/unknown set before any human is prompted.
            if rest.len() == 2 && rest[1] == "none" {
                let rc = network(&rest[0], std::slice::from_ref(&rest[1]));
                (rc, summ("network", &rest[0], rc))
            } else {
                crate::consent::run_socket_consent(cred, peer_fd, "network", rest)
            }
        }
        // promote is authority-DECLARING (mints a durable reusable recipe) → the console consent ceremony,
        // exactly like grant/export. precheck rejects a bad bench/name/package before any human is prompted.
        "promote" => crate::consent::run_socket_consent(cred, peer_fd, "promote", rest),
        // workshop-launch is authority-MATERIALIZING (activates the recipe's grants/egress + runs a workload)
        // → the console consent ceremony, like promote. precheck rejects a bad/un-launchable recipe first.
        "workshop-launch" => crate::consent::run_socket_consent(cred, peer_fd, "launch", rest),
        // workshop list/show are READ-ONLY (a recipe is root-owned world-readable metadata) → ceremony-free,
        // like `list`. The daemon reads the recipes and streams RESULT lines to the allowlisted peer.
        "workshop-list" => {
            let mut lines: Vec<String> = workshop_record::list_records(&workshop_record::workshops_dir())
                .into_iter()
                .map(|r| format!("RESULT workshop {} seed={} apt={} pip={} source={}", r.name, r.seed, r.apt.len(), r.pip.len(), r.source))
                .collect();
            if lines.is_empty() {
                lines.push("RESULT workshop - none".into());
            }
            (0, lines)
        }
        "workshop-show" => match rest.first() {
            Some(n) => match workshop_record::load_record(&workshop_record::workshops_dir(), n) {
                Some(r) => {
                    let mut lines = vec![
                        format!("RESULT workshop {} seed={} source={} created={}", r.name, r.seed, r.source, r.created),
                        format!("RESULT workshop-apt {}", r.apt.join(" ")),
                        format!("RESULT workshop-pip {}", r.pip.join(" ")),
                    ];
                    for g in &r.grants {
                        lines.push(format!("RESULT workshop-grant {g}"));
                    }
                    for e in &r.exports {
                        lines.push(format!("RESULT workshop-export {e}"));
                    }
                    (0, lines)
                }
                None => (4, vec![format!("RESULT workshop-show {n} fail")]),
            },
            None => (2, vec!["RESULT workshop-show - usage".into()]),
        },
        "" => (2, vec!["RESULT bench - usage".into()]),
        other => (2, vec![format!("RESULT bench-{other} - refused unknown-verb")]),
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
    fn export_roundtrips_argv_with_spaces_and_percent() {
        let e = Export {
            key: "convert".into(),
            file: "shrek-bench-media-convert.desktop".into(),
            icon: "application-x-executable".into(),
            label: "Convert Video (100%)".into(),
            cmd: vec!["ffmpeg".into(), "-i".into(), "my clip.mp4".into(), "out 50%.webm".into()],
        };
        let wire = e.encode();
        assert!(!wire.contains('\n'), "wire is single-line");
        // the space-bearing label/args are %-escaped so the space-split parse is faithful.
        assert!(wire.split(' ').count() >= 8, "each field is one space-free token: {wire}");
        assert_eq!(Export::parse(&wire), Some(e), "argv + label round-trip exactly (fix 3)");
    }

    #[test]
    fn export_parse_rejects_empty_workload() {
        // key + file + icon + label but no argv ⇒ not a valid export.
        assert!(Export::parse("k shrek-bench-a-k.desktop icon Label").is_none());
    }

    #[test]
    fn desktop_content_is_constrained() {
        let d = desktop_content("media", "convert", "My App", "app-icon");
        assert!(d.contains("Exec=/usr/bin/shrek-bench-run media convert"), "absolute wrapper Exec, 2 tokens");
        assert!(!d.contains('%'), "NO desktop field codes (%f/%u/…) — launchers pass no file args: {d}");
        assert!(d.contains("Terminal=false"));
        assert!(d.contains("Type=Application"));
        assert!(d.contains("GenericName=Shrek Bench app"), "badge so an export can't masquerade as a baked app");
        assert!(!d.contains("DBusActivatable"));
        assert!(!d.contains("\nActions="));
        assert!(!d.contains("MimeType"));
    }

    #[test]
    fn export_hardening_validators() {
        assert!(valid_icon("org.gnome.TextEditor"));
        assert!(!valid_icon("/home/dev/evil.png"), "icon path (spoof surface) rejected");
        assert!(!valid_icon(".hidden"));
        assert!(valid_desktop_file("shrek-bench-a-b.desktop"));
        assert!(!valid_desktop_file("../../etc/x.desktop"));
        assert!(!valid_desktop_file("evil.sh"));
        assert_eq!(sanitize_label(Some("  hi\nthere  ")).as_deref(), Some("hithere"), "control chars stripped, trimmed");
        assert_eq!(sanitize_label(Some("   ")), None);
        assert_eq!(sanitize_label(None), None);
    }

    #[test]
    fn pct_roundtrips() {
        for s in ["", "plain", "a b c", "100%", "tab\tnl\nend", "unicode-café"] {
            assert_eq!(pct_decode(&pct_encode(s)), s, "roundtrip {s:?}");
        }
        assert!(!pct_encode("a b%c\n").contains(' '), "no space survives encode");
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
    fn seed_tar_default_and_dir_override() {
        // each seed's archive lives in the sysext seeds dir by its catalog basename (the shipped build
        // ALWAYS uses it — the override is compiled out).
        std::env::remove_var("SHREK_BENCH_SEED_DIR");
        assert_eq!(seed_tar("scratch"), PathBuf::from("/usr/share/shrek/bench/seeds/scratch.tar"));
        assert_eq!(seed_tar("debian"), PathBuf::from("/usr/share/shrek/bench/seeds/debian.tar"));
        // SHREK_BENCH_SEED_DIR steers the oracle's staged-archive DIR ONLY in the oracle-env build; a
        // shipped build ignores it entirely (env can't redirect the trust anchor).
        std::env::set_var("SHREK_BENCH_SEED_DIR", "/seed");
        #[cfg(feature = "oracle-env")]
        assert_eq!(seed_tar("scratch"), PathBuf::from("/seed/scratch.tar"));
        #[cfg(not(feature = "oracle-env"))]
        assert_eq!(seed_tar("scratch"), PathBuf::from("/usr/share/shrek/bench/seeds/scratch.tar"));
        std::env::remove_var("SHREK_BENCH_SEED_DIR");
    }

    #[test]
    fn seed_catalog_resolves_image_and_validates_names() {
        assert!(valid_seed("scratch") && valid_seed("debian"));
        assert!(!valid_seed("bogus") && !valid_seed(""));
        assert_eq!(seed_image("scratch"), "localhost/scratch");
        assert_eq!(seed_image("debian"), "localhost/debian");
        // an unknown seed falls back to the scratch basename/image (create() rejects unknown names up front,
        // so a run never reaches here with a bad seed — this is belt-and-suspenders).
        assert_eq!(seed_tar("bogus"), PathBuf::from("/usr/share/shrek/bench/seeds/scratch.tar"));
        assert_eq!(seed_image("bogus"), "localhost/scratch");
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
        assert!(!a.contains(&"--rm".to_string()), "detached holder+exec run keeps the container until the workload exec returns");
        assert!(s.contains("--network=none"), "still --network=none — egress is INJECTED into this netns");
        assert!(s.contains("-v /run/shrek/bench/media/hosts:/etc/hosts:ro"), "bench-owned hosts bind: {s}");
    }

    #[test]
    fn podman_exec_argv_workdir_flags_before_ctr_name() {
        // Non-interactive: `exec --workdir /work <ctr> <workload...>` — flags STRICTLY before the ctr name so
        // a workload arg like `-c` is the command, not an exec flag. No -it.
        let wl = vec!["sh".to_string(), "-c".to_string(), "apt-get update".to_string()];
        let a = podman_exec_argv("shrek-bench-web", false, &wl);
        assert_eq!(a, vec!["exec", "--workdir", "/work", "shrek-bench-web", "sh", "-c", "apt-get update"]);
        // The container name precedes every workload token (podman stops option-parsing at the first positional).
        let ctr_i = a.iter().position(|x| x == "shrek-bench-web").unwrap();
        assert!(a.iter().position(|x| x == "--workdir").unwrap() < ctr_i, "flags precede the ctr name");
        assert!(a.iter().position(|x| x == "-c").unwrap() > ctr_i, "the workload's -c is AFTER the ctr (a command arg)");
        // Interactive adds -it (also before the ctr name).
        let ai = podman_exec_argv("shrek-bench-web", true, &["/bin/sh".to_string()]);
        assert_eq!(ai, vec!["exec", "-it", "--workdir", "/work", "shrek-bench-web", "/bin/sh"]);
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
    fn egress_profiles_reads_the_net_grant_set_in_order() {
        let mut r = BenchRecord {
            name: "b".into(), id: "b".into(), project: 100_000, quota_kib: 1024,
            created: 0, state: "created".into(), seed: "scratch".into(),
            grants: vec!["fs-ro /home/dev/in".into(), "net debian-apt".into(), "net pypi-https".into()],
            exports: vec![],
        };
        // Multiple net grants COMPOSE, ordered as recorded (the union is resolved at run time).
        assert_eq!(egress_profiles(&r), vec!["debian-apt".to_string(), "pypi-https".to_string()]);
        r.grants = vec!["fs-rw /home/dev/out".into()];
        assert!(egress_profiles(&r).is_empty());
    }

    #[test]
    fn validate_profile_set_declarative_grammar() {
        // Single profile: byte-for-byte the legacy single-arg behavior.
        assert_eq!(validate_profile_set(&["debian-apt".into()]).unwrap(), vec!["debian-apt".to_string()]);
        // Multiple profiles COMPOSE, input order preserved.
        assert_eq!(
            validate_profile_set(&["debian-apt".into(), "pypi-https".into()]).unwrap(),
            vec!["debian-apt".to_string(), "pypi-https".to_string()]
        );
        // `none` ALONE = the empty set (revoke-all).
        assert!(validate_profile_set(&["none".into()]).unwrap().is_empty());
        // `none` mixed with a real profile is REFUSED — both orderings (no silent drop; Fable item-1 fix 2).
        assert!(validate_profile_set(&["none".into(), "pypi-https".into()]).is_err());
        assert!(validate_profile_set(&["debian-apt".into(), "none".into()]).is_err());
        // An unknown name refuses the WHOLE call (fail-closed, default-deny).
        assert!(validate_profile_set(&["debian-apt".into(), "evil-exfil".into()]).is_err());
        // Duplicates are REFUSED (clear error, never a silent dedup).
        assert!(validate_profile_set(&["pypi-https".into(), "pypi-https".into()]).is_err());
        // A flag-looking token is refused (the set holds profile names, not flags).
        assert!(validate_profile_set(&["--rw".into()]).is_err());
        // Empty input is a usage error (use `none` to revoke).
        assert!(validate_profile_set(&[]).is_err());
    }

    #[test]
    fn precheck_network_rejects_bad_sets_before_any_record_io() {
        // These all fail in validation — BEFORE load_record — so they need no fixture and prove the human
        // is never prompted for a malformed/authority-laundering request (consent invariant 1). Covers both
        // `none`-mixed orderings (the must-fix-2 contract on the ceremony path), dup, unknown, and flags.
        for rest in [
            vec!["b".to_string(), "none".into(), "pypi-https".into()], // none first
            vec!["b".to_string(), "debian-apt".into(), "none".into()], // none last
            vec!["b".to_string(), "pypi-https".into(), "pypi-https".into()], // dup
            vec!["b".to_string(), "not-a-profile".into()], // unknown
            vec!["b".to_string(), "--rw".into()], // flag token
            vec!["b".to_string()], // empty set
        ] {
            assert!(precheck_network(&rest).is_err(), "precheck must reject {rest:?} pre-I/O");
        }
    }

    #[test]
    fn network_profile_rows_are_one_per_profile_never_joined() {
        // must-fix 4: the consent screen shows the COMPLETE set as one row per name — not a single joined
        // "debian-apt, pypi-https" line where a name could be visually lost.
        let rows = network_profile_rows(&["debian-apt".to_string(), "pypi-https".to_string()]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], ("Egress profile".to_string(), "debian-apt".to_string()));
        assert_eq!(rows[1], ("Egress profile".to_string(), "pypi-https".to_string()));
        assert!(!rows.iter().any(|(_, v)| v.contains(',')), "no joined multi-name value");
    }

    #[test]
    fn network_authority_action_says_set_not_attach() {
        // Replace semantics: the ceremony header must say SET (the verb replaces the whole set), not ATTACH.
        let plan = AuthorityPlan::test_plan("network", "media", false, false);
        let action = plan.action();
        assert!(action.contains("SET the network egress"), "truthful replace-semantics header: {action}");
        assert!(!action.to_lowercase().contains("attach"));
    }

    #[test]
    fn promote_is_high_authority_and_action_names_both_bench_and_workshop() {
        // promote mints a durable reusable authority template ⇒ HIGH-authority (typed confirmation code),
        // like export. The ceremony header must name BOTH the source bench and the target recipe.
        let plan = AuthorityPlan::test_plan("promote", "workbench", false, false);
        assert!(plan.high_authority(), "promote must demand the typed confirmation code");
        let action = plan.action();
        assert!(action.contains("PROMOTE"), "header says PROMOTE: {action}");
        assert!(action.contains("workbench") && action.contains("ws"), "names bench + workshop: {action}");
    }

    #[test]
    fn precheck_promote_rejects_before_touching_fs() {
        // These all fail during arg-validation, BEFORE any bench record is loaded (the human is never
        // asked): a missing --name, an unsafe workshop name, a flag-shaped package token, and a duplicate.
        // (AuthorityPlan is not Debug, so read the rc off the Err arm rather than `unwrap_err`.)
        let rc = |args: &[&str]| -> i32 {
            let v: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            match precheck_promote(&v) {
                Ok(_) => panic!("expected refusal for {args:?}"),
                Err((rc, _)) => rc,
            }
        };
        assert_eq!(rc(&["wb"]), 2, "missing --name is a usage error");
        assert_eq!(rc(&["wb", "--name", "../escape"]), 2, "unsafe workshop name refused");
        assert_eq!(rc(&["wb", "--name", "ws", "--apt", "--force-yes"]), 2, "a flag-shaped package token is refused");
        assert_eq!(rc(&["wb", "--name", "ws", "--pip", "six", "--pip", "six"]), 1, "a duplicate package token is refused");
    }

    #[test]
    fn launch_is_high_authority_and_action_names_the_workshop() {
        // launch materializes real authority (activates grants+egress, runs a workload) ⇒ HIGH-authority
        // (typed code), and re-consents per launch. The ceremony header names the recipe.
        let plan = AuthorityPlan::test_plan("launch", "toolshop", false, false);
        assert!(plan.high_authority(), "launch must demand the typed confirmation code");
        let action = plan.action();
        assert!(action.contains("LAUNCH") && action.contains("toolshop"), "header names the recipe: {action}");
    }

    #[test]
    fn precheck_launch_usage_without_workshop() {
        // No positional workshop ⇒ a usage error, before any recipe is loaded (the human is never asked).
        match precheck_launch(&["--".into(), "cmd".into()]) {
            Ok(_) => panic!("expected a usage refusal"),
            Err((rc, _)) => assert_eq!(rc, 2),
        }
        // A stray unknown flag is still refused (didn't silently become the workshop name).
        assert!(matches!(precheck_launch(&["--bogus".into()]), Err((2, _))));
    }

    #[test]
    fn launch_flags_parse_before_the_workshop_and_dashdash() {
        // `--refresh`/`--offline` are recognized modifiers wherever they sit before `--`; they must NOT be
        // mistaken for the positional workshop, and the workload after `--` is untouched. (No recipe on disk
        // here, so a well-formed parse still returns the recipe-not-found refusal — that's the tell that the
        // flags parsed and the positional `ws` was captured, not consumed by a flag.)
        for args in [
            vec!["--refresh".to_string(), "ws".into(), "--".into(), "true".into()],
            vec!["--offline".to_string(), "ws".into()],
            vec!["ws".to_string(), "--refresh".into(), "--offline".into(), "--".into(), "true".into()],
        ] {
            match precheck_launch(&args) {
                Err((4, msg)) => assert!(msg.contains("no such workshop ws"), "flags parsed, ws captured: {msg}"),
                Err((rc, msg)) => panic!("expected recipe-not-found (flags parsed), got Err({rc}, {msg})"),
                Ok(_) => panic!("expected recipe-not-found (flags parsed), got Ok"),
            }
        }
    }

    #[test]
    fn apt_base_name_strips_a_pin_only() {
        assert_eq!(apt_base_name("sl"), "sl");
        assert_eq!(apt_base_name("sl=3.03-17"), "sl"); // apt version pin stripped for dpkg-query
        assert_eq!(apt_base_name("python3-dev"), "python3-dev"); // a dash in the NAME is kept
    }

    #[test]
    fn pip_norm_name_normalizes_and_strips_version_markers() {
        // Pairs a declared pip token with its `pip freeze` line: name only, lowercased, `_`→`-`.
        assert_eq!(pip_norm_name("six==1.17.0"), "six");
        assert_eq!(pip_norm_name("Flask>=2.0"), "flask");
        assert_eq!(pip_norm_name("typing_extensions==4.9.0"), "typing-extensions");
        assert_eq!(pip_norm_name("requests [security]"), "requests");
        // The declared token and its freeze line normalize to the SAME key (so the filter pairs them).
        assert_eq!(pip_norm_name("Typing_Extensions"), pip_norm_name("typing-extensions==4.9.0"));
    }

    #[test]
    fn egress_profiles_from_reads_only_net_lines_in_order() {
        let grants = vec![
            "fs-ro /home/dev/in".to_string(),
            "net debian-apt".to_string(),
            "fs-rw /home/dev/out".to_string(),
            "net pypi-https".to_string(),
        ];
        assert_eq!(egress_profiles_from(&grants), vec!["debian-apt".to_string(), "pypi-https".to_string()]);
    }

    #[test]
    fn derive_run_argv_is_pristine_no_work_no_grants() {
        let argv = derive_run_argv("derive-ws", "localhost/debian", Path::new("/run/h"));
        assert!(!argv.iter().any(|a| a.contains(":/work")), "pristine derivation binds no /work");
        assert!(!argv.iter().any(|a| a == "-w"), "pristine derivation pins no workdir");
        assert!(argv.iter().any(|a| a == "--network=none"), "starts with no egress (late-attach)");
        assert_eq!(argv.last().unwrap(), "infinity", "runs a sleep-infinity holder as PID1");
        assert!(argv.iter().any(|a| a == "/run/h:/etc/hosts:ro"), "binds only the egress hosts file");
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
