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

use egressd::store::{
    self, store_dir, run_dir, BlessRecord, FaultKind, Pin, PinRecord,
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("store") => store_cli(&args[1..]),
        _ => {
            eprintln!("egressd: usage: egressd store <init|bless|unbless|pin|unpin|fault|project|list> [args]");
            eprintln!("egressd: (the supervisor daemon socket lands in S2d)");
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
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let key = a.strip_prefix("--").ok_or_else(|| format!("unexpected arg {a}"))?;
        let val = it.next().ok_or_else(|| format!("--{key} needs a value"))?;
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
            println!("egressd: projected pinned map -> {}", p.display());
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
