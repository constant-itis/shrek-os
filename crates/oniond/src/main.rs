//! oniond — Shrek Onion policy client (Phase 4, slice 2).
//!
//! oniond is now UNPRIVILEGED (`User=shrek`): it holds no mount privilege and performs no merge. It
//! is the policy brain — it reads the sealed enable-list, proposes a DESIRED layer set, and asks the
//! privileged gatekeeperd broker to realize it over the root-owned socket. gatekeeperd independently
//! re-checks and is the only thing that mounts/merges (docs/phase4-gatekeeperd.md). If the broker is
//! unreachable, oniond fails CLOSED (no layers merge) but exits 0 so the OS still boots (fail-open
//! availability, security-model §7).
//!
//! Dependency-free. Env overrides for the repro: SHREK_ONION_POLICY, SHREK_ONION_STORE,
//! SHREK_BROKER_SOCK, SHREK_ONION_SELFTEST=0 to skip the privilege probe.

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::Command;
use std::time::Duration;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn main() {
    if std::env::args().nth(1).as_deref() != Some("merge") {
        eprintln!("oniond — Shrek Onion policy client (Phase 4).");
        eprintln!("usage: oniond merge   (propose the sealed layer set to the gatekeeperd broker)");
        std::process::exit(2);
    }

    println!("oniond: running as uid={} (unprivileged policy client)", self_uid());

    // G1 evidence: prove the privilege is really gone. A direct merge MUST be denied — systemd-sysext
    // checks CAP_SYS_ADMIN up front and returns EPERM ("Need to be privileged.") before any mount.
    if env_or("SHREK_ONION_SELFTEST", "1") != "0" {
        privilege_probe();
    }

    let sock = env_or("SHREK_BROKER_SOCK", "/run/shrek-gk.sock");
    let mut stream = match connect_retry(&sock, 15, Duration::from_millis(200)) {
        Some(s) => s,
        None => {
            // Layer plane fails CLOSED; OS availability fails OPEN — exit 0 so boot completes.
            println!("oniond: broker unavailable at {sock} — layers NOT merged (fail-closed); boot continues");
            std::process::exit(0);
        }
    };

    // Store is guaranteed mounted by the time the broker accepts (it mounts before binding), so the
    // optional inject marker is readable now. It simulates a COMPROMISED brain that requests a
    // signed-but-unsealed layer — the wall must refuse it (gate G3).
    let mut desired: BTreeSet<String> = read_policy(&env_or("SHREK_ONION_POLICY", "/usr/lib/shrek/onion-policy"));
    let store = env_or("SHREK_ONION_STORE", "/run/shrek-store");
    if let Ok(inj) = fs::read_to_string(format!("{store}/oniond-inject")) {
        for n in inj.split_whitespace() {
            println!("oniond: INJECT — additionally requesting '{n}' (compromised-brain simulation)");
            desired.insert(n.to_string());
        }
    }
    let desired: Vec<String> = desired.into_iter().collect();
    println!("oniond: requesting merge of [{}] via broker", desired.join(", "));

    if writeln!(stream, "merge {}", desired.join(" ")).is_err() {
        println!("oniond: broker write failed — layers NOT merged (fail-closed); boot continues");
        std::process::exit(0);
    }
    let reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => std::process::exit(0),
    });
    for line in reader.lines().map_while(Result::ok) {
        let mut t = line.split_whitespace();
        match t.next() {
            Some("RESULT") => {
                let name = t.next().unwrap_or("?");
                let kind = t.next().unwrap_or("?");
                let decision = t.next().unwrap_or("?");
                let reason = t.next().unwrap_or("-");
                if reason == "-" {
                    println!("oniond: {name} ({kind}) -> {decision}");
                } else {
                    println!("oniond: {name} ({kind}) -> {decision} ({reason})");
                }
            }
            Some("END") => {
                let sx = t.next().unwrap_or("?");
                let cx = t.next().unwrap_or("?");
                println!("oniond: sysext merge rc={sx} confext merge rc={cx}");
                break;
            }
            _ => {}
        }
    }
    std::process::exit(0);
}

fn self_uid() -> String {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))
                .and_then(|l| l.split_whitespace().nth(1).map(String::from))
        })
        .unwrap_or_else(|| "?".into())
}

fn privilege_probe() {
    match Command::new("systemd-sysext")
        .arg("--image-policy=root=signed+absent:usr=signed+absent")
        .arg("merge")
        .output()
    {
        Ok(o) if o.status.success() => {
            println!("oniond: WARN privilege probe MERGED directly — oniond is NOT unprivileged (G1 FAIL)");
        }
        Ok(o) => {
            let msg = String::from_utf8_lossy(&o.stderr);
            let msg = msg.lines().next().unwrap_or("").trim();
            println!("oniond: privilege probe DENIED (direct merge rc={}) — privilege correctly dropped: {msg}",
                o.status.code().unwrap_or(-1));
        }
        Err(e) => println!("oniond: privilege probe could not run systemd-sysext: {e}"),
    }
}

fn connect_retry(sock: &str, tries: u32, delay: Duration) -> Option<UnixStream> {
    for _ in 0..tries {
        if let Ok(s) = UnixStream::connect(sock) {
            return Some(s);
        }
        std::thread::sleep(delay);
    }
    None
}

fn read_policy(path: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    let Ok(text) = fs::read_to_string(path) else { return set };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        if parts.next() == Some("enable") {
            if let Some(n) = parts.next() {
                set.insert(n.to_string());
            }
        }
    }
    set
}
