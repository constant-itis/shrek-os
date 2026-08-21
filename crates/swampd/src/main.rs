//! swampd — Shrek's filesystem-intelligence daemon (swamp.md). Phase-6 Swamp slice-1: authority-
//! filtered project indexing + the `shrek find` query gate.
//!
//! Lifecycle (all AFTER Landlock self-confinement, which is the first thing that happens):
//!   1. CONFINE   — Landlock this process default-deny to the sealed indexable allow-set + its own
//!                  operational dirs (confine.rs, swamp.md §5). From here, an open() outside the
//!                  allow-set fails at the kernel — ~/Vault's bytes can never enter our address space.
//!   2. RECONCILE — arm an inotify watch on every allowed directory AND upsert every live object in ONE
//!                  walk, then prune DB rows for objects that vanished while swampd was down (crawl.rs,
//!                  §6/§7). The index is PERSISTENT (slice-3): it survives restart/reboot in /var and is
//!                  reconciled to reality on start rather than rebuilt from empty.
//!   3. SERVE     — single-thread reactor (server.rs, §9): `poll` the query socket AND the watcher fd,
//!                  so caller-scoped queries and live map-repair share one thread. Every query is
//!                  SO_PEERCRED-authenticated, authorized against the session's grant record, and
//!                  tagged with the index's freshness.
//!
//! swampd is availability-plane (fail-open, §10): if it dies nothing else breaks, only enhanced search
//! disappears. But it NEVER runs unconfined — a Landlock failure is fatal (fail-closed on the wall) —
//! and it never masquerades a non-live index as current: without a working watcher it refuses to serve,
//! and a degraded watcher marks every response STALE (slice-3 §3).
//!
//! Daemon-launch env (systemd unit in production; set by the oracle for the host repro). These are
//! operational config for the daemon PROCESS, not caller-influenced authority:
//!   SWAMP_HOME (home whose sealed member trees to index; default $HOME)
//!   SWAMP_STATE_DIR (DURABLE index db + query socket; default /var/lib/swamp — survives reboot)
//!   SWAMP_AUTHORITY_DIR (session grant records; default /run/shrek/authority)
//!   SWAMP_ALLOW_UID (extra uids allowed to connect to the query socket; root always allowed)

mod authority;
mod confine;
mod crawl;
mod embed;
mod index;
mod linux_uapi;
mod server;
mod watch;

use confine::Confinement;
use embed::{EmbeddingBackend, SemanticCtx};
use index::Index;
use std::path::PathBuf;
use watch::{Freshness, Watcher};

/// The semantic-tier interface/schema version — one component of the `semantic_version` rebuild key
/// (slice-4). Bump when the chunking or wire semantics change so stored vectors are wiped + re-embedded.
const SEMANTIC_INTERFACE_VERSION: u32 = 1;

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
    // Durable state (slice-3): /var survives reboot on the sealed image, unlike the tmpfs /run.
    let state_dir: PathBuf = env_or("SWAMP_STATE_DIR", "/var/lib/swamp").into();
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

fn open_persistent_index(cfg: &Config) -> Index {
    // Slice-3: the index is DURABLE derived state — reused in place across restart/reboot, never wiped
    // on start. Any WAL/SHM sidecars are recovered by SQLite; a schema-version mismatch or corruption is
    // handled inside Index::open (wipe + rebuild), and the startup reconcile brings it to current.
    let db = cfg.state_dir.join("index.db");
    match Index::open(&db) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("swampd: FATAL cannot open index {}: {e}", db.display());
            std::process::exit(1);
        }
    }
}

/// Construct the embedding backend from daemon-launch env (operational config set by the systemd unit,
/// NEVER caller-influenced authority). `SWAMP_EMBED_SOCKET` — the LOCAL unix socket of the off-image
/// `swamp-embed-proxy` (the sealed provider-profile channel) — GATES the whole tier: unset/empty ⇒ no
/// backend ⇒ semantic unavailable, FTS floor only (unchanged slice-3 behavior). The base never dials the
/// network; the proxy reaches the LAN provider over the gated egress plane (slice-4 §1.2/§1.3).
///
///   SWAMP_EMBED_SOCKET   proxy socket path (presence enables the tier)
///   SWAMP_EMBED_PROVIDER provider id  (default evo-x2-lan)
///   SWAMP_EMBED_MODEL    model id     (default embeddinggemma-300m)
///   SWAMP_EMBED_DIM      vector dim   (default 768 — the live EmbeddingGemma-300M)
fn build_backend() -> Option<embed::SocketBackend> {
    let sock = std::env::var("SWAMP_EMBED_SOCKET").ok().filter(|s| !s.is_empty())?;
    let identity = embed::BackendIdentity {
        provider_id: env_or("SWAMP_EMBED_PROVIDER", "evo-x2-lan"),
        model_id: env_or("SWAMP_EMBED_MODEL", "embeddinggemma-300m"),
        dim: env_or("SWAMP_EMBED_DIM", "768").parse::<u32>().unwrap_or(768),
        version: SEMANTIC_INTERFACE_VERSION,
    };
    eprintln!(
        "swampd: semantic tier ENABLED via {} ({}|{}|{})",
        sock, identity.provider_id, identity.model_id, identity.dim
    );
    Some(embed::SocketBackend::new(PathBuf::from(sock), identity))
}

/// Bind a backend into a runtime [`SemanticCtx`] and reconcile the persistent store's `semantic_version`
/// against it — a provider/model/dim/interface change wipes stale vectors so they are re-embedded from
/// scratch (a wipe only degrades ranking, never correctness). Returns `None` when no backend is present.
fn build_semantic<'a>(index: &Index, backend: &'a Option<embed::SocketBackend>) -> Option<SemanticCtx<'a>> {
    let b = backend.as_ref()?;
    let semantic_version = b.identity().semantic_version();
    match index.reconcile_semantic_version(&semantic_version) {
        Ok(true) => eprintln!("swampd: semantic_version changed → stale vectors wiped, will re-embed"),
        Ok(false) => {}
        Err(e) => eprintln!("swampd: semantic_version reconcile error ({e}) — continuing (FTS floor holds)"),
    }
    Some(SemanticCtx { backend: b, semantic_version })
}

/// Create the inotify watcher, or die. A swampd that cannot watch would silently serve an ever-more-
/// stale snapshot as if current — the exact thing slice-3 forbids. Refusing to serve is the SAFE
/// direction (search unavailable, never wrong-authority); the availability plane tolerates it.
fn watcher_or_die() -> Watcher {
    match Watcher::new() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("swampd: FATAL inotify unavailable ({e}) — refusing to serve a silently-static index");
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
        // Reconcile once (against the persistent DB) and print coverage stats, then exit (no serve).
        // For inspecting the map.
        Some("crawl") | Some("reconcile") => {
            ensure_dirs(&cfg);
            enforce_or_die(&cfg);
            let index = open_persistent_index(&cfg);
            let mut watcher = watcher_or_die();
            let backend = build_backend();
            let sem = build_semantic(&index, &backend);
            let stats = crawl::reconcile_full(&index, &cfg.home, &mut watcher, sem.as_ref());
            println!(
                "swampd: reconcile done objects={} texts={} pruned={} skipped_never={} deleted={} semantic={}",
                stats.objects,
                stats.texts,
                stats.pruned,
                stats.skipped_never,
                stats.deleted,
                if sem.is_some() { "enabled" } else { "unavailable" }
            );
            std::process::exit(0);
        }
        // Default (or explicit `serve`): confine → arm+reconcile → reactor (serve + live repair).
        Some("serve") | None => {
            ensure_dirs(&cfg);
            enforce_or_die(&cfg);
            let index = open_persistent_index(&cfg);
            let mut watcher = watcher_or_die();
            // Semantic tier (slice-4): present only if a provider socket is configured; else FTS floor.
            let backend = build_backend();
            let sem = build_semantic(&index, &backend);
            // ONE walk arms every watch AND reconciles the persistent map (embedding enabled domains as it
            // goes); FRESH iff all watches armed. Embedding failures degrade to FTS, never fail the walk.
            let stats = crawl::reconcile_full(&index, &cfg.home, &mut watcher, sem.as_ref());
            let freshness = if watcher.healthy() { Freshness::Fresh } else { Freshness::Stale };
            eprintln!(
                "swampd: initial reconcile objects={} texts={} pruned={} skipped_never={} deleted={} freshness={} semantic={}",
                stats.objects,
                stats.texts,
                stats.pruned,
                stats.skipped_never,
                stats.deleted,
                freshness.wire(),
                if sem.is_some() { "enabled" } else { "unavailable" }
            );
            let sock = cfg.state_dir.join("query.sock");
            let srv = server::Server::new(&index, cfg.authority_dir.clone(), cfg.allowed_uids.clone(), sem.as_ref());
            if let Err(e) = srv.serve(&sock, &mut watcher, &cfg.home, freshness) {
                eprintln!("swampd: FATAL serve error: {e}");
                std::process::exit(1);
            }
        }
        Some(other) => {
            eprintln!("swampd: unknown subcommand {other:?} (expected: serve | crawl | reconcile | confine-probe)");
            std::process::exit(2);
        }
    }
}
