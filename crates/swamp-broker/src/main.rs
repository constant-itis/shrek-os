//! swamp-broker — the in-sandbox SWAMP-QUERY broker (Phase-6 Swamp slice-2,
//! docs/phase6-swamp-slice2-broker-routed-find.md). A host-side forwarder that lets a T2 gVisor coding
//! session query the swamp WITHOUT a hole in its wall.
//!
//! Trust shape (amendment 4: this broker is TCB for session selection):
//!   1. A sandbox's `swamp_find` tool POSTs a plaintext query over the sealed `swamp-query` egress
//!      (tcp:8400), carrying its opaque handle in `X-Shrek-Session`.
//!   2. `getpeername()` (here `TcpStream::peer_addr`) yields the caller's `/30` `cont_ip` — un-forgeable
//!      (per-veth anti-spoof) and un-masqueraded (net_plane Mechanism-A carve-out preserves it to us).
//!   3. We look up gatekeeperd's root-owned `cont_ip→session` binding and FORWARD only if the caller's
//!      presented handle equals that bound session. The handle is opaque IDENTITY, not bearer authority:
//!      a leaked handle is worthless off its own wire, because a different sandbox's `cont_ip` maps to a
//!      different session and the check fails (fail-closed empty).
//!   4. On a match we connect swampd's `/run/swamp/query.sock` as an allowed uid and rebuild the swampd
//!      wire FROM SCRATCH with the AUTHENTICATED session — a client-supplied `session` line is never
//!      echoed (the anti-forgery core). swampd resolves authority independently from its root-owned
//!      handle-keyed record: routing changes reachability, NEVER authority.
//!
//! Fail-closed everywhere: bad transport, no binding, a stale binding, or a handle mismatch all return
//! `RESULT 0 / END` — byte-identical to a query that legitimately matched nothing, so a caller cannot
//! probe which sessions or objects exist. std-only line-text wire (swampd's idiom); the only in-tree
//! dep is `gatekeeperd`, reused so the binding + handle formats are single-sourced with the writer.
//!
//! Env:
//!   SHREK_SWAMP_BROKER_LISTEN   plaintext listen addr for the box     (default 127.0.0.1:8400)
//!   SHREK_SWAMP_QUERY_SOCK      swampd query unix socket               (default /run/swamp/query.sock)
//!   SHREK_NET_BINDING_DIR       cont_ip→session bindings dir           (via gatekeeperd::net_binding)

use gatekeeperd::authority_record::valid_session_id;
use gatekeeperd::net_binding;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

const DEFAULT_LISTEN: &str = "127.0.0.1:8400";
const DEFAULT_SOCK: &str = "/run/swamp/query.sock";
const SESSION_HEADER: &str = "x-shrek-session";
const MAX_BODY: usize = 16 * 1024;
const MAX_LIMIT: usize = 500;
const DEFAULT_LIMIT: usize = 50;
/// The fail-closed empty projection. The `RESULT 0` / `END` structural core is byte-identical to
/// swampd's zero-hit wire, so a denied caller still cannot distinguish denial from a legitimate empty
/// match (slice-2). It carries `freshness unknown` because on this path the broker never reached a
/// healthy swampd (deny, or swampd down) — the honest state. It also carries `semantic unavailable`
/// (slice-4): the broker reached no index, so no similarity ranking was applied. Both signals are
/// index-global (no per-session/per-object information), so neither is an existence oracle; and the
/// broker never REINTERPRETS a swampd-supplied header — a forwarded response is relayed verbatim.
const EMPTY_RESULT: &str = "RESULT 0\nfreshness unknown\nsemantic unavailable\nEND\n";

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let listen = env_or("SHREK_SWAMP_BROKER_LISTEN", DEFAULT_LISTEN);
    let sock = swampd_sock();
    let listener = match TcpListener::bind(&listen) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("SWAMP-BROKER-ERROR bind {listen}: {e}");
            return 2;
        }
    };
    println!("SWAMP-BROKER-LISTEN {listen} swampd_sock={} binding_dir={}", sock.display(), net_binding::binding_dir().display());
    for conn in listener.incoming() {
        match conn {
            Ok(mut stream) => {
                if let Err(e) = handle(&mut stream, &sock) {
                    eprintln!("SWAMP-BROKER-CONN error: {e}");
                }
            }
            Err(e) => eprintln!("SWAMP-BROKER-ACCEPT error: {e}"),
        }
    }
    0
}

/// Serve one connection. Authenticates by transport identity, then either forwards the query to swampd
/// (rebuilding the wire with the authenticated session) or returns the fail-closed empty projection.
fn handle(stream: &mut TcpStream, sock: &std::path::Path) -> std::io::Result<()> {
    // (2) Transport identity: the peer's /30 cont_ip, preserved by the masquerade carve-out. A non-IPv4
    // peer (e.g. a stray loopback v6) can carry no binding, so it fails closed.
    let cont_ip = match stream.peer_addr()?.ip() {
        IpAddr::V4(v4) => v4,
        IpAddr::V6(_) => return respond_empty(stream, "non-ipv4 peer"),
    };

    let (method, presented, body) = match read_request(stream) {
        Ok(t) => t,
        Err(e) => {
            // A malformed HTTP request is a client bug, not an auth probe — a plain error is fine and
            // does not leak session/object existence (nothing was looked up).
            let _ = write_plain(stream, 400, &format!("bad request: {e}"));
            return Ok(());
        }
    };
    dispatch(stream, sock, cont_ip, &method, presented.as_deref(), &body)
}

/// The authenticated forward decision + relay, split out so the identity core is unit-testable.
fn dispatch(
    stream: &mut TcpStream,
    sock: &std::path::Path,
    cont_ip: Ipv4Addr,
    method: &str,
    presented: Option<&str>,
    body: &[u8],
) -> std::io::Result<()> {
    if method != "POST" {
        return write_plain(stream, 405, "method not allowed");
    }
    // (3) Authorize by transport: the presented handle must equal the session bound to THIS cont_ip.
    let expected = net_binding::load_binding(&net_binding::binding_dir(), cont_ip);
    let Some(session) = authorize(expected.as_deref(), presented) else {
        // No binding, stale binding, absent handle, or a stolen handle from the wrong wire — all empty.
        eprintln!("SWAMP-BROKER-DENY cip={cont_ip} (no matching binding for presented handle)");
        return respond_empty(stream, "unauthorized");
    };
    // Parse the query fields; a client-supplied `session` in the body is ignored (never trusted).
    let Some(q) = parse_query_body(body) else {
        return write_plain(stream, 400, "malformed query body");
    };
    // (4) Forward: rebuild the swampd wire with the AUTHENTICATED session and relay swampd's projection.
    let wire = build_swampd_wire(&session, &q);
    let result = relay_to_swampd(sock, &wire);
    eprintln!("SWAMP-BROKER-FWD cip={cont_ip} session-bound intent={} limit={} (forwarded to swampd)", q.intent, q.limit);
    write_result(stream, &result)
}

/// The identity gate (fail-closed): forward IFF a binding exists for this cont_ip AND the presented
/// handle equals it AND it is a well-formed session id. Returns the session to forward under (the
/// authenticated one), or `None` to return empty. Pure — unit-tested for every fail branch.
fn authorize(expected: Option<&str>, presented: Option<&str>) -> Option<String> {
    let expected = expected?; // no binding for this cont_ip
    let presented = presented?; // caller sent no handle
    // Both must be valid session ids, and they must be equal. valid_session_id is reused from
    // gatekeeperd so the accepted charset matches exactly what gatekeeperd mints and binds.
    if !valid_session_id(expected) || !valid_session_id(presented) {
        return None;
    }
    if expected != presented {
        return None; // stolen handle presented from the wrong wire
    }
    Some(expected.to_string())
}

/// The swamp query fields the broker forwards. `session` is NEVER read from the client — it is injected
/// from the authenticated binding, so this struct deliberately has no session field.
struct Query {
    intent: String,
    scope: String,
    limit: usize,
    q: String,
}

/// Parse the plaintext line-text request body (swampd's idiom; no JSON dep). Recognized keys:
/// `intent search|discover` (default search), `scope <abs|->` (default `-`), `limit <n>` (clamped
/// 1..=500, default 50), `q <terms>` (REQUIRED). Any other key — including a smuggled `session` — is
/// ignored. Fail-closed: no `q`, an invalid intent, a non-numeric limit, or a control char in any value
/// returns `None` (the caller then answers 400). One key per line; the value is the line remainder.
fn parse_query_body(body: &[u8]) -> Option<Query> {
    let text = std::str::from_utf8(body).ok()?;
    let mut intent = String::from("search");
    let mut scope = String::from("-");
    let mut limit = DEFAULT_LIMIT;
    let mut q: Option<String> = None;
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        // A control char (incl an embedded CR) in a value would corrupt the swampd line wire — reject.
        if line.bytes().any(|b| b < 0x20 && b != b'\t') {
            return None;
        }
        let (k, v) = line.split_once(' ').unwrap_or((line, ""));
        match k {
            "intent" => {
                if v != "search" && v != "discover" {
                    return None;
                }
                intent = v.to_string();
            }
            "scope" => scope = if v.is_empty() { "-".to_string() } else { v.to_string() },
            "limit" => limit = v.parse::<usize>().ok()?.clamp(1, MAX_LIMIT),
            "q" => q = Some(v.to_string()),
            _ => {} // ignore unknown keys (forward-compat); a client `session` is never trusted here
        }
    }
    let q = q?;
    if q.trim().is_empty() {
        return None;
    }
    Some(Query { intent, scope, limit, q })
}

/// Build the swampd query wire (matches swampd `Request::read`) with the AUTHENTICATED session. This is
/// the anti-forgery core: the session line comes ONLY from the transport-authenticated binding, so a
/// caller can never present a body that makes swampd resolve a session it is not bound to.
fn build_swampd_wire(session: &str, q: &Query) -> String {
    format!(
        "QUERY 1\nsession {session}\nintent {}\nscope {}\nlimit {}\nq {}\nEND\n",
        q.intent, q.scope, q.limit, q.q
    )
}

/// Connect swampd's unix socket (as an allowed uid), send the wire, and return its response verbatim.
/// Any I/O failure (socket absent, swampd down, short write) fails closed to the empty projection —
/// availability, not authority: a query simply returns nothing.
fn relay_to_swampd(sock: &std::path::Path, wire: &str) -> String {
    match relay_inner(sock, wire) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("SWAMP-BROKER-SWAMPD unreachable ({}): {e} — fail-closed empty", sock.display());
            EMPTY_RESULT.to_string()
        }
    }
}

fn relay_inner(sock: &std::path::Path, wire: &str) -> std::io::Result<String> {
    let mut s = UnixStream::connect(sock)?;
    s.write_all(wire.as_bytes())?;
    s.flush()?;
    let mut out = String::new();
    s.read_to_string(&mut out)?;
    Ok(out)
}

// -------------------------------------------------------------------------------------------------
// HTTP transport (std-only, mirrors the model brokers)
// -------------------------------------------------------------------------------------------------

/// Read one HTTP request: returns `(method, X-Shrek-Session, body)`. Header names are case-insensitive;
/// the body is bounded by Content-Length (≤ MAX_BODY). We do not care about the path (single-purpose).
fn read_request(stream: &mut TcpStream) -> std::io::Result<(String, Option<String>, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        if let Some(p) = find_subslice(&buf, b"\r\n\r\n") {
            break p + 4;
        }
        if buf.len() > 64 * 1024 {
            return Err(ioerr("request headers too large"));
        }
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            return Err(ioerr("connection closed before headers complete"));
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let req_line = lines.next().unwrap_or("");
    let method = req_line.split_whitespace().next().unwrap_or("").to_string();
    if method.is_empty() {
        return Err(ioerr("malformed request line"));
    }
    let mut content_length = 0usize;
    let mut session: Option<String> = None;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim();
            if k.eq_ignore_ascii_case("content-length") {
                content_length = v.trim().parse().map_err(|_| ioerr("bad content-length"))?;
            } else if k.eq_ignore_ascii_case(SESSION_HEADER) {
                session = Some(v.trim().to_string());
            }
        }
    }
    if content_length > MAX_BODY {
        return Err(ioerr("request body exceeds cap"));
    }
    let mut body = buf[head_end..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            return Err(ioerr("connection closed before body complete"));
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);
    Ok((method, session, body))
}

/// Respond with swampd's projection (or the fail-closed empty one) as a plain-text body. The coder's
/// tool parses the same `RESULT n / hit path\tsnippet / END` wire swampd emits (and `shrek find` reads).
fn write_result(stream: &mut TcpStream, result: &str) -> std::io::Result<()> {
    let resp = format!(
        "HTTP/1.1 200 SWAMP\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{result}",
        result.len()
    );
    stream.write_all(resp.as_bytes())?;
    stream.flush().ok();
    Ok(())
}

/// Fail-closed empty projection with a 200 (indistinguishable from a legitimate zero-hit query — the
/// reason is logged server-side only, never sent to the caller).
fn respond_empty(stream: &mut TcpStream, _why: &str) -> std::io::Result<()> {
    write_result(stream, EMPTY_RESULT)
}

fn write_plain(stream: &mut TcpStream, code: u16, msg: &str) -> std::io::Result<()> {
    let body = format!("{code} {msg}\n");
    let resp = format!(
        "HTTP/1.1 {code} SWAMP\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(resp.as_bytes())?;
    stream.flush().ok();
    Ok(())
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn ioerr(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg)
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
}

fn swampd_sock() -> PathBuf {
    env_or("SHREK_SWAMP_QUERY_SOCK", DEFAULT_SOCK).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_forwards_only_on_exact_binding_match() {
        // Happy path: presented handle equals the bound session → forward under it.
        assert_eq!(authorize(Some("abc123"), Some("abc123")).as_deref(), Some("abc123"));
    }

    #[test]
    fn authorize_fails_closed_when_no_binding() {
        // B3: unbound source (no binding for this cont_ip) → refuse, even with a valid-looking handle.
        assert_eq!(authorize(None, Some("abc123")), None);
    }

    #[test]
    fn authorize_fails_closed_when_no_handle() {
        // A caller that presents no X-Shrek-Session cannot be forwarded even if a binding exists.
        assert_eq!(authorize(Some("abc123"), None), None);
    }

    #[test]
    fn authorize_fails_closed_on_stolen_handle_wrong_wire() {
        // B2: a *valid* handle presented from a DIFFERENT sandbox's wire — its cont_ip is bound to
        // another session, so expected != presented → empty. This is the whole point of the binding.
        assert_eq!(authorize(Some("sessionB"), Some("sessionA")), None);
    }

    #[test]
    fn authorize_rejects_malformed_session_ids() {
        // A binding or handle that is not a valid session id can never authorize (defensive; the writer
        // already validates, but the broker must not trust a corrupt record or a crafted header).
        assert_eq!(authorize(Some("../escape"), Some("../escape")), None);
        assert_eq!(authorize(Some(""), Some("")), None);
        assert_eq!(authorize(Some("ok"), Some("bad/slash")), None);
    }

    #[test]
    fn parse_body_defaults_and_requires_q() {
        let q = parse_query_body(b"q needle").expect("q alone is valid");
        assert_eq!(q.q, "needle");
        assert_eq!(q.intent, "search");
        assert_eq!(q.scope, "-");
        assert_eq!(q.limit, DEFAULT_LIMIT);
        // No q → fail closed.
        assert!(parse_query_body(b"intent search\nlimit 10").is_none());
        assert!(parse_query_body(b"q    ").is_none(), "blank q is rejected");
    }

    #[test]
    fn parse_body_reads_all_fields_and_clamps_limit() {
        let q = parse_query_body(b"intent discover\nscope /srv/project\nlimit 9999\nq foo bar baz").unwrap();
        assert_eq!(q.intent, "discover");
        assert_eq!(q.scope, "/srv/project");
        assert_eq!(q.limit, MAX_LIMIT, "limit clamps to MAX_LIMIT");
        assert_eq!(q.q, "foo bar baz", "q value is the whole line remainder (spaces preserved)");
    }

    #[test]
    fn parse_body_rejects_bad_intent_and_control_chars() {
        assert!(parse_query_body(b"intent delete\nq x").is_none(), "unknown intent rejected");
        // A control byte that is NOT a line terminator (here a bare CR / BEL mid-value) is rejected — it
        // would otherwise be carried into swampd's line wire. (`\n` can never appear in a value: lines()
        // has already split on it, which is what structurally prevents wire-line injection.)
        assert!(parse_query_body(b"q ok\revil").is_none(), "embedded lone CR is rejected");
        assert!(parse_query_body(b"q ok\x07evil").is_none(), "embedded BEL control char is rejected");
        // `\r\n` is a normal line ending (lines() strips the CR): the body below is two clean lines.
        assert_eq!(parse_query_body(b"intent search\r\nq needle").unwrap().q, "needle");
    }

    #[test]
    fn parse_body_ignores_a_smuggled_session_line() {
        // A client cannot inject its own session — the parser drops it, and build_swampd_wire only ever
        // uses the authenticated one. Guards the anti-forgery core end-to-end at the parse boundary.
        let q = parse_query_body(b"session EVIL\nq needle").expect("unknown keys are ignored");
        let wire = build_swampd_wire("REAL", &q);
        assert!(wire.contains("session REAL\n"), "wire carries only the authenticated session: {wire}");
        assert!(!wire.contains("EVIL"), "a client-supplied session must never reach swampd: {wire}");
    }

    #[test]
    fn build_wire_matches_swampd_request_grammar() {
        // The exact grammar swampd's Request::read parses: QUERY 1 / session / intent / scope / limit /
        // q / END, one field per line, terminated by END.
        let q = Query { intent: "search".into(), scope: "-".into(), limit: 50, q: "alpha beta".into() };
        let wire = build_swampd_wire("s1", &q);
        assert_eq!(wire, "QUERY 1\nsession s1\nintent search\nscope -\nlimit 50\nq alpha beta\nEND\n");
    }

    #[test]
    fn empty_result_preserves_zero_hit_core_and_marks_freshness_unknown() {
        // The RESULT/END core stays byte-identical to swampd's zero-hit wire (denied ≡ empty match), and
        // the fail-closed path honestly reports freshness=unknown + semantic=unavailable (the broker
        // reached no healthy index, so nothing was ranked). Both are index-global, not existence oracles.
        assert_eq!(EMPTY_RESULT, "RESULT 0\nfreshness unknown\nsemantic unavailable\nEND\n");
        assert!(EMPTY_RESULT.starts_with("RESULT 0\n"));
        assert!(EMPTY_RESULT.trim_end().ends_with("END"));
        assert!(EMPTY_RESULT.contains("freshness unknown"));
        assert!(EMPTY_RESULT.contains("semantic unavailable"));
    }
}
