//! shrekctl — operator CLI.
//!
//! Phase-4 slice 1: `shrekctl onion status` reads oniond's audit record (/run/shrek/onion.json) and
//! prints a legible table of which signed layers were merged / omitted / refused. It is a thin,
//! UNPRIVILEGED reader — it makes no policy decision and performs no merge (that is oniond). This is
//! the operator query surface that replaces "read the serial console" (docs/phase4-oniond.md).
//!
//! Dependency-free: the record is oniond's own stable, one-object-per-line JSON, read line-oriented
//! (no general JSON parser). Override the path with SHREK_ONION_STATE for the container repro.

use std::fs;

const ONION_STATE: &str = "/run/shrek/onion.json";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match (args.first().map(String::as_str), args.get(1).map(String::as_str)) {
        (Some("onion"), Some("status")) => onion_status(),
        (Some("onion"), _) => {
            eprintln!("usage: shrekctl onion status");
            std::process::exit(2);
        }
        _ => usage(),
    }
}

fn usage() {
    eprintln!("shrekctl {} — operator CLI", env!("CARGO_PKG_VERSION"));
    eprintln!("  shrekctl onion status         which signed layers oniond merged / omitted / refused");
    eprintln!("  (planned) shrek find|history|related|status; shrek run --trust=…; shrek audit --agent");
    std::process::exit(0);
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
