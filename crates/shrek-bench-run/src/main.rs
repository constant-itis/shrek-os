//! shrek-bench-run — the fixed launcher target of an exported Bench `.desktop` (ADR-003 Part 2 step 7).
//!
//! The constrained `.desktop` carries ONLY `Exec=/usr/bin/shrek-bench-run <bench> <key>` — two safe
//! tokens, never a command. That is the fixed-baked-key discipline (mirroring shrek-menu's baked provider
//! key): the command lives in the ROOT-OWNED bench record, not the dev-editable `.desktop`, so a forged or
//! tampered `.desktop` can pass only a key — which the supervisor refuses if it is not registered — and can
//! never smuggle a command onto the host.
//!
//! This wrapper is deliberately tiny and does ONE thing: forward exactly two positional tokens to the
//! privileged supervisor's `run-export` verb. It:
//!   * accepts EXACTLY two args and forwards NOTHING else (extra args ⇒ refused);
//!   * CLEARS the environment before escalating, so no `SHREK_BENCH_*` / `SHREK_GATEKEEPERD` from the dev
//!     session can influence the privileged call (defence in depth atop the shipped build compiling those
//!     overrides out, and sudo's own `env_reset`);
//!   * execs `gatekeeperd` at its ABSOLUTE path via `sudo -n` (dev has NOPASSWD) — never a PATH-resolved
//!     name (unshadowable), and never depending on profile.d env being present in a `.desktop` launch.
//! Compiled, not a shell script, to avoid `BASH_ENV`/`ENV`/`IFS` hazards fed from launcher-inherited env.

use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode};

/// The privileged Bench supervisor, at its fixed sealed path (never PATH-resolved).
const GATEKEEPERD: &str = "/usr/libexec/shrek/gatekeeperd";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        eprintln!("usage: shrek-bench-run <bench> <key>  (the exported Bench .desktop launcher target)");
        return ExitCode::from(2);
    }
    // env_clear(): the privileged call inherits NONE of the launcher/session environment. The supervisor
    // re-validates both tokens and resolves the key against the root-owned record; this wrapper adds no logic.
    let err = Command::new("/usr/bin/sudo")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .arg("-n")
        .arg(GATEKEEPERD)
        .arg("bench")
        .arg("run-export")
        .arg(&args[0])
        .arg(&args[1])
        .exec();
    // exec() only returns on failure.
    eprintln!("shrek-bench-run: cannot escalate to the Bench supervisor: {err}");
    ExitCode::from(127)
}
