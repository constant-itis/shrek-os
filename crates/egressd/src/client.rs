//! client — the uid-1000 socket FRONT DOOR (ADR-007 S3).
//!
//! `egressd ask <status|bless|unbless|repin> [profile]` is what the desktop UI (DMS Connectivity panel +
//! first-run onboarding) execs — with a FIXED argv, never a shell — to drive a bless over the supervisor
//! socket. It runs UNPRIVILEGED as uid 1000: opening `/run/shrek/egress/sock` and writing one request
//! line needs no capability. It adds convenience, not capability — any uid-1000 process could already
//! open that socket; the DAEMON is the sole authority (uid gate, sealed Tier-B gate, verb allowlist,
//! third-field rejection, rate limit). So the client is a dumb, single-shot pipe.
//!
//! `ask` ALSO carries the ROOT-only `confirmed-*` ceremony-commit relay (ADR-007 S6 fix #4): gatekeeperd,
//! after a confirmed console SAK/VT ceremony, execs `egressd ask confirmed-<verb> <arg>` — a CAPLESS
//! socket client — instead of a CLI that mutates nft. The daemon (the sole nft mutator, already holding
//! `CAP_NET_ADMIN`) commits it, and authorizes it on the ROOT peer uid, so gatekeeperd never needs a
//! network capability and no transient process edits the ROOT-netns table under the broker's caps. Same
//! dumb-pipe shape; the only difference is which verbs `build_line` will encode:
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
/// Matches the daemon's `REQ_MAX` (the uid-1000 verbs); a verb + a ≤64-char token + newline fits.
const LINE_MAX: usize = 128;
/// Matches the daemon's `REQ_MAX_PRIV` — the ROOT ceremony-commit path, where the arg is a raw
/// `host:proto:port` (bounded at 300 by the sealed grammar) rather than a short profile token.
const LINE_MAX_PRIV: usize = 384;

/// `egressd ask <verb> [profile]`. Returns a process exit code: 0 = daemon replied `OK`, 1 = daemon
/// replied `ERR ...` (denied / rate-limited / resolve-failed — the bless intent may still have persisted;
/// the panel's state view is authoritative), 2 = bad usage or the supervisor is unreachable.
pub fn ask(args: &[String]) -> i32 {
    let verb = match args.first() {
        Some(v) => v.as_str(),
        None => {
            eprintln!("egressd ask: usage: egressd ask <status|bless|unbless|repin|browser-up|want> [token]");
            eprintln!("             egressd ask bind <provider> <ipv4>   |   egressd ask unbind <provider>");
            eprintln!("             (ROOT only) egressd ask confirmed-<bless|unbless|add-raw|remove-raw> <arg>");
            eprintln!("             (ROOT only) egressd ask confirmed-manifest-<install|remove> <capability-name>");
            return 2;
        }
    };
    let line = match build_line(verb, args.get(1).map(String::as_str), args.get(2).map(String::as_str)) {
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
fn build_line(verb: &str, a: Option<&str>, b: Option<&str>) -> Result<String, String> {
    // `bind` is the sole verb with a second argument (the address). Reject a stray second arg on every
    // other verb up front, mirroring the daemon's grammar (ADR-008 [R1-MF8b]).
    if verb != "bind" && b.is_some() {
        return Err(format!("`{verb}` takes at most one argument"));
    }
    let profile = a;
    match verb {
        "status" => {
            if profile.is_some() {
                return Err("`status` takes no profile".into());
            }
            Ok("status\n".into())
        }
        // ADR-008 hosts verbs. `bind <provider> <ipv4>` carries a second field; `unbind <provider>` one.
        // The address is validated by the SAME sealed grammar the daemon uses (strict IPv4 literal) —
        // shrek-policy is the single source of truth — so an obvious mistake is caught before the socket.
        "bind" => {
            let p = a.ok_or("`bind` needs a provider (local|anthropic|claude|codex)")?;
            let addr = b.ok_or("`bind` needs an address (e.g. `egressd ask bind local 192.168.1.152`)")?;
            if !valid_client_token(p) {
                return Err(format!("invalid provider token {p:?}"));
            }
            if shrek_policy::provider_bind::valid_bind_addr(addr).is_none() {
                return Err(format!("address {addr:?} must be an IPv4 literal (e.g. 192.168.1.152)"));
            }
            let line = format!("bind {p} {addr}\n");
            if line.len() > LINE_MAX {
                return Err("request too large".into());
            }
            Ok(line)
        }
        "unbind" => {
            let p = a.ok_or("`unbind` needs a provider (local|anthropic|claude|codex)")?;
            if !valid_client_token(p) {
                return Err(format!("invalid provider token {p:?}"));
            }
            let line = format!("unbind {p}\n");
            if line.len() > LINE_MAX {
                return Err("request too large".into());
            }
            Ok(line)
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
        // ADR-009 uid-1000 capability request: a single CLOSED catalog token (the daemon refuses a token
        // that names no capability). Discoverability with zero authority.
        "want" => {
            let p = profile.ok_or("`want` needs a capability token (e.g. `egressd ask want weather`)")?;
            if !valid_client_token(p) {
                return Err(format!("invalid capability token {p:?} (alnum . _ - only, ≤64 chars)"));
            }
            let line = format!("want {p}\n");
            if line.len() > LINE_MAX {
                return Err("request too large".into());
            }
            Ok(line)
        }
        // ADR-009 ROOT-only owner-manifest ceremony relay (gatekeeperd post-ceremony). Carries only the
        // capability NAME token; gatekeeperd staged the confirmed bytes to the staging dir first. The
        // daemon re-parses + §4.4-validates the staged candidate and authorizes on the ROOT peer uid.
        "confirmed-manifest-install" | "confirmed-manifest-remove" => {
            let p = profile.ok_or_else(|| format!("`{verb}` needs a capability name"))?;
            if !valid_client_token(p) {
                return Err(format!("invalid capability name {p:?} (alnum . _ - only, ≤64 chars)"));
            }
            let line = format!("{verb} {p}\n");
            if line.len() > LINE_MAX {
                return Err("request too large".into());
            }
            Ok(line)
        }
        // ROOT-only ceremony-commit relay (gatekeeperd post-ceremony, or the S6 probe's setup). The daemon
        // authorizes on the ROOT peer uid; a uid-1000 process sending these is refused server-side. The
        // profile variants carry a token; the raw variants carry a `host:proto:port`, validated +
        // canonicalized through THE one sealed grammar (the daemon re-validates regardless).
        "confirmed-bless" | "confirmed-unbless" => {
            let p = profile.ok_or_else(|| format!("`{verb}` needs a profile (e.g. `egressd ask {verb} web-browsing`)"))?;
            if !valid_client_token(p) {
                return Err(format!("invalid profile token {p:?} (alnum . _ - only, ≤64 chars)"));
            }
            let line = format!("{verb} {p}\n");
            if line.len() > LINE_MAX {
                return Err("request too large".into());
            }
            Ok(line)
        }
        "confirmed-add-raw" | "confirmed-remove-raw" => {
            let p = profile.ok_or_else(|| format!("`{verb}` needs a host:proto:port destination"))?;
            let t = shrek_policy::desktop_egress::parse_raw_triple(p)
                .map_err(|e| format!("invalid destination {p:?}: {e}"))?;
            let line = format!("{verb} {}\n", t.to_wire());
            if line.len() > LINE_MAX_PRIV {
                return Err("request too large".into());
            }
            Ok(line)
        }
        other => Err(format!(
            "unknown verb `{other}` (want status|bless|unbless|repin|browser-up|want|confirmed-*)"
        )),
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
        assert_eq!(build_line("status", None, None).unwrap(), "status\n");
        assert_eq!(build_line("bless", Some("weather"), None).unwrap(), "bless weather\n");
        assert_eq!(build_line("unbless", Some("weather"), None).unwrap(), "unbless weather\n");
        assert_eq!(build_line("repin", Some("weather"), None).unwrap(), "repin weather\n");
        assert_eq!(build_line("browser-up", None, None).unwrap(), "browser-up\n");
    }

    #[test]
    fn build_line_encodes_the_hosts_verbs() {
        // ADR-008: bind carries provider + IPv4; unbind carries provider only.
        assert_eq!(build_line("bind", Some("local"), Some("192.168.1.152")).unwrap(), "bind local 192.168.1.152\n");
        assert_eq!(build_line("unbind", Some("anthropic"), None).unwrap(), "unbind anthropic\n");
        // a non-IPv4 address is refused before the socket is opened (the daemon re-validates too).
        assert!(build_line("bind", Some("local"), Some("myhost.lan")).is_err());
        assert!(build_line("bind", Some("local"), Some("0x7f000001")).is_err());
        assert!(build_line("bind", Some("local"), Some("::1")).is_err());
        // missing pieces
        assert!(build_line("bind", Some("local"), None).is_err()); // no address
        assert!(build_line("bind", None, None).is_err()); // no provider
        assert!(build_line("unbind", None, None).is_err()); // no provider
        // a smuggled token can't be encoded (space rejected as a bad token).
        assert!(build_line("bind", Some("local evil"), Some("1.2.3.4")).is_err());
    }

    #[test]
    fn build_line_browser_up_is_valueless() {
        // mirrors the daemon: browser-up carries NO argument (the cgroup is derived from the peer uid).
        assert!(build_line("browser-up", Some("web-browsing"), None).is_err());
        assert!(build_line("browser-up", Some("anything"), None).is_err());
    }

    #[test]
    fn build_line_encodes_the_confirmed_relay_verbs() {
        // profile-arg ceremony verbs
        assert_eq!(build_line("confirmed-bless", Some("web-browsing"), None).unwrap(), "confirmed-bless web-browsing\n");
        assert_eq!(build_line("confirmed-unbless", Some("web-browsing"), None).unwrap(), "confirmed-unbless web-browsing\n");
        // raw-triple ceremony verbs: validated + canonicalized through the sealed grammar
        assert_eq!(build_line("confirmed-add-raw", Some("example.com:tcp:8443"), None).unwrap(), "confirmed-add-raw example.com:tcp:8443\n");
        assert_eq!(build_line("confirmed-remove-raw", Some("203.0.113.7:udp:8883"), None).unwrap(), "confirmed-remove-raw 203.0.113.7:udp:8883\n");
        // a malformed destination is refused before the socket is even opened (the daemon re-checks too).
        assert!(build_line("confirmed-add-raw", Some("singlelabel:tcp:443"), None).is_err());
        assert!(build_line("confirmed-add-raw", Some("e.com:icmp:0"), None).is_err());
        assert!(build_line("confirmed-add-raw", None, None).is_err());
        assert!(build_line("confirmed-bless", None, None).is_err());
    }

    #[test]
    fn build_line_encodes_want_and_manifest_verbs() {
        // uid-1000 capability request: a single token.
        assert_eq!(build_line("want", Some("weather"), None).unwrap(), "want weather\n");
        assert!(build_line("want", None, None).is_err()); // needs a token
        assert!(build_line("want", Some("../escape"), None).is_err()); // traversal
        assert!(build_line("want", Some("weather"), Some("extra")).is_err()); // stray 2nd arg
        // ROOT-only owner-manifest relay: a single capability NAME token.
        assert_eq!(
            build_line("confirmed-manifest-install", Some("radar"), None).unwrap(),
            "confirmed-manifest-install radar\n"
        );
        assert_eq!(
            build_line("confirmed-manifest-remove", Some("radar"), None).unwrap(),
            "confirmed-manifest-remove radar\n"
        );
        assert!(build_line("confirmed-manifest-install", None, None).is_err());
        assert!(build_line("confirmed-manifest-install", Some("bad name"), None).is_err());
    }

    #[test]
    fn build_line_rejects_abuse_locally() {
        assert!(build_line("status", Some("weather"), None).is_err()); // status takes no arg
        assert!(build_line("bless", None, None).is_err()); // missing profile
        assert!(build_line("bogus", Some("weather"), None).is_err()); // unknown verb
        // a stray second argument on a single-arg verb is rejected up front.
        assert!(build_line("bless", Some("weather"), Some("evil")).is_err());
        // a smuggled destination / second token can't be encoded — the space is rejected as a bad token
        assert!(build_line("bless", Some("weather evil.example"), None).is_err());
        assert!(build_line("bless", Some("6.6.6.6 extra"), None).is_err());
        assert!(build_line("bless", Some("../escape"), None).is_err()); // traversal
        assert!(build_line("bless", Some("we\nather"), None).is_err()); // embedded newline
        assert!(build_line("bless", Some("we\0ather"), None).is_err()); // control byte
        assert!(build_line("bless", Some(&"x".repeat(200)), None).is_err()); // oversize
    }
}
