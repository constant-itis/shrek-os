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
        Some("ask") => egressd::client::ask(&args[1..]),
        Some("store") => store_cli(&args[1..]),
        Some("resolve") => resolve_cli(&args[1..]),
        Some("apply") => apply_cli(&args[1..]),
        Some("apply-browser") => apply_browser_cli(&args[1..]),
        _ => {
            eprintln!("egressd: usage:");
            eprintln!("  egressd daemon                                          # run the supervisor (uid-1000 socket)");
            eprintln!("  egressd ask <status|bless|unbless|repin> [profile]      # uid-1000 socket client (the UI front door)");
            eprintln!("  egressd store <init|bless|unbless|pin|unpin|fault|project|list> [args]");
            eprintln!("  egressd resolve --profile <p> [--at <secs>] [--apply]   # DoT-resolve + store pins (+apply)");
            eprintln!("  egressd apply --profile <p> [--unbless] [--at <secs>]   # reconcile stored pins into nft");
            eprintln!("  egressd apply-browser --path <cgroup> --level <n>       # insert browser-cgroup rules");
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

    // unbless: reconcile to empty
    if opts.single.contains_key("unbless") {
        return match apply::unapply(&store, &mut exec, &profile) {
            Ok(()) => {
                let _ = store::project_pinned(&store, &run);
                let _ = store::project_state(&store, &run);
                println!("egressd: unblessed+unpinned {profile}");
                0
            }
            Err(e) => {
                eprintln!("egressd apply --unbless: {e:?}");
                1
            }
        };
    }

    // desired addrs come from the stored pin record (S2c will populate it via sealed DoT; here the
    // oracle/tests seed it with `store pin`).
    let desired: Vec<std::net::Ipv4Addr> = match store::load_pin(&store, &profile) {
        Some(rec) => rec.pins.iter().map(|p| p.addr).collect(),
        None => Vec::new(),
    };

    match apply::apply_pins(&store, &mut exec, &profile, &desired) {
        Ok(addrs) => {
            let _ = store::clear_fault(&store, &profile);
            let _ = store::project_pinned(&store, &run);
            let _ = store::project_state(&store, &run);
            println!("egressd: applied {profile} -> {} element(s)", addrs.len());
            0
        }
        Err(ApplyError::Unmanaged(p)) => {
            // unknown/broad/baseline/pre-pinned: park a fault, install NO element (fail-closed).
            let _ = store::write_fault(&store, &p, FaultKind::UnknownProfile, "not a pinnable set-managed profile", at);
            let _ = store::project_state(&store, &run);
            eprintln!("egressd apply: {p} is not pinnable — parked unknown-profile fault, no element written");
            1
        }
        Err(ApplyError::Nft(msg)) => {
            let _ = store::write_fault(&store, &profile, FaultKind::ApplyFail, &msg, at);
            let _ = store::project_state(&store, &run);
            eprintln!("egressd apply: nft failure (rolled back, deny skeleton stands): {msg}");
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
    // Fixed query id (see dot:: docs — over authenticated single-query TLS the transport is the
    // security, not the id). Timeout kept short so a hung resolver fails closed to the next / to fault.
    let pins = match dot::resolve_profile_pins(&profile, 0x7e57, Duration::from_secs(5)) {
        Ok(p) => p,
        Err(e) => {
            let _ = store::write_fault(&store, &profile, FaultKind::ResolveFail, &e.to_string(), at);
            let _ = store::project_state(&store, &run);
            eprintln!("egressd resolve: {e} — parked resolve-fail fault, no pin written");
            return 1;
        }
    };
    let rec = PinRecord { profile: profile.clone(), pins: pins.clone(), resolved: at };
    if let Err(e) = store::write_pin(&store, &rec) {
        // write_pin re-validates every name against the sealed profile; a rejection here is a seal/bug
        // fault, not a steer. Fail-closed.
        let _ = store::write_fault(&store, &profile, FaultKind::ResolveFail, &format!("store: {e}"), at);
        let _ = store::project_state(&store, &run);
        eprintln!("egressd resolve: store pin rejected: {e}");
        return 1;
    }
    let _ = store::clear_fault(&store, &profile);
    let _ = store::project_state(&store, &run);
    println!("egressd: resolved {profile} -> {} IP(s) over sealed DoT", pins.len());
    for p in &pins {
        println!("  {} {}", p.name, p.addr);
    }

    if opts.single.contains_key("apply") {
        let mut exec = ShellNft;
        let desired: Vec<std::net::Ipv4Addr> = pins.iter().map(|p| p.addr).collect();
        return match apply::apply_pins(&store, &mut exec, &profile, &desired) {
            Ok(a) => {
                let _ = store::project_pinned(&store, &run);
                let _ = store::project_state(&store, &run);
                println!("egressd: applied {profile} -> {} element(s)", a.len());
                0
            }
            Err(e) => {
                let msg = format!("{e:?}");
                let _ = store::write_fault(&store, &profile, FaultKind::ApplyFail, &msg, at);
                let _ = store::project_state(&store, &run);
                eprintln!("egressd resolve --apply: {msg}");
                1
            }
        };
    }
    0
}

/// `egressd apply-browser --path <cgroup> --level <n>` — insert the sole runtime rule pair
/// (browser-cgroup accept + browser-scope stub-accept) above rule 0. Driven at browser launch / by the
/// oracle when the `shrek-browser.slice` cgroup exists.
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
            let p = store::project_pinned(&store, &run).map_err(|e| e.to_string())?;
            let s = store::project_state(&store, &run).map_err(|e| e.to_string())?;
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
