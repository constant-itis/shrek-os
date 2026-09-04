//! client — the uid-1000 socket FRONT DOOR (ADR-007 S3).
//!
//! `egressd ask <status|bless|unbless|repin> [profile]` is what the desktop UI (DMS Connectivity panel +
//! first-run onboarding) execs — with a FIXED argv, never a shell — to drive a bless over the supervisor
//! socket. It runs UNPRIVILEGED as uid 1000: opening `/run/shrek/egress/sock` and writing one request
//! line needs no capability. It adds convenience, not capability — any uid-1000 process could already
//! open that socket; the DAEMON is the sole authority (uid gate, sealed Tier-B gate, verb allowlist,
//! third-field rejection, rate limit). So the client is a dumb, single-shot pipe:
//!
//!   * It builds the wire line from argv, but locally rejects whitespace/control/oversize tokens first —
//!     better errors, and it never even opens the socket for obvious garbage (the daemon re-validates
//!     regardless: a smuggled space becomes a third field → hard parse error server-side).
//!   * It uses [`store::run_dir`] / [`supervisor::socket_path`], whose `SHREK_EGRESS_*` overrides are
//!     compiled OUT of the shipped (non-`oracle-env`) build — so nothing in uid 1000's environment can
//!     redirect the client at a fake daemon in the sealed image `[Fable S3 fix #8]`.
//!   * DISPLAY TRUTH IS NEVER the `OK` reply — the UI reads the root-written `/run/shrek/egress/state`
//!     projection. `ask` only reports the immediate outcome as an exit code (`OK`→0, `ERR`→1,
//!     unreachable/io→2) so a caller can branch, e.g. the onboarding "enable later in Settings" path.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use crate::store::run_dir;
use crate::supervisor::socket_path;

/// One request/reply round-trip should be near-instant on a local socket; the daemon's own read timeout
/// is 5s, so 6s here fails closed a hair after it rather than hanging the UI.
const IO_TIMEOUT: Duration = Duration::from_secs(6);
/// Matches the daemon's `REQ_MAX`; a verb + a ≤64-char token + newline fits comfortably.
const LINE_MAX: usize = 128;

/// `egressd ask <verb> [profile]`. Returns a process exit code: 0 = daemon replied `OK`, 1 = daemon
/// replied `ERR ...` (denied / rate-limited / resolve-failed — the bless intent may still have persisted;
/// the panel's state view is authoritative), 2 = bad usage or the supervisor is unreachable.
pub fn ask(args: &[String]) -> i32 {
    let verb = match args.first() {
        Some(v) => v.as_str(),
        None => {
            eprintln!("egressd ask: usage: egressd ask <status|bless|unbless|repin|browser-up> [profile]");
            return 2;
        }
    };
    let line = match build_line(verb, args.get(1).map(String::as_str)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("egressd ask: {e}");
            return 2;
        }
    };

    let sock = socket_path(&run_dir());
    let mut stream = match UnixStream::connect(&sock) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("egressd ask: supervisor unreachable at {}: {e}", sock.display());
            return 2;
        }
    };
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    if let Err(e) = stream.write_all(line.as_bytes()) {
        eprintln!("egressd ask: write failed: {e}");
        return 2;
    }

    // Read exactly one reply line (the daemon writes one `\n`-terminated line then may close).
    let mut buf = Vec::with_capacity(256);
    let mut chunk = [0u8; 256];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.contains(&b'\n') || buf.len() > 512 {
                    break;
                }
            }
            Err(e) => {
                eprintln!("egressd ask: read failed: {e}");
                return 2;
            }
        }
    }
    let reply = String::from_utf8_lossy(&buf);
    let reply = reply.trim_end_matches(['\r', '\n']);
    if reply.is_empty() {
        eprintln!("egressd ask: empty reply from supervisor");
        return 2;
    }
    println!("{reply}");
    if reply.starts_with("OK") {
        0
    } else {
        1
    }
}

/// Build the one wire line, mirroring the daemon's allowlist for friendly local errors. The daemon is
/// still authoritative — this only spares an obviously-bad request the round-trip and gives a clear
/// message. Rejects a whitespace/control/oversize profile so a smuggled second token can't even be typed.
fn build_line(verb: &str, profile: Option<&str>) -> Result<String, String> {
    match verb {
        "status" => {
            if profile.is_some() {
                return Err("`status` takes no profile".into());
            }
            Ok("status\n".into())
        }
        // valueless verb — the browser launcher fires it AFTER the scope joins shrekbrowser.slice, so the
        // supervisor installs the cgroup accept-pair (no-op unless web-browsing is already blessed). No
        // argument: the daemon derives the cgroup path from the peer uid, never from the wire (a supplied
        // path would be a spoof of "which cgroup is the browser").
        "browser-up" => {
            if profile.is_some() {
                return Err("`browser-up` takes no argument".into());
            }
            Ok("browser-up\n".into())
        }
        "bless" | "unbless" | "repin" => {
            let p = profile.ok_or_else(|| format!("`{verb}` needs a profile (e.g. `egressd ask {verb} weather`)"))?;
            if !valid_client_token(p) {
                return Err(format!("invalid profile token {p:?} (alnum . _ - only, ≤64 chars)"));
            }
            let line = format!("{verb} {p}\n");
            if line.len() > LINE_MAX {
                return Err("request too large".into());
            }
            Ok(line)
        }
        other => Err(format!("unknown verb `{other}` (want status|bless|unbless|repin|browser-up)")),
    }
}

/// The same token shape the daemon's `store::valid_token` accepts — no whitespace, no control bytes, no
/// path separators, bounded length. Local guard only; the daemon re-checks.
fn valid_client_token(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 64
        && p != "."
        && p != ".."
        && !p.starts_with('.')
        && p.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_line_accepts_the_allowed_verbs() {
        assert_eq!(build_line("status", None).unwrap(), "status\n");
        assert_eq!(build_line("bless", Some("weather")).unwrap(), "bless weather\n");
        assert_eq!(build_line("unbless", Some("weather")).unwrap(), "unbless weather\n");
        assert_eq!(build_line("repin", Some("weather")).unwrap(), "repin weather\n");
        assert_eq!(build_line("browser-up", None).unwrap(), "browser-up\n");
    }

    #[test]
    fn build_line_browser_up_is_valueless() {
        // mirrors the daemon: browser-up carries NO argument (the cgroup is derived from the peer uid).
        assert!(build_line("browser-up", Some("web-browsing")).is_err());
        assert!(build_line("browser-up", Some("anything")).is_err());
    }

    #[test]
    fn build_line_rejects_abuse_locally() {
        assert!(build_line("status", Some("weather")).is_err()); // status takes no arg
        assert!(build_line("bless", None).is_err()); // missing profile
        assert!(build_line("bogus", Some("weather")).is_err()); // unknown verb
        // a smuggled destination / second token can't be encoded — the space is rejected as a bad token
        assert!(build_line("bless", Some("weather evil.example")).is_err());
        assert!(build_line("bless", Some("6.6.6.6 extra")).is_err());
        assert!(build_line("bless", Some("../escape")).is_err()); // traversal
        assert!(build_line("bless", Some("we\nather")).is_err()); // embedded newline
        assert!(build_line("bless", Some("we\0ather")).is_err()); // control byte
        assert!(build_line("bless", Some(&"x".repeat(200))).is_err()); // oversize
    }
}
