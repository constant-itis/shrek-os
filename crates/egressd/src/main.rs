//! egressd (binary) — ADR-007 S2a: the store CLI.
//!
//! The long-running supervisor daemon (the `SO_PEERCRED`-gated bless socket) lands in S2d. In S2a the
//! binary exposes ONLY the store's state operations, which the host-oracle proof (S2e) and the boot
//! re-issuance path drive:
//!
//!   egressd store init                                  # lay out the 0700 skeleton
//!   egressd store bless   --profile <p> --tier <t> --at <secs>
//!   egressd store unbless --profile <p>
//!   egressd store pin     --profile <p> --at <secs> --pin <name>=<ipv4> [--pin ...]
//!   egressd store unpin   --profile <p>
//!   egressd store fault   --profile <p> --kind <unknown-profile|resolve-fail|apply-fail> --reason <t> --at <secs>
//!   egressd store project                               # rewrite /run/shrek/egress/pinned
//!   egressd store list                                  # dump blessed + pinned + faults
//!
//! Store paths are the sealed defaults; the oracle build (`--features oracle-env`) redirects them with
//! `SHREK_EGRESS_STORE` / `SHREK_EGRESS_RUN`. Every write goes through the same validated `store` fns
//! the daemon will, so the on-disk format is single-sourced. Returns a process exit code.

use std::net::Ipv4Addr;
use std::process::exit;

use std::time::Duration;

use egressd::apply::{self, ApplyError, ShellNft};
use egressd::dot;
use egressd::store::{
    self, store_dir, run_dir, BlessRecord, FaultKind, Pin, PinRecord,
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("daemon") => daemon_cli(&args[1..]),
        Some("reconcile") => reconcile_cli(),
        Some("compose-hosts") => compose_hosts_cli(),
        Some("ask") => egressd::client::ask(&args[1..]),
        Some("store") => store_cli(&args[1..]),
        Some("resolve") => resolve_cli(&args[1..]),
        Some("apply") => apply_cli(&args[1..]),
        Some("apply-browser") => apply_browser_cli(&args[1..]),
        _ => {
            eprintln!("egressd: usage:");
            eprintln!("  egressd daemon                                          # run the supervisor (uid-1000 socket)");
            eprintln!("  egressd compose-hosts                                   # ADR-008: (re)compose root-owned /run/shrek/hosts");
            eprintln!("  egressd ask <status|bless|unbless|repin|bind|unbind> [args]  # uid-1000 socket client (the UI front door)");
            eprintln!("  egressd store <init|bless|unbless|pin|unpin|fault|project|list> [args]");
            eprintln!("  egressd resolve --profile <p> [--at <secs>] [--apply]   # DoT-resolve + store pins (+apply)");
            eprintln!("  egressd apply --profile <p> [--unbless] [--at <secs>]   # reconcile stored pins into nft");
            eprintln!("  egressd apply-browser --path <cgroup> --level <n>       # insert browser-cgroup rules");
            eprintln!("  egressd ask confirmed-<bless|unbless|add-raw|remove-raw> <arg>   # ROOT-only ceremony commit (relayed to the daemon)");
            2
        }
    };
    exit(code);
}

/// Pull `--flag value` pairs and repeated `--pin name=ip` out of an arg slice. Minimal, dep-free.
struct Opts {
    single: std::collections::HashMap<String, String>,
    pins: Vec<Pin>,
}

fn parse_opts(args: &[String]) -> Result<Opts, String> {
    let mut single = std::collections::HashMap::new();
    let mut pins = Vec::new();
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        let key = a.strip_prefix("--").ok_or_else(|| format!("unexpected arg {a}"))?;
        // A `--flag` at the end, or followed by another `--x`, is a valueless boolean (e.g. --unbless).
        let takes_value = it.peek().map(|n| !n.starts_with("--")).unwrap_or(false);
        if !takes_value {
            single.insert(key.to_string(), String::new());
            continue;
        }
        let val = it.next().unwrap();
        if key == "pin" {
            let (name, ip) = val.split_once('=').ok_or_else(|| format!("--pin wants name=ipv4, got {val}"))?;
            let addr: Ipv4Addr = ip.parse().map_err(|_| format!("bad ipv4 in --pin: {ip}"))?;
            pins.push(Pin { name: name.to_string(), addr });
        } else {
            single.insert(key.to_string(), val.to_string());
        }
    }
    Ok(Opts { single, pins })
}

fn require<'a>(o: &'a Opts, key: &str) -> Result<&'a String, String> {
    o.single.get(key).ok_or_else(|| format!("--{key} required"))
}
fn parse_at(o: &Opts) -> Result<u64, String> {
    o.single.get("at").map(|s| s.parse::<u64>().map_err(|_| "bad --at".to_string())).unwrap_or(Ok(0))
}

/// `egressd apply` — reconcile the LIVE nft table from stored state, fail-closed. This is the S2b
/// enforcement front door the host-oracle proof drives against a real `nft` in a netns:
///   * `--profile <p>` reconciles `@<p>_pinned` to the addrs in the stored pin record. Unknown/broad ⇒
///     an `unknown-profile` fault is parked and NO element is written; an nft error ⇒ an `apply-fail`
///     fault (the add path already rolled back, so the deny skeleton stands). Success ⇒ re-project the
///     `/run` map + clear any prior fault.
///   * `--profile <p> --unbless` reconciles the set to empty.
///   * `--browser --path <cgroup> --level <n>` inserts the browser-cgroup rule pair above rule 0.
fn apply_cli(args: &[String]) -> i32 {
    let store = store_dir();
    let run = run_dir();
    let opts = match parse_opts(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("egressd apply: {e}");
            return 2;
        }
    };
    let mut exec = ShellNft;

    let profile = match opts.single.get("profile") {
        Some(p) => p.clone(),
        None => {
            eprintln!("egressd apply: --profile <p> required");
            return 2;
        }
    };
    let at = match parse_at(&opts) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("egressd apply: {e}");
            return 2;
        }
    };

    // Catalog for the state projection (source/feature card tokens + owner-capability lines).
    let catalog = egressd::catalog::load_catalog();

    // unbless: reconcile @cap_pinned to the union EXCLUDING this profile (nft-only; the durable records
    // are untouched — `store unbless`/`store unpin` remove those). Mirrors the daemon's teardown-first
    // order (ADR-009 §4.5).
    if opts.single.contains_key("unbless") {
        let desired = egressd::confirmed::desired_cap_union(&store, Some(&profile));
        return match apply::apply_cap(&mut exec, &desired) {
            Ok(_) => {
                let _ = store::project_pinned(&store, &run);
                let _ = store::project_state(&store, &run, &catalog);
                println!("egressd: withdrew {profile} from @cap_pinned");
                0
            }
            Err(e) => {
                eprintln!("egressd apply --unbless: {e:?}");
                1
            }
        };
    }

    // Reconcile the WHOLE @cap_pinned union from stored state (this profile's pins fold in with every
    // other grant's; ADR-009 §4.5). The oracle seeds the store with `store bless` + `store pin` first.
    match egressd::confirmed::reconcile_cap(&store, &mut exec) {
        Ok(present) => {
            let _ = store::clear_fault(&store, &profile);
            let _ = store::project_pinned(&store, &run);
            let _ = store::project_state(&store, &run, &catalog);
            println!("egressd: applied @cap_pinned union -> {} element(s)", present.len());
            0
        }
        Err(ApplyError::Nft(msg)) => {
            let _ = store::write_fault(&store, &profile, FaultKind::ApplyFail, &msg, at);
            let _ = store::project_state(&store, &run, &catalog);
            eprintln!("egressd apply: nft failure (rolled back, deny skeleton stands): {msg}");
            1
        }
    }
}

/// `egressd reconcile` — run ONE boot-style reconcile (re-apply every stored bless/pin + raw union +
/// blessed web-browsing) into the baked sets, flush-free, then exit. Root-only (mutates nft + the store).
/// The manual re-pin trigger the timer/network-online path would fire; also the oracle's "reboot" proof.
fn reconcile_cli() -> i32 {
    if egressd::uapi::geteuid() != 0 {
        eprintln!("egressd reconcile: refused — root only");
        return 2;
    }
    let store = store_dir();
    let run = run_dir();
    if let Err(e) = egressd::store::ensure_store(&store) {
        eprintln!("egressd reconcile: ensure store: {e}");
        return 1;
    }
    let _lock = egressd::store::lock_store(&store).ok();
    let mut exec = egressd::apply::ShellNft;
    let mut resolver = egressd::supervisor::DotResolver;
    let summary = egressd::supervisor::reconcile(&store, &run, &mut exec, &mut resolver, egressd::supervisor::now_unix());
    println!("egressd: {summary}");
    0
}

/// `egressd compose-hosts` — ADR-008 S2/S3: (re)compose the root-owned `/run/shrek/hosts` projection from
/// the sealed baseline + the owner's provider bindings, migrating + re-owning a pre-fix legacy hosts file
/// on the way. Run by the base `shrek-hosts-compose` oneshot at boot AND in-process by the daemon after a
/// `bind`/`unbind`. UNCONDITIONAL: localhost is always installed even if the binding store is absent or
/// hostile (`[R2-MF1]`). Takes the hosts lock (shared with the daemon). Root in production; the oracle
/// build redirects the paths via `SHREK_HOSTS_HOME` / `SHREK_HOSTS_RUN`.
fn compose_hosts_cli() -> i32 {
    let home = egressd::hosts::hosts_home_dir();
    let run = egressd::hosts::hosts_run_dir();
    let _lock = egressd::hosts::lock_hosts(&home).ok();
    // ADR-009: the sealed-source delivery filter needs the catalog (a variant with no manifests composes
    // baseline + provider-bindings only, fail-closed).
    let catalog = egressd::catalog::load_catalog();
    match egressd::hosts::compose_hosts(&home, &run, &catalog) {
        Ok(p) => {
            println!("egressd: composed hosts -> {}", p.display());
            0
        }
        Err(e) => {
            eprintln!("egressd compose-hosts: {e}");
            1
        }
    }
}

/// `egressd daemon` — run the S2d supervisor: bind the uid-1000 socket, reconcile blessed pins into the
/// baked sets (flush-free), and serve bless/unbless/re-pin. This is what the systemd unit runs. Never
/// returns except on a fatal bind error.
fn daemon_cli(_args: &[String]) -> i32 {
    let store = store_dir();
    let run = run_dir();
    eprintln!("egressd[boot]: starting supervisor (store={}, run={})", store.display(), run.display());
    match egressd::supervisor::serve(store, run) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("egressd[boot]: FATAL {e}");
            1
        }
    }
}

/// `egressd resolve --profile <p> [--at <secs>] [--apply]` — the S2c re-pin front door: DoT-resolve the
/// profile's sealed name(s) over the sealed resolver set, write the resulting pin record to the store,
/// and (with `--apply`) reconcile the nft set + re-project the `/run` map. Fail-closed: a resolution
/// failure parks a `resolve-fail` fault and writes NO pin (the prior pin, if any, and the baked deny
/// skeleton stand). The sealed daemon never resolves via getaddrinfo/resolved/NM — only this path.
fn resolve_cli(args: &[String]) -> i32 {
    let store = store_dir();
    let run = run_dir();
    let opts = match parse_opts(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("egressd resolve: {e}");
            return 2;
        }
    };
    let profile = match opts.single.get("profile") {
        Some(p) => p.clone(),
        None => {
            eprintln!("egressd resolve: --profile <p> required");
            return 2;
        }
    };
    let at = match parse_at(&opts) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("egressd resolve: {e}");
            return 2;
        }
    };
    let catalog = egressd::catalog::load_catalog();
    // Fixed query id (see dot:: docs — over authenticated single-query TLS the transport is the
    // security, not the id). Timeout kept short so a hung resolver fails closed to the next / to fault.
    let pins = match dot::resolve_profile_pins(&profile, 0x7e57, Duration::from_secs(5)) {
        Ok(p) => p,
        Err(e) => {
            let _ = store::write_fault(&store, &profile, FaultKind::ResolveFail, &e.to_string(), at);
            let _ = store::project_state(&store, &run, &catalog);
            eprintln!("egressd resolve: {e} — parked resolve-fail fault, no pin written");
            return 1;
        }
    };
    let rec = PinRecord { profile: profile.clone(), pins: pins.clone(), resolved: at };
    if let Err(e) = store::write_pin(&store, &rec) {
        // write_pin re-validates every name against the sealed profile; a rejection here is a seal/bug
        // fault, not a steer. Fail-closed.
        let _ = store::write_fault(&store, &profile, FaultKind::ResolveFail, &format!("store: {e}"), at);
        let _ = store::project_state(&store, &run, &catalog);
        eprintln!("egressd resolve: store pin rejected: {e}");
        return 1;
    }
    let _ = store::clear_fault(&store, &profile);
    let _ = store::project_state(&store, &run, &catalog);
    println!("egressd: resolved {profile} -> {} IP(s) over sealed DoT", pins.len());
    for p in &pins {
        println!("  {} {}", p.name, p.addr);
    }

    if opts.single.contains_key("apply") {
        let mut exec = ShellNft;
        // Fold the just-written pins into the @cap_pinned union (ADR-009 §4.5) — not a per-profile set.
        return match egressd::confirmed::reconcile_cap(&store, &mut exec) {
            Ok(present) => {
                let _ = store::project_pinned(&store, &run);
                let _ = store::project_state(&store, &run, &catalog);
                println!("egressd: applied @cap_pinned union -> {} element(s)", present.len());
                0
            }
            Err(e) => {
                let msg = format!("{e:?}");
                let _ = store::write_fault(&store, &profile, FaultKind::ApplyFail, &msg, at);
                let _ = store::project_state(&store, &run, &catalog);
                eprintln!("egressd resolve --apply: {msg}");
                1
            }
        };
    }
    0
}

/// `egressd apply-browser --path <cgroup> --level <n>` — insert the sole runtime rule pair
/// (browser-cgroup accept + browser-scope stub-accept) above rule 0. Driven at browser launch / by the
/// oracle when the `shrekbrowser.slice` cgroup exists.
fn apply_browser_cli(args: &[String]) -> i32 {
    let opts = match parse_opts(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("egressd apply-browser: {e}");
            return 2;
        }
    };
    let path = match opts.single.get("path") {
        Some(p) => p,
        None => {
            eprintln!("egressd apply-browser: --path <cgroup> required");
            return 2;
        }
    };
    let level: u32 = match opts.single.get("level").map(|s| s.parse()) {
        Some(Ok(n)) => n,
        _ => {
            eprintln!("egressd apply-browser: --level <n> required");
            return 2;
        }
    };
    let mut exec = ShellNft;
    match apply::install_browser_rules(&mut exec, path, level) {
        Ok(()) => {
            println!("egressd: inserted browser-cgroup rules for {path} (level {level})");
            0
        }
        Err(e) => {
            eprintln!("egressd apply-browser: {e:?}");
            1
        }
    }
}

fn store_cli(args: &[String]) -> i32 {
    let store = store_dir();
    let run = run_dir();
    let verb = match args.first() {
        Some(v) => v.as_str(),
        None => {
            eprintln!("egressd store: missing verb");
            return 2;
        }
    };
    let opts = match parse_opts(&args[1..]) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("egressd store: {e}");
            return 2;
        }
    };

    let result: Result<i32, String> = (|| match verb {
        "init" => {
            store::ensure_store(&store).map_err(|e| e.to_string())?;
            println!("egressd: store ready at {}", store.display());
            Ok(0)
        }
        "bless" => {
            let rec = BlessRecord {
                profile: require(&opts, "profile")?.clone(),
                tier: require(&opts, "tier")?.clone(),
                blessed: parse_at(&opts)?,
            };
            let p = store::write_bless(&store, &rec).map_err(|e| e.to_string())?;
            println!("egressd: blessed {} (tier {}) -> {}", rec.profile, rec.tier, p.display());
            Ok(0)
        }
        "unbless" => {
            let profile = require(&opts, "profile")?;
            store::remove_bless(&store, profile).map_err(|e| e.to_string())?;
            println!("egressd: unblessed {profile}");
            Ok(0)
        }
        "pin" => {
            let rec = PinRecord {
                profile: require(&opts, "profile")?.clone(),
                pins: opts.pins.clone(),
                resolved: parse_at(&opts)?,
            };
            let p = store::write_pin(&store, &rec).map_err(|e| e.to_string())?;
            println!("egressd: pinned {} ({} addr) -> {}", rec.profile, rec.pins.len(), p.display());
            Ok(0)
        }
        "unpin" => {
            let profile = require(&opts, "profile")?;
            store::remove_pin(&store, profile).map_err(|e| e.to_string())?;
            println!("egressd: unpinned {profile}");
            Ok(0)
        }
        "fault" => {
            let profile = require(&opts, "profile")?;
            let kind = match require(&opts, "kind")?.as_str() {
                "unknown-profile" => FaultKind::UnknownProfile,
                "resolve-fail" => FaultKind::ResolveFail,
                "apply-fail" => FaultKind::ApplyFail,
                other => return Err(format!("unknown fault kind {other}")),
            };
            let reason = opts.single.get("reason").cloned().unwrap_or_default();
            let p = store::write_fault(&store, profile, kind, &reason, parse_at(&opts)?)
                .map_err(|e| e.to_string())?;
            println!("egressd: fault {} ({}) -> {}", profile, kind.as_str(), p.display());
            Ok(0)
        }
        "project" => {
            let catalog = egressd::catalog::load_catalog();
            let p = store::project_pinned(&store, &run).map_err(|e| e.to_string())?;
            let s = store::project_state(&store, &run, &catalog).map_err(|e| e.to_string())?;
            println!("egressd: projected pinned map -> {}", p.display());
            println!("egressd: projected state view -> {}", s.display());
            Ok(0)
        }
        "list" => {
            println!("# blessed");
            for b in store::list_bless(&store) {
                println!("bless {} tier={} at={}", b.profile, b.tier, b.blessed);
            }
            println!("# pinned");
            for pr in store::list_pins(&store) {
                for pin in &pr.pins {
                    println!("pin {} {} {} at={}", pr.profile, pin.name, pin.addr, pr.resolved);
                }
            }
            Ok(0)
        }
        other => Err(format!("unknown verb {other}")),
    })();

    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("egressd store {verb}: {e}");
            1
        }
    }
}
