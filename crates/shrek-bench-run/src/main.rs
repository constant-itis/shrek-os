//! shrek-bench-run — the fixed launcher target of an exported Bench `.desktop` (ADR-003 Part 2 step 7).
//!
//! The constrained `.desktop` carries ONLY `Exec=/usr/bin/shrek-bench-run <bench> <key>` — two safe
//! tokens, never a command. That is the fixed-baked-key discipline (mirroring shrek-menu's baked provider
//! key): the command lives in the ROOT-OWNED bench record, not the dev-editable `.desktop`, so a forged or
//! tampered `.desktop` can pass only a key — which the supervisor refuses if it is not registered — and can
//! never smuggle a command onto the host.
//!
//! This wrapper is deliberately tiny and does ONE thing: forward exactly two positional tokens to the
//! privileged supervisor's `run-export` verb over gatekeeperd's authenticated socket (ADR-003 Part 2
//! authorization slice) — NOT via `sudo`. `run-export` is NEUTRAL (it resolves the key server-side against
//! the root-owned record and runs only a registered command), so it needs no consent ceremony. The socket
//! path is a compile-time constant (never PATH- or env-resolved, unshadowable), there is no privileged
//! fork/exec, and the daemon re-validates both tokens — strictly less attack surface than the old
//! `sudo -n gatekeeperd` escalation (no NOPASSWD dependency at all). Compiled, not a shell script, to avoid
//! `BASH_ENV`/`ENV`/`IFS` hazards fed from launcher-inherited env.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;

/// gatekeeperd's authenticated control socket, at its fixed default path (never env in the shipped build).
const SOCKET: &str = "/run/shrek-gk.sock";

/// %-escape space/`%`/control so each token is one newline-free line on the count-framed BENCH wire
/// (mirrors `gatekeeperd::bench_plane::pct_encode`; `<bench>`/`<key>` are charset-validated names so this
/// is belt-and-braces — an embedded newline can never smuggle a second request line).
fn pct_encode(s: &str) -> String {
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

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        eprintln!("usage: shrek-bench-run <bench> <key>  (the exported Bench .desktop launcher target)");
        return ExitCode::from(2);
    }
    let mut stream = match UnixStream::connect(SOCKET) {
        Ok(s) => s,
        Err(e) => {
            // Fail-closed: an unreachable supervisor is unavailability, never a launch. Report + exit nonzero.
            eprintln!("shrek-bench-run: gatekeeper socket unavailable ({SOCKET}): {e}");
            return ExitCode::from(1);
        }
    };
    // Count-framed request: `BENCH run-export 2` + the two pct-encoded tokens. The supervisor re-validates
    // both and resolves the key against the root-owned record; this wrapper adds no logic.
    let req = format!("BENCH run-export 2\n{}\n{}\n", pct_encode(&args[0]), pct_encode(&args[1]));
    if let Err(e) = stream.write_all(req.as_bytes()) {
        eprintln!("shrek-bench-run: write failed: {e}");
        return ExitCode::from(1);
    }
    let _ = stream.flush();
    let reader = BufReader::new(&stream);
    let mut rc: u8 = 1;
    let mut saw_end = false;
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => { eprintln!("shrek-bench-run: read failed: {e}"); return ExitCode::from(1); }
        };
        if let Some(tail) = line.strip_prefix("END ") {
            rc = tail.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(1);
            saw_end = true;
            break;
        }
    }
    if !saw_end {
        eprintln!("shrek-bench-run: no well-formed response from the supervisor");
        return ExitCode::from(1);
    }
    ExitCode::from(rc)
}
