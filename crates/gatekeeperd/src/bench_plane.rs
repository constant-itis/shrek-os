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
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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

/// Build the `podman run` argv for a Bench workload. `--network=none --no-hosts` (rule 3: benches start
/// with NO egress; step 5 late-attaches a grant; `--no-hosts` also sidesteps Shrek's /etc/hosts symlink,
/// #2816). The Bench's own quota-scoped data dir is bound at `/work` (the workload's cwd). `crun` runtime.
/// `-it` only for an interactive `enter`. The container is named `shrek-bench-<name>` so lifecycle verbs
/// can find it; `--rm` keeps the shared graphroot clean (Bench persistence lives in the /work data dir +
/// the record, not a stopped container).
fn podman_run_argv(name: &str, data: &Path, image: &str, interactive: bool, workload: &[String]) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "run".into(),
        "--rm".into(),
        "--name".into(),
        format!("shrek-bench-{name}"),
        "--network=none".into(),
        "--no-hosts".into(),
        "--runtime".into(),
        "crun".into(),
        "-v".into(),
        format!("{}:/work", data.display()),
        "-w".into(),
        "/work".into(),
    ];
    if interactive {
        a.push("-it".into());
    }
    a.push(image.to_string());
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

/// run <name> [-i] -- <workload>: run a container in the Bench (rootless, no-net). `-i` = interactive shell.
fn run(name: &str, interactive: bool, workload: &[String]) -> i32 {
    let rdir = bench_record::records_dir();
    let Some(mut rec) = bench_record::load_record(&rdir, name) else {
        eprintln!("bench: no such bench {name}");
        return 4;
    };
    let pool = pool_dir();
    let data = data_dir(&pool, name);
    let wl = if interactive && workload.is_empty() { vec!["/bin/sh".to_string()] } else { workload.to_vec() };
    let argv = podman_run_argv(name, &data, &seed_image(), interactive, &wl);

    rec.state = "running".into();
    let _ = bench_record::write_record(&rdir, &rec);
    let code = podman_as_dev(&argv).unwrap_or(-1);
    rec.state = "stopped".into();
    let _ = bench_record::write_record(&rdir, &rec);
    code
}

/// reset <name>: wipe the Bench's mutable data (its /work dir) but KEEP its identity, project id, quota,
/// and record — a "clean the workbench, keep the bench" operation.
fn reset(name: &str) -> i32 {
    let rdir = bench_record::records_dir();
    if bench_record::load_record(&rdir, name).is_none() {
        eprintln!("bench: no such bench {name}");
        return 4;
    }
    podman_rm(name);
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
    podman_rm(name);
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
    for r in &recs {
        if r.project != 0 {
            if apply_quota(r.project, r.quota_kib).is_ok() {
                applied += 1;
            }
            // Re-tag the data dir in case the project inherit flag was lost (defensive; cheap).
            let _ = chattr_project(r.project, &data_dir(&pool_dir(), &r.name));
        }
    }
    println!("bench: reissued {applied}/{} bench quota(s)", recs.len());
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
        // Step 5+: FS/egress grants + the ADR-002 promote path. Explicit stubs so the verb surface is
        // stable and a caller gets a clear "not yet", never a silent success.
        "grant" | "network" | "promote" => {
            eprintln!("bench {verb}: deferred to a later MVP step (grants=step 5, promote=step 9)");
            3
        }
        "" => { eprintln!("usage: gatekeeperd bench <create|run|enter|reset|quota|destroy|list|reissue> …"); 2 }
        other => { eprintln!("bench: unknown verb {other}"); 2 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn podman_run_argv_is_noexec_no_net_workdir_bound() {
        let a = podman_run_argv("media", Path::new("/home/.shrek/benches/b/media"), "localhost/scratch", false, &["ffmpeg".into(), "-i".into()]);
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
    fn enter_argv_is_interactive() {
        let a = podman_run_argv("b", Path::new("/p/b"), "img", true, &["/bin/sh".into()]);
        assert!(a.contains(&"-it".to_string()), "enter is interactive");
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
