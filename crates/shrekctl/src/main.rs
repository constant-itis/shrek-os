//! shrekctl — operator CLI.
//!
//! Phase-4: `shrekctl onion status` reads the audit record (/run/shrek/onion.json) written by the
//! gatekeeperd broker; `shrekctl onion activate|deactivate <name>` drives the broker's runtime API
//! over the root-owned socket (slice 2). shrekctl is a thin, UNPRIVILEGED client — it makes no policy
//! decision and performs no merge; the broker independently re-checks the sealed policy, so even the
//! operator cannot activate a non-policy or unsigned layer (docs/phase4-gatekeeperd.md).
//!
//! Dependency-free: the record is a stable one-object-per-line JSON, read line-oriented; the wire
//! protocol is line text. Override with SHREK_ONION_STATE / SHREK_BROKER_SOCK for the container repro.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

const ONION_STATE: &str = "/run/shrek/onion.json";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match (args.first().map(String::as_str), args.get(1).map(String::as_str)) {
        (Some("onion"), Some("status")) => onion_status(),
        (Some("onion"), Some("activate")) => onion_op("activate", args.get(2).map(String::as_str)),
        (Some("onion"), Some("deactivate")) => onion_op("deactivate", args.get(2).map(String::as_str)),
        (Some("onion"), _) => {
            eprintln!("usage: shrekctl onion status | activate <layer> | deactivate <layer>");
            std::process::exit(2);
        }
        _ => usage(),
    }
}

fn usage() {
    eprintln!("shrekctl {} — operator CLI", env!("CARGO_PKG_VERSION"));
    eprintln!("  shrekctl onion status                which signed layers are merged / omitted / refused");
    eprintln!("  shrekctl onion activate <layer>      ask the broker to merge a sealed+signed layer live");
    eprintln!("  shrekctl onion deactivate <layer>    ask the broker to unmerge a layer live");
    eprintln!("  (planned) shrek find|history|related|status; shrek run --trust=…; shrek audit --agent");
    std::process::exit(0);
}

/// Send a runtime request to the broker and print its verdict. The broker re-checks the sealed policy,
/// so a non-policy/unsigned layer is refused here just as at boot.
fn onion_op(verb: &str, name: Option<&str>) {
    let Some(name) = name else {
        eprintln!("usage: shrekctl onion {verb} <layer>");
        std::process::exit(2);
    };
    let sock = std::env::var("SHREK_BROKER_SOCK").unwrap_or_else(|_| "/run/shrek-gk.sock".into());
    let mut stream = match UnixStream::connect(&sock) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("shrekctl: broker unavailable at {sock}: {e}");
            std::process::exit(1);
        }
    };
    if writeln!(stream, "{verb} {name}").is_err() {
        eprintln!("shrekctl: broker write failed");
        std::process::exit(1);
    }
    let reader = BufReader::new(stream);
    for line in reader.lines().map_while(Result::ok) {
        let mut t = line.split_whitespace();
        match t.next() {
            Some("RESULT") => {
                let n = t.next().unwrap_or("?");
                let k = t.next().unwrap_or("?");
                let d = t.next().unwrap_or("?");
                let r = t.next().unwrap_or("-");
                println!("  {n} ({k}) -> {d}{}", if r == "-" { String::new() } else { format!(" ({r})") });
            }
            Some("END") => break,
            _ => {}
        }
    }
}

/// Value of a quoted field on a line: `"key": "value"` → `value`. Scoped to oniond's flat schema.
fn field(line: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":");
    let rest = &line[line.find(&needle)? + needle.len()..];
    let after = &rest[rest.find('"')? + 1..];
    Some(after[..after.find('"')?].to_string())
}

/// Value of a numeric field on a line: `"key": 0` → `0` (also handles a leading `-`).
fn num_field(line: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":");
    let rest = line[line.find(&needle)? + needle.len()..].trim_start();
    let n: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    (!n.is_empty()).then_some(n)
}

fn onion_status() {
    let path = std::env::var("SHREK_ONION_STATE").unwrap_or_else(|_| ONION_STATE.to_string());
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            println!("shrekctl: no onion record at {path} (oniond has not run)");
            return;
        }
    };

    let mut sysext_rc = String::from("?");
    let mut confext_rc = String::from("?");
    let mut rows: Vec<(String, String, String, String)> = Vec::new();
    for line in text.lines() {
        if line.contains("\"sysext_merge_rc\":") {
            if let Some(n) = num_field(line, "sysext_merge_rc") {
                sysext_rc = n;
            }
        }
        if line.contains("\"confext_merge_rc\":") {
            if let Some(n) = num_field(line, "confext_merge_rc") {
                confext_rc = n;
            }
        }
        if line.contains("\"name\":") && line.contains("\"decision\":") {
            rows.push((
                field(line, "name").unwrap_or_default(),
                field(line, "kind").unwrap_or_default(),
                field(line, "decision").unwrap_or_default(),
                field(line, "reason").unwrap_or_default(),
            ));
        }
    }

    println!(
        "shrek onion — {} layer(s)  (sysext merge rc={sysext_rc}, confext merge rc={confext_rc})",
        rows.len()
    );
    println!("  {:<16} {:<8} {:<9} {}", "LAYER", "KIND", "DECISION", "REASON");
    for (name, kind, decision, reason) in &rows {
        let reason = if reason.is_empty() { "-" } else { reason };
        println!("  {name:<16} {kind:<8} {decision:<9} {reason}");
    }
    println!("  (record: {path})");
}
