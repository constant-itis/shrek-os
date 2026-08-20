//! swampd — Shrek's filesystem-intelligence daemon (swamp.md). Phase-6 Swamp slice-1: authority-
//! filtered project indexing + the `shrek find` query gate.
//!
//! Lifecycle (all AFTER Landlock self-confinement, which is the first thing that happens):
//!   1. CONFINE  — Landlock this process default-deny to the sealed indexable allow-set + its own
//!                 operational dirs (confine.rs, swamp.md §5). From here, an open() outside the
//!                 allow-set fails at the kernel — ~/Vault's bytes can never enter our address space.
//!   2. CRAWL    — one-shot map of the allowed member trees → SQLite metadata + FTS5 (crawl.rs, §7).
//!   3. SERVE    — caller-scoped query API over a root-owned unix socket (server.rs, §9): every query
//!                 is SO_PEERCRED-authenticated and authorized against the session's grant record.
//!
//! swampd is availability-plane (fail-open, §10): if it dies nothing else breaks, only enhanced search
//! disappears. But it NEVER runs unconfined — a Landlock failure is fatal (fail-closed on the wall).
//!
//! Daemon-launch env (systemd unit in production; set by the oracle for the host repro). These are
//! operational config for the daemon PROCESS, not caller-influenced authority:
//!   SWAMP_HOME (home whose sealed member trees to index; default $HOME)
//!   SWAMP_STATE_DIR (index db + query socket; default /run/swamp)
//!   SWAMP_AUTHORITY_DIR (session grant records; default /run/shrek/authority)
//!   SWAMP_ALLOW_UID (extra uids allowed to connect to the query socket; root always allowed)

mod authority;
mod confine;
mod crawl;
mod index;
mod linux_uapi;
mod server;

use confine::Confinement;
use index::Index;
use std::path::PathBuf;

/// System directories swampd needs to run (loader/NSS/proc/dev). Read+exec; not protected user
/// domains. Missing ones are skipped by `confine`.
const SYSTEM_ROOTS: &[&str] =
    &["/usr", "/lib", "/lib64", "/lib32", "/bin", "/sbin", "/etc", "/proc", "/sys", "/dev"];

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

struct Config {
    home: PathBuf,
    state_dir: PathBuf,
    authority_dir: PathBuf,
    allowed_uids: Vec<u32>,
}

fn config() -> Config {
    let home = env_or("SWAMP_HOME", &env_or("HOME", "/root")).into();
    let state_dir: PathBuf = env_or("SWAMP_STATE_DIR", "/run/swamp").into();
    let authority_dir: PathBuf = env_or("SWAMP_AUTHORITY_DIR", "/run/shrek/authority").into();
    let mut allowed_uids = vec![0u32]; // root always allowed to connect
    if let Ok(extra) = std::env::var("SWAMP_ALLOW_UID") {
        for tok in extra.split([',', ' ']) {
            if let Ok(u) = tok.trim().parse::<u32>() {
                allowed_uids.push(u);
            }
        }
    }
    Config { home, state_dir, authority_dir, allowed_uids }
}

/// Build the confinement plan from the sealed allow-set + operational dirs. The SAME plan is used by
/// the serve path and the `confine-probe` acceptance verb, so the probe proves exactly what serving
/// enforces.
fn build_confinement(cfg: &Config) -> Confinement {
    let mut c = Confinement::new();
    for s in SYSTEM_ROOTS {
        c.system_read(*s);
    }
    for root in confine::index_member_roots(&cfg.home) {
        c.index_read(root);
    }
    c.authority_read(&cfg.authority_dir);
    c.state_rw(&cfg.state_dir);
    c
}

/// Best-effort creation of swampd's own dirs before confinement (they must exist to be added to the
/// ruleset; the state dir and authority dir are `required` roots).
fn ensure_dirs(cfg: &Config) {
    let _ = std::fs::create_dir_all(&cfg.state_dir);
    let _ = std::fs::create_dir_all(&cfg.authority_dir);
}

fn enforce_or_die(cfg: &Config) {
    if let Err(e) = build_confinement(cfg).enforce() {
        eprintln!("swampd: FATAL confinement failed ({e}) — refusing to run unconfined");
        std::process::exit(1);
    }
    eprintln!("swampd: Landlock confinement enforced (allow-set only)");
}

fn open_fresh_index(cfg: &Config) -> Index {
    // v1 index is a point-in-time snapshot rebuilt each start (swamp.md §11: mutable state on the
    // volatile plane; the live watcher + persistence are deferred). Start clean.
    let db = cfg.state_dir.join("index.db");
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(cfg.state_dir.join(format!("index.db{suffix}")));
    }
    match Index::open(&db) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("swampd: FATAL cannot open index {}: {e}", db.display());
            std::process::exit(1);
        }
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let cfg = config();

    match argv.first().map(String::as_str) {
        // Acceptance verb: enforce the REAL confinement, then report whether each path is openable.
        // Proves the swamp.md §5 kernel boundary — `swampd confine-probe ~/Vault/x` must be DENIED,
        // an allow-set member OK. Prints one `PROBE <path> OK|DENIED <errno>` line per arg.
        Some("confine-probe") => {
            ensure_dirs(&cfg);
            enforce_or_die(&cfg);
            for p in &argv[1..] {
                match std::fs::File::open(p) {
                    Ok(_) => println!("PROBE {p} OK"),
                    Err(e) => println!("PROBE {p} DENIED {}", e.raw_os_error().unwrap_or(-1)),
                }
            }
            std::process::exit(0);
        }
        // Crawl once and print coverage stats, then exit (no serve). For inspecting the map.
        Some("crawl") => {
            ensure_dirs(&cfg);
            enforce_or_die(&cfg);
            let index = open_fresh_index(&cfg);
            let stats = crawl::crawl(&index, &cfg.home);
            println!(
                "swampd: crawl done objects={} texts={} pruned={} skipped_never={}",
                stats.objects, stats.texts, stats.pruned, stats.skipped_never
            );
            std::process::exit(0);
        }
        // Default (or explicit `serve`): confine → crawl → serve.
        Some("serve") | None => {
            ensure_dirs(&cfg);
            enforce_or_die(&cfg);
            let index = open_fresh_index(&cfg);
            let stats = crawl::crawl(&index, &cfg.home);
            eprintln!(
                "swampd: initial map objects={} texts={} pruned={} skipped_never={}",
                stats.objects, stats.texts, stats.pruned, stats.skipped_never
            );
            let sock = cfg.state_dir.join("query.sock");
            let srv = server::Server::new(&index, cfg.authority_dir.clone(), cfg.allowed_uids.clone());
            if let Err(e) = srv.serve(&sock) {
                eprintln!("swampd: FATAL serve error: {e}");
                std::process::exit(1);
            }
        }
        Some(other) => {
            eprintln!("swampd: unknown subcommand {other:?} (expected: serve | crawl | confine-probe)");
            std::process::exit(2);
        }
    }
}
