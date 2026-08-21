//! claude-broker — the broker-side SUBSCRIPTION-model provider for the coder (Phase-6 slice-4,
//! docs/phase6-slice4-claude-cli-broker.md, security-model.md §7).
//!
//! WHY IT EXISTS. "Sign in with Claude" for Shrek means: log into the official `claude` CLI ONCE, then
//! Shrek invokes the logged-in CLI. The CLI owns its own subscription credential — Shrek never sees,
//! stores, or manages the OAuth token. This broker is the seam that lets the sandboxed coder drive that
//! CLI without any of it entering the box.
//!
//! HOW IT FITS THE EXISTING DESIGN — zero change to the coder or to the api-key path. The coder speaks
//! the SAME plaintext messages-API wire it already speaks to the api-key proxy (crates/model-proxy),
//! but over the sealed one-destination `model-claude-cli` egress instead of `model-anthropic`. This
//! broker ACCEPTS that messages-API request, TRANSLATES it into an invocation of `claude -p
//! --output-format json` broker-side, and WRAPS the reply back into the messages-API shape the coder's
//! extractor already parses. The box holds no secret, speaks no TLS, and reaches only this broker — the
//! lethal-trifecta break is identical to slice-3 (box: untrusted-read + egress, NEVER the secret).
//!
//! WHAT IT IS NOT. Not a control plane, not sealed into the appliance image (it shells the host's
//! `claude`), not a credential handler. It reads no token and never calls `claude auth status` (which
//! lies — #1567): the ONLY authority on token health is a real `claude -p` round-trip, whose error IS
//! surfaced fail-closed here.
//!
//! ARGV SAFETY. The CLI is invoked with a std::process argv VECTOR, never a shell string. The caller's
//! model id is mapped through a fixed broker-side ALLOWLIST to a `&'static` value — a raw caller string
//! never becomes a command argument. The prompt/system text are single argv elements (data, not flags).
//!
//! Config (all env; broker-side):
//!   SHREK_CLAUDE_BROKER_LISTEN   plaintext listen addr for the box   (default 127.0.0.1:8300)
//!   SHREK_CLAUDE_BIN             path/name of the `claude` binary     (default `claude`)
//!   SHREK_CLAUDE_DEFAULT_MODEL   model alias when the request's id is absent/unmapped (default `sonnet`)
//!   SHREK_CLAUDE_STATE_DIR       broker-side dir for the availability breadcrumb
//!                                (default $HOME/.local/state/shrek-claude-cli)
//!
//! Subcommands (Phase-6 slice-5, docs/phase6-slice5-claude-login-ux.md):
//!   (none) | serve   accept box requests and translate them to `claude -p` — the slice-4 behavior.
//!   login            run the official `claude auth login --claudeai` in the OPERATOR'S REAL TERMINAL
//!                    (the CLI owns ALL credential state), then verify with ONE real `claude -p`
//!                    round-trip and record an AUDIT-ONLY availability breadcrumb. No token is ever
//!                    read, parsed, or stored — login completion is observed as a STATE TRANSITION.
//!   health           run just the `claude -p` round-trip probe and update the breadcrumb.
//!
//! THE BREADCRUMB IS AUDIT-ONLY, NEVER AUTHORITY. Every real box request still round-trips live and
//! fails closed on the true error (#1567: cached state lies). Its `reason` is a FIXED enum — never raw
//! CLI output — so the breadcrumb can never become an accidental credential/logging surface.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{IsTerminal, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tinyjson::JsonValue;

const DEFAULT_LISTEN: &str = "127.0.0.1:8300";
const DEFAULT_CLAUDE_BIN: &str = "claude";
const DEFAULT_MODEL: &str = "sonnet";
/// Guard: refuse to read an absurdly large request so a broken peer cannot OOM the broker.
const MAX_BODY: usize = 16 * 1024 * 1024;
/// The one request header that carries the coder's opaque per-session handle (Phase-6 slice-7).
const SESSION_HEADER: &str = "x-shrek-session";
/// Upper bound on the untrusted handle length (defense for the map key; the handle NEVER reaches a
/// path or argv regardless — the broker-owned `internal_id` does, see [`Registry`]).
const MAX_HANDLE_LEN: usize = 128;

// ---- session registry (Phase-6 slice-7, docs/phase6-slice7-cross-provider-session.md) --------------
//
// The coder attaches an opaque `X-Shrek-Session` handle per session. This registry maps that handle to a
// broker-owned session slot so the broker can drive a REAL `claude` session (`--session-id` then
// `--resume`) and forward only the new tail turn each step, instead of re-flattening the whole transcript.
//
// INVARIANTS (owner-locked): (1) session identity is the handle, NEVER derived from transcript bytes, and
// carries no filesystem/network authority; (2) the raw handle NEVER reaches a path or argv — a
// broker-minted `internal_id` (which is also the `--session-id`) does; (3) requests bearing the SAME
// handle serialize on a per-session lock, different handles run concurrently; (4) if native resume ever
// fails, the caller falls back to the stateless full-transcript flatten, which is always correct.

/// A single broker-owned session slot. Its `state` mutex is the per-session serialization lock.
struct Session {
    state: Mutex<SessionState>,
}

struct SessionState {
    /// Broker-minted, opaque, and ALSO the `claude --session-id` value. Never the caller's handle.
    internal_id: String,
    /// How many transcript messages have already been forwarded into the native session.
    msgs_forwarded: usize,
}

/// Handle → session slot. The outer mutex guards only the map; the actual CLI call is serialized by the
/// per-slot `state` lock so a slow turn never blocks unrelated sessions.
struct Registry {
    map: Mutex<HashMap<String, Arc<Session>>>,
}

impl Registry {
    fn new() -> Self {
        Registry { map: Mutex::new(HashMap::new()) }
    }

    /// Get-or-create the slot for `handle`, minting a fresh broker-owned `internal_id` on first sighting.
    fn slot(&self, handle: &str) -> Arc<Session> {
        let mut m = self.map.lock().unwrap_or_else(|e| e.into_inner());
        m.entry(handle.to_string())
            .or_insert_with(|| {
                Arc::new(Session {
                    state: Mutex::new(SessionState { internal_id: mint_internal_id(), msgs_forwarded: 0 }),
                })
            })
            .clone()
    }

    /// Forget a handle so the next request with it re-establishes a fresh native session (used on any
    /// divergence/resume-failure → the caller takes the flatten fallback for the current call).
    fn forget(&self, handle: &str) {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).remove(handle);
    }
}

/// Validate the untrusted `X-Shrek-Session` handle: bounded length, safe charset. `None` ⇒ treat the
/// request as stateless (the slice-4 path). This is defense for the map key + logs; the handle is never
/// placed in a path/argv regardless.
fn valid_handle(h: &str) -> Option<String> {
    let h = h.trim();
    if h.is_empty() || h.len() > MAX_HANDLE_LEN {
        return None;
    }
    if h.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        Some(h.to_string())
    } else {
        None
    }
}

/// Mint a broker-owned opaque id (UUIDv4 shape) from `/dev/urandom`. Used as BOTH the internal session id
/// and the `claude --session-id` value — a valid UUID the CLI accepts, never the caller's handle.
fn mint_internal_id() -> String {
    let mut b = [0u8; 16];
    fill_random(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // variant 1
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    )
}

/// Fill `buf` with random bytes from `/dev/urandom`; on the (Linux-broker-unreachable) failure path,
/// derive a non-repeating fallback from the clock so an id is still produced.
fn fill_random(buf: &mut [u8]) {
    if let Ok(mut f) = File::open("/dev/urandom") {
        if f.read_exact(buf).is_ok() {
            return;
        }
    }
    let t = now_epoch();
    for (i, x) in buf.iter_mut().enumerate() {
        *x = (t >> ((i % 8) * 8)) as u8 ^ (i as u8).wrapping_mul(31);
    }
}

/// Render a slice of `(role, content)` messages to the role-labelled transcript a single `claude -p`
/// call reproduces. The stateless path flattens ALL messages; a sessioned continuation flattens only the
/// new tail `messages[msgs_forwarded..]`.
fn flatten(messages: &[(String, String)]) -> String {
    let mut prompt = String::new();
    for (role, content) in messages {
        let label = if role == "assistant" { "Assistant" } else { "User" };
        if !prompt.is_empty() {
            prompt.push_str("\n\n");
        }
        prompt.push_str(label);
        prompt.push_str(": ");
        prompt.push_str(content);
    }
    prompt
}

/// How the native session id is threaded onto a `claude -p` call.
enum Sess<'a> {
    /// Stateless single-shot (slice-4): no session id; system prompt sent inline.
    None,
    /// First turn of a session: `--session-id <internal_id>` + the system prompt.
    Create(&'a str),
    /// Continuation: `--resume <internal_id>`; the session already holds the system prompt.
    Resume(&'a str),
}

fn main() {
    std::process::exit(dispatch());
}

/// Subcommand dispatch. No args (or `serve`) preserves the slice-4 behavior verbatim; `login`/`health`
/// are the slice-5 operator commands. All three read the same broker-side env config.
fn dispatch() -> i32 {
    let claude_bin = env_or("SHREK_CLAUDE_BIN", DEFAULT_CLAUDE_BIN);
    let default_model = map_model(Some(&env_or("SHREK_CLAUDE_DEFAULT_MODEL", DEFAULT_MODEL)), "sonnet");
    match std::env::args().nth(1).as_deref() {
        None | Some("serve") => serve(&claude_bin, default_model),
        Some("login") => cmd_login(&claude_bin, default_model),
        Some("health") => cmd_health(&claude_bin, default_model),
        Some(other) => {
            eprintln!("CLAUDE-BROKER-USAGE unknown subcommand {other:?}; use: serve | login | health");
            2
        }
    }
}

/// The slice-4 serve loop: accept the box's plaintext messages-API requests and translate each to a
/// `claude -p` invocation. Unchanged in behavior from slice-4 — only lifted behind the dispatcher.
fn serve(claude_bin: &str, default_model: &'static str) -> i32 {
    let listen = env_or("SHREK_CLAUDE_BROKER_LISTEN", DEFAULT_LISTEN);
    let listener = match TcpListener::bind(&listen) {
        Ok(l) => l,
        Err(e) => { eprintln!("CLAUDE-BROKER-ERROR bind {listen}: {e}"); return 2; }
    };
    println!("CLAUDE-BROKER-LISTEN {listen} claude_bin={claude_bin} default_model={default_model}");

    // One registry for the process lifetime: handle → broker-owned session slot (slice-7). In-memory by
    // design (owner-accepted v1); a broker restart drops the map and the next request re-establishes via
    // the stateless flatten fallback.
    let registry = Arc::new(Registry::new());

    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                let (bin, dm) = (claude_bin.to_string(), default_model);
                let reg = Arc::clone(&registry);
                std::thread::spawn(move || {
                    if let Err(e) = handle(stream, &bin, dm, &reg) {
                        eprintln!("CLAUDE-BROKER-ERROR conn: {e}");
                    }
                });
            }
            Err(e) => eprintln!("CLAUDE-BROKER-ERROR accept: {e}"),
        }
    }
    0
}

/// Handle one box→broker→claude exchange. Reads the box's plaintext messages-API request, translates it
/// to a `claude -p` invocation broker-side, and writes the messages-API-shaped reply back plaintext. On
/// ANY failure — malformed request, CLI non-zero exit, CLI-reported error, unparseable output — fails
/// CLOSED with a 502 to the box (never a fabricated success). No credential is ever read or logged.
fn handle(
    mut box_stream: TcpStream,
    claude_bin: &str,
    default_model: &'static str,
    registry: &Registry,
) -> std::io::Result<()> {
    let (method, path, session, body) = match read_http_request(&mut box_stream) {
        Ok(v) => v,
        Err(e) => {
            write_plain(&mut box_stream, 400, "bad request from box")?;
            return Err(e);
        }
    };

    let req = match parse_messages_request(&body) {
        Some(r) => r,
        None => {
            eprintln!("CLAUDE-BROKER-ERROR unparseable messages-API request body");
            write_plain(&mut box_stream, 400, "unparseable messages request")?;
            return Ok(());
        }
    };
    let model = map_model(req.model.as_deref(), default_model);

    // Route: a valid session handle drives a REAL native session (forward only the tail turn); otherwise,
    // or on any resume failure, fall back to the proven stateless flatten. `run_turn` returns the CLI
    // result either way, so the reply/fail-closed tail below is identical.
    let handle_ok = session.as_deref().and_then(valid_handle);
    let result = match &handle_ok {
        Some(h) => run_session_turn(claude_bin, model, &req, h, registry, &method, &path),
        None => {
            println!(
                "CLAUDE-BROKER-FWD {method} {path} stateless model_req={:?} -> claude --model {model} (msgs={} system={}B)",
                req.model, req.messages.len(), req.system.len()
            );
            invoke_claude(claude_bin, model, &req.system, &flatten(&req.messages), Sess::None)
        }
    };

    // Invoke the LOGGED-IN CLI broker-side. This is the ONLY authority on token health: a real round-trip.
    let result = match result {
        Ok(text) => text,
        Err(e) => {
            // Surface a likely-auth failure distinctly so a revoked/expired login is diagnosable and not
            // mistaken for a model error (the #1567 split-brain lesson — but grounded in a REAL error,
            // never in `claude auth status`). Still fails closed either way.
            if looks_like_auth_failure(&e) {
                eprintln!("CLAUDE-BROKER-UPSTREAM-AUTH-FAIL claude login appears invalid: {e}");
            } else {
                eprintln!("CLAUDE-BROKER-CLI-ERROR {e}");
            }
            write_plain(&mut box_stream, 502, "claude cli invocation failed")?;
            return Ok(());
        }
    };
    println!("CLAUDE-BROKER-CLI-OK result={}B", result.len());

    // Wrap the CLI's text into the Anthropic messages reply shape the coder's extractor already parses.
    let reply = wrap_reply(&result);
    let resp = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
        reply.len()
    );
    box_stream.write_all(resp.as_bytes())?;
    box_stream.flush().ok();
    Ok(())
}

/// Drive one turn of a REAL native `claude` session for `handle`, forwarding only the new tail turn.
/// Holds the per-session lock for the whole turn (owner requirement: same-handle requests serialize;
/// different handles run concurrently). Returns the CLI result, transparently falling back to a stateless
/// full-transcript flatten on ANY divergence or resume failure — so the caller's reply path is identical.
fn run_session_turn(
    bin: &str,
    model: &'static str,
    req: &ParsedRequest,
    handle: &str,
    registry: &Registry,
    method: &str,
    path: &str,
) -> Result<String, String> {
    let slot = registry.slot(handle);
    let mut st = slot.state.lock().unwrap_or_else(|e| e.into_inner());
    let n = req.messages.len();
    let m = st.msgs_forwarded;
    let internal = st.internal_id.clone();
    let mut invalidate = false;

    let result = if m == 0 {
        // First turn: create the native session pinned to the broker-owned id, seed with the full
        // transcript so far.
        println!("CLAUDE-BROKER-FWD {method} {path} session=NEW model={model} msgs={n} -> --session-id (create)");
        let r = invoke_claude(bin, model, &req.system, &flatten(&req.messages), Sess::Create(&internal));
        if r.is_ok() {
            // +1: the CLI produces one assistant reply that the coder appends to its transcript; that reply
            // already lives in the native session, so the next delta must start AFTER it (never re-send it).
            st.msgs_forwarded = n + 1;
        } else {
            invalidate = true;
        }
        r
    } else if n > m {
        // Continuation: forward ONLY the new tail turn(s) via resume; the session retains prior context.
        println!(
            "CLAUDE-BROKER-FWD {method} {path} session=RESUME model={model} forwarding tail {m}..{n} of {n}"
        );
        match invoke_claude(bin, model, "", &flatten(&req.messages[m..n]), Sess::Resume(&internal)) {
            Ok(text) => {
                st.msgs_forwarded = n + 1; // account for the assistant reply the coder will append
                Ok(text)
            }
            Err(e) => {
                // Native resume failed → fall back to a full stateless flatten for THIS call, then forget
                // the handle so the next request re-establishes cleanly. Always correct, just not cheap.
                eprintln!("CLAUDE-BROKER-SESSION-FALLBACK resume failed ({e}); flattening full transcript");
                invalidate = true;
                invoke_claude(bin, model, &req.system, &flatten(&req.messages), Sess::None)
            }
        }
    } else {
        // Divergence: the transcript does not extend what we forwarded (coder reset / retry). Fall back.
        eprintln!("CLAUDE-BROKER-SESSION-FALLBACK divergent transcript (msgs={n} <= forwarded={m}); flattening");
        invalidate = true;
        invoke_claude(bin, model, &req.system, &flatten(&req.messages), Sess::None)
    };

    drop(st);
    if invalidate {
        registry.forget(handle);
    }
    result
}

/// The messages-API request fields the broker cares about. The `messages` array is kept STRUCTURED as
/// `(role, content)` so a sessioned continuation can forward only the new tail turn (slice-7); the
/// stateless path flattens the whole vector via [`flatten`].
struct ParsedRequest {
    /// The top-level `system` field, concatenated (empty string if absent).
    system: String,
    /// The `messages` array as ordered `(role, content)` pairs.
    messages: Vec<(String, String)>,
    /// The requested `model` id, if present — mapped through the allowlist before use.
    model: Option<String>,
}

/// Parse the coder's Anthropic messages-API body into the pieces a `claude -p` call needs. `system` is
/// lifted verbatim; the `messages` array is preserved as ordered `(role, content)` pairs. Returns `None`
/// on any shape mismatch (→ fail closed).
fn parse_messages_request(body: &[u8]) -> Option<ParsedRequest> {
    let s = std::str::from_utf8(body).ok()?;
    let v: JsonValue = s.parse().ok()?;
    let obj = v.get::<HashMap<String, JsonValue>>()?;

    let system = obj
        .get("system")
        .and_then(|s| s.get::<String>())
        .cloned()
        .unwrap_or_default();

    let model = obj.get("model").and_then(|m| m.get::<String>()).cloned();

    let msgs = obj.get("messages")?.get::<Vec<JsonValue>>()?;
    let mut messages = Vec::with_capacity(msgs.len());
    for m in msgs {
        let mo = m.get::<HashMap<String, JsonValue>>()?;
        let role = mo.get("role").and_then(|r| r.get::<String>()).cloned().unwrap_or_else(|| "user".into());
        let content = mo.get("content").and_then(|c| c.get::<String>()).cloned().unwrap_or_default();
        messages.push((role, content));
    }
    if messages.is_empty() {
        return None;
    }
    Some(ParsedRequest { system, messages, model })
}

/// Map a requested model id to a SAFE, fixed `--model` value. The return is always a `&'static str` from
/// this allowlist — a raw caller string NEVER reaches the CLI argv (the fork-#2 argv-injection guard).
/// An absent or unrecognized id maps to `default` (itself already an allowlist value), never rejected in
/// a way that would strand the caller; the caller can *select* among known models but cannot *introduce*
/// one. Accepts both the short aliases and the full product ids the coder may send.
fn map_model(requested: Option<&str>, default: &'static str) -> &'static str {
    match requested.map(str::trim).unwrap_or("") {
        "opus" | "claude-opus-4-8" => "opus",
        "sonnet" | "claude-sonnet-5" => "sonnet",
        "haiku" | "claude-haiku-4-5-20251001" | "claude-haiku-4-5" => "haiku",
        _ => default,
    }
}

/// Invoke the logged-in `claude` CLI in print mode, returning its result text. Built as an argv VECTOR
/// (no shell): the prompt and system are single data arguments, `model` is an allowlist `&'static str`.
/// A non-zero exit, or an `is_error` result, is an `Err` (fails closed upstream). The subscription
/// credential is entirely the CLI's; this process reads none of it.
fn invoke_claude(bin: &str, model: &'static str, system: &str, prompt: &str, sess: Sess) -> Result<String, String> {
    let mut cmd = Command::new(bin);
    cmd.arg("-p").arg(prompt)
        .arg("--output-format").arg("json")
        .arg("--model").arg(model);
    match sess {
        // Continuation: the native session already holds the system prompt and prior turns; resume it by
        // the broker-owned id and send ONLY the new tail turn as the prompt.
        Sess::Resume(id) => {
            cmd.arg("--resume").arg(id);
        }
        // First turn of a session: pin the broker-minted id so later `--resume <id>` targets it, and set
        // the system prompt once.
        Sess::Create(id) => {
            cmd.arg("--session-id").arg(id);
            if !system.is_empty() {
                cmd.arg("--system-prompt").arg(system);
            }
        }
        // Stateless single-shot (slice-4): the whole transcript is the prompt; system sent inline.
        Sess::None => {
            if !system.is_empty() {
                // Replace the default system prompt with the coder's protocol (the proven
                // CLAUDE-SUB-AS-LLM pattern, #1788): the CLI acts as a raw protocol-following LLM.
                cmd.arg("--system-prompt").arg(system);
            }
        }
    }
    let out = cmd.output().map_err(|e| format!("spawn {bin}: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("claude exit={:?}: {}", out.status.code(), cap(&err, 400)));
    }
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    extract_claude_result(&stdout)
}

/// Pull `.result` out of `claude --output-format json` stdout. Fails closed if `is_error` is true, if
/// the object is missing `result`, or if stdout is not the expected JSON object.
fn extract_claude_result(stdout: &str) -> Result<String, String> {
    use std::collections::HashMap;
    let v: JsonValue = stdout.trim().parse().map_err(|_| "claude stdout not JSON".to_string())?;
    let obj = v.get::<HashMap<String, JsonValue>>().ok_or("claude stdout not a JSON object")?;
    let is_error = obj.get("is_error").and_then(|b| b.get::<bool>()).copied().unwrap_or(false);
    let result = obj.get("result").and_then(|r| r.get::<String>()).cloned();
    if is_error {
        let sub = obj.get("subtype").and_then(|s| s.get::<String>()).cloned().unwrap_or_default();
        return Err(format!("claude is_error subtype={sub:?} result={:?}", result.map(|r| cap(&r, 300))));
    }
    result.ok_or_else(|| "claude reply had no result field".to_string())
}

/// Wrap the CLI's text into the Anthropic messages reply shape (`content: [{type:text,text:…}]`) so the
/// coder's `extract_anthropic_content` parses it unchanged. Built as a `JsonValue` so the text is
/// JSON-escaped correctly regardless of what the model returned.
fn wrap_reply(result: &str) -> String {
    use std::collections::HashMap;
    let mut block = HashMap::new();
    block.insert("type".to_string(), JsonValue::String("text".to_string()));
    block.insert("text".to_string(), JsonValue::String(result.to_string()));
    let mut root = HashMap::new();
    root.insert("content".to_string(), JsonValue::Array(vec![JsonValue::Object(block)]));
    JsonValue::Object(root).stringify().expect("reply JSON always serializes")
}

/// Best-effort classification of a CLI error as an auth/login failure (for a distinct diagnostic marker
/// only; the failure is fail-closed regardless). Grounded in the REAL error text, never `auth status`.
fn looks_like_auth_failure(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("authentication") || e.contains("401") || e.contains("unauthorized")
        || e.contains("invalid api key") || e.contains("oauth") || e.contains("login")
}

fn cap(s: &str, n: usize) -> String {
    if s.len() <= n { s.to_string() } else { format!("{}…", &s[..n]) }
}

// ---- minimal HTTP/1.1 request reader (box→broker is a controlled, single-request path) -------------
// Mirrors crates/model-proxy's proven reader: request line + headers + exactly Content-Length body.

fn read_http_request(stream: &mut TcpStream) -> std::io::Result<(String, String, Option<String>, Vec<u8>)> {
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
    let mut parts = req_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    if method.is_empty() || path.is_empty() {
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
                // Captured verbatim here; validated by valid_handle() before any use.
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
    Ok((method, path, session, body))
}

fn write_plain(stream: &mut TcpStream, code: u16, msg: &str) -> std::io::Result<()> {
    let body = format!("{{\"error\":{{\"type\":\"broker_error\",\"message\":{msg:?}}}}}");
    let resp = format!(
        "HTTP/1.1 {code} BROKER\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(resp.as_bytes())?;
    stream.flush().ok();
    Ok(())
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
}

fn ioerr(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg.to_string())
}

// ---- Phase-6 slice-5: the login UX + the audit-only availability breadcrumb ------------------------
//
// "Sign in with Claude" WITHOUT a preexisting manual login: `claude-broker login` hands the operator's
// REAL terminal to the official `claude auth login --claudeai`, which owns 100% of the credential state
// (it writes its own ~/.claude; Shrek never sees a token). Completion is then VERIFIED by one real
// `claude -p` round-trip — never `claude auth status` (#1567 lies) — and observed as a STATE TRANSITION
// recorded in an audit-only breadcrumb. No token is ever read, parsed, or stored. This is a broker-host
// OPERATOR ceremony, deliberately NOT the sandboxed-agent grant path (grant-protocol.md): there is no
// sandboxed adversary in this loop to spoof a prompt, so the SAK/VT anchor does not apply here.

const PROVIDER: &str = "claude-cli";
/// The fixed, tiny prompt for the round-trip health probe. A real reply proves the login round-trips;
/// its CONTENT is irrelevant (we never inspect it), so no assistant behavior is depended upon.
const PROBE_PROMPT: &str = "ping";
const BREADCRUMB_FILE: &str = "availability.json";
const BREADCRUMB_TMP: &str = ".availability.json.tmp";

/// The breadcrumb's `reason` — a FIXED enum, never free text. This is the owner-mandated guard against
/// the breadcrumb becoming an accidental credential/logging surface: raw `claude` stdout/stderr (which
/// could in principle echo a token) NEVER reaches the file — only one of these five stable strings does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reason {
    /// Login present and a real round-trip succeeded.
    Verified,
    /// The round-trip failed in a way that classifies as an auth/login problem.
    AuthFailed,
    /// `claude auth login` itself exited non-zero (or could not be spawned).
    LoginFailed,
    /// `login` refused because it was not attached to a real terminal (fail-closed, never hangs — #595).
    NonTty,
    /// The round-trip failed for a non-auth reason (CLI error, unparseable output, spawn failure).
    ProbeFailed,
}

impl Reason {
    fn as_str(self) -> &'static str {
        match self {
            Reason::Verified => "verified",
            Reason::AuthFailed => "auth-failed",
            Reason::LoginFailed => "login-failed",
            Reason::NonTty => "non-tty",
            Reason::ProbeFailed => "probe-failed",
        }
    }
}

/// The audit-only availability record. Deliberately holds NO credential and NO CLI output — only a bit,
/// a fixed reason, and a timestamp. It informs; it never gates. (Every real request still round-trips
/// and fails closed on the live error.)
struct Availability {
    available: bool,
    reason: Reason,
    /// Seconds since the Unix epoch at the moment of observation (broker-side host clock).
    last_verified: u64,
}

/// Serialize the breadcrumb via the JSON encoder (so every value is correctly escaped) with EXACTLY the
/// four audit fields. No field carries CLI output or a credential.
fn availability_json(a: &Availability) -> String {
    use std::collections::HashMap;
    let mut root = HashMap::new();
    root.insert("provider".to_string(), JsonValue::String(PROVIDER.to_string()));
    root.insert("available".to_string(), JsonValue::Boolean(a.available));
    root.insert("reason".to_string(), JsonValue::String(a.reason.as_str().to_string()));
    root.insert("last_verified".to_string(), JsonValue::Number(a.last_verified as f64));
    JsonValue::Object(root).stringify().expect("availability JSON always serializes")
}

fn now_epoch() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Broker-side directory for the breadcrumb: `$SHREK_CLAUDE_STATE_DIR`, else `$HOME/.local/state/
/// shrek-claude-cli`, else a cwd-relative fallback. Always broker-side; the sealed box never reads it.
fn state_dir() -> PathBuf {
    if let Some(d) = std::env::var("SHREK_CLAUDE_STATE_DIR").ok().filter(|v| !v.is_empty()) {
        return PathBuf::from(d);
    }
    if let Some(h) = std::env::var("HOME").ok().filter(|v| !v.is_empty()) {
        return Path::new(&h).join(".local/state/shrek-claude-cli");
    }
    PathBuf::from("shrek-claude-cli-state")
}

/// Write the breadcrumb ATOMICALLY and OWNER-ONLY: fresh 0600 temp file → write → fsync → rename over
/// the target → fsync the directory. A crash can never leave believable partial state (a half-written
/// file is only ever the temp name, and the rename is atomic + durably persisted). The dir is 0700.
fn write_availability_to(dir: &Path, a: &Availability) -> std::io::Result<()> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let tmp = dir.join(BREADCRUMB_TMP);
    let final_path = dir.join(BREADCRUMB_FILE);
    // Remove any stale temp so the create below always yields a fresh, exclusively-created 0600 file.
    let _ = fs::remove_file(&tmp);
    {
        let mut f = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        f.write_all(availability_json(a).as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, &final_path)?;
    // Persist the rename itself (the directory entry) so a crash cannot resurrect the old target.
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Record an observation to the breadcrumb (audit-only). A write failure is logged with a fixed marker
/// and otherwise ignored — the breadcrumb never gates behavior, so its absence must not mask a result.
fn record(available: bool, reason: Reason) {
    let a = Availability { available, reason, last_verified: now_epoch() };
    if let Err(e) = write_availability_to(&state_dir(), &a) {
        // Only the io::Error kind (a fixed set), never CLI output, reaches this operator log line.
        eprintln!("CLAUDE-BROKER-BREADCRUMB-WRITE-FAIL kind={:?}", e.kind());
    }
}

/// One real `claude -p` round-trip. Returns `Ok` if the CLI round-tripped; on failure returns a FIXED
/// `Reason` derived only from the error CLASSIFIER — the raw error text (which could echo a secret) is
/// deliberately DROPPED here, never returned or stored. This is the only authority on login health;
/// `claude auth status` is never consulted (#1567).
fn probe(bin: &str, model: &'static str) -> Result<(), Reason> {
    match invoke_claude(bin, model, "", PROBE_PROMPT, Sess::None) {
        Ok(_) => Ok(()),
        Err(e) if looks_like_auth_failure(&e) => Err(Reason::AuthFailed),
        Err(_) => Err(Reason::ProbeFailed),
    }
}

/// `claude-broker login` — the trusted OPERATOR path (broker-host console, not the sandboxed-agent grant
/// path). Refuses fast if not on a real terminal (never hangs on a browser callback — #595), hands the
/// terminal to the official `claude auth login --claudeai` (which owns all credential state and whose
/// output we never capture), then folds in the round-trip health check and records the result.
fn cmd_login(bin: &str, model: &'static str) -> i32 {
    if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        record(false, Reason::NonTty);
        eprintln!(
            "CLAUDE-BROKER-LOGIN-REFUSED reason=non-tty (run at a real console; the official OAuth flow \
             needs an interactive terminal — a headless run would hang on the browser callback)"
        );
        return 3;
    }
    println!(
        "CLAUDE-BROKER-LOGIN-BEGIN handing the terminal to `claude auth login --claudeai` \
         (the CLI owns ALL credential state; Shrek captures nothing)"
    );
    // Fixed argv vector, inherited stdio (never captured): no caller input, no token-capture surface.
    match Command::new(bin).arg("auth").arg("login").arg("--claudeai").status() {
        Ok(s) if s.success() => {}
        Ok(s) => {
            record(false, Reason::LoginFailed);
            eprintln!("CLAUDE-BROKER-LOGIN-FAIL reason=login-failed exit={:?}", s.code());
            return 5;
        }
        Err(_e) => {
            // Do not print `_e` — keep even spawn errors off the operator log's credential-adjacent paths.
            record(false, Reason::LoginFailed);
            eprintln!("CLAUDE-BROKER-LOGIN-FAIL reason=login-failed (could not spawn the claude CLI)");
            return 5;
        }
    }
    println!("CLAUDE-BROKER-LOGIN-DONE verifying with one real `claude -p` round-trip (never `auth status`)");
    match probe(bin, model) {
        Ok(()) => {
            record(true, Reason::Verified);
            println!("CLAUDE-BROKER-LOGIN-VERIFIED reason=verified provider={PROVIDER}");
            0
        }
        Err(r) => {
            record(false, r);
            eprintln!(
                "CLAUDE-BROKER-LOGIN-UNVERIFIED reason={} (login exited 0 but the round-trip failed)",
                r.as_str()
            );
            4
        }
    }
}

/// `claude-broker health` — run just the round-trip probe and update the breadcrumb. Used standalone (a
/// proactive "is the login still valid" check) and as the tail of `login`.
fn cmd_health(bin: &str, model: &'static str) -> i32 {
    match probe(bin, model) {
        Ok(()) => {
            record(true, Reason::Verified);
            println!("CLAUDE-BROKER-PROBE-OK reason=verified provider={PROVIDER}");
            0
        }
        Err(r) => {
            record(false, r);
            eprintln!("CLAUDE-BROKER-PROBE-FAIL reason={}", r.as_str());
            4
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_model_allowlists_and_never_passes_raw() {
        // Known short aliases and full ids map to a fixed value.
        assert_eq!(map_model(Some("sonnet"), "sonnet"), "sonnet");
        assert_eq!(map_model(Some("claude-sonnet-5"), "sonnet"), "sonnet");
        assert_eq!(map_model(Some("opus"), "sonnet"), "opus");
        assert_eq!(map_model(Some("claude-opus-4-8"), "sonnet"), "opus");
        assert_eq!(map_model(Some("haiku"), "sonnet"), "haiku");
        // Unknown / absent / injection-y strings ALL fall to the default — the raw string is discarded,
        // never returned (so it can never reach the CLI argv).
        assert_eq!(map_model(Some("evil; rm -rf /"), "sonnet"), "sonnet");
        assert_eq!(map_model(Some("--dangerously-skip-permissions"), "haiku"), "haiku");
        assert_eq!(map_model(Some(""), "opus"), "opus");
        assert_eq!(map_model(None, "sonnet"), "sonnet");
    }

    #[test]
    fn parse_messages_flattens_transcript_and_lifts_system() {
        let body = br#"{"model":"claude-sonnet-5","max_tokens":2048,"system":"be terse",
            "messages":[{"role":"user","content":"fix the bug"},
                        {"role":"assistant","content":"{\"tool\":\"read_file\"}"},
                        {"role":"user","content":"OK contents"}]}"#;
        let r = parse_messages_request(body).expect("parses");
        assert_eq!(r.system, "be terse");
        assert_eq!(r.model.as_deref(), Some("claude-sonnet-5"));
        assert_eq!(r.messages.len(), 3);
        assert_eq!(
            flatten(&r.messages),
            "User: fix the bug\n\nAssistant: {\"tool\":\"read_file\"}\n\nUser: OK contents"
        );
        // A sessioned continuation flattens ONLY the tail, never re-sending the prefix.
        assert_eq!(flatten(&r.messages[2..]), "User: OK contents");
    }

    #[test]
    fn parse_messages_absent_system_is_empty() {
        let body = br#"{"messages":[{"role":"user","content":"hi"}]}"#;
        let r = parse_messages_request(body).expect("parses");
        assert_eq!(r.system, "");
        assert_eq!(flatten(&r.messages), "User: hi");
        assert!(r.model.is_none());
    }

    #[test]
    fn parse_messages_fails_closed_on_junk() {
        assert!(parse_messages_request(b"not json").is_none());
        assert!(parse_messages_request(b"{\"messages\":[]}").is_none()); // empty prompt → None
        assert!(parse_messages_request(b"{\"no_messages\":true}").is_none());
    }

    // ---- slice-7: session registry / handle hygiene ----

    #[test]
    fn valid_handle_bounds_charset_and_length() {
        assert_eq!(valid_handle("aZ09-_").as_deref(), Some("aZ09-_"));
        assert_eq!(valid_handle("  trim-me  ").as_deref(), Some("trim-me"));
        assert!(valid_handle("").is_none());
        // Path-traversal / argv-injection shapes are rejected outright.
        assert!(valid_handle("../etc").is_none());
        assert!(valid_handle("a/b").is_none());
        assert!(valid_handle("a b").is_none());
        assert!(valid_handle("a;rm").is_none());
        assert!(valid_handle(&"x".repeat(MAX_HANDLE_LEN + 1)).is_none());
    }

    #[test]
    fn internal_id_is_uuid_shaped_and_unique_never_the_handle() {
        let a = mint_internal_id();
        let b = mint_internal_id();
        assert_ne!(a, b, "ids must be unique");
        assert_eq!(a.len(), 36);
        assert_eq!(a.as_bytes()[14], b'4', "version 4 nibble");
        let dashes: Vec<usize> = a.match_indices('-').map(|(i, _)| i).collect();
        assert_eq!(dashes, vec![8, 13, 18, 23], "8-4-4-4-12 layout");
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-'));
    }

    #[test]
    fn registry_reuses_slot_for_same_handle_and_forget_resets_identity() {
        let reg = Registry::new();
        let s1 = reg.slot("sess-A");
        let id1 = s1.state.lock().unwrap().internal_id.clone();
        // Same handle → same slot, same broker-owned id (identity is stable within a session).
        let s2 = reg.slot("sess-A");
        assert_eq!(s2.state.lock().unwrap().internal_id, id1);
        assert!(Arc::ptr_eq(&s1, &s2));
        // A different handle → a different slot/id (never a collision).
        let other = reg.slot("sess-B");
        assert_ne!(other.state.lock().unwrap().internal_id, id1);
        // Forget → the next sighting mints a FRESH id (fallback re-establishment).
        reg.forget("sess-A");
        let s3 = reg.slot("sess-A");
        assert_ne!(s3.state.lock().unwrap().internal_id, id1);
    }

    #[test]
    fn extract_result_reads_success_and_fails_closed_on_error() {
        let ok = r#"{"type":"result","subtype":"success","is_error":false,"result":"done"}"#;
        assert_eq!(extract_claude_result(ok).unwrap(), "done");
        // is_error true → Err even if a result string is present.
        let err = r#"{"is_error":true,"subtype":"error_during_execution","result":"boom"}"#;
        assert!(extract_claude_result(err).is_err());
        // missing result → Err.
        assert!(extract_claude_result(r#"{"is_error":false}"#).is_err());
        // non-JSON → Err.
        assert!(extract_claude_result("total garbage").is_err());
    }

    #[test]
    fn wrap_reply_is_valid_anthropic_shape_and_escapes() {
        // A result containing quotes/newlines must round-trip as valid JSON text.
        let reply = wrap_reply("line1\n\"quoted\"");
        let v: JsonValue = reply.parse().expect("valid json");
        let obj = v.get::<std::collections::HashMap<String, JsonValue>>().unwrap();
        let blocks = obj.get("content").unwrap().get::<Vec<JsonValue>>().unwrap();
        let b0 = blocks[0].get::<std::collections::HashMap<String, JsonValue>>().unwrap();
        assert_eq!(b0.get("type").unwrap().get::<String>().unwrap(), "text");
        assert_eq!(b0.get("text").unwrap().get::<String>().unwrap(), "line1\n\"quoted\"");
    }

    #[test]
    fn auth_failure_classifier() {
        assert!(looks_like_auth_failure("Error: 401 Unauthorized"));
        assert!(looks_like_auth_failure("OAuth token expired"));
        assert!(looks_like_auth_failure("please run login"));
        assert!(!looks_like_auth_failure("model produced invalid output"));
    }

    #[test]
    fn find_subslice_locates_blank_line() {
        assert_eq!(find_subslice(b"a\r\n\r\nb", b"\r\n\r\n"), Some(1));
        assert_eq!(find_subslice(b"noblank", b"\r\n\r\n"), None);
    }

    // ---- slice-5: the audit-only availability breadcrumb ----

    #[test]
    fn reason_maps_to_fixed_strings() {
        // The five stable strings the breadcrumb may ever carry — the guard against raw CLI text.
        assert_eq!(Reason::Verified.as_str(), "verified");
        assert_eq!(Reason::AuthFailed.as_str(), "auth-failed");
        assert_eq!(Reason::LoginFailed.as_str(), "login-failed");
        assert_eq!(Reason::NonTty.as_str(), "non-tty");
        assert_eq!(Reason::ProbeFailed.as_str(), "probe-failed");
    }

    #[test]
    fn availability_json_has_only_audit_fields_and_no_credential() {
        use std::collections::HashMap;
        let a = Availability { available: true, reason: Reason::Verified, last_verified: 1_700_000_000 };
        let s = availability_json(&a);
        // Never a credential-shaped surface.
        assert!(!s.contains("sk-ant"), "breadcrumb must never carry a token: {s}");
        let v: JsonValue = s.parse().expect("valid json");
        let obj = v.get::<HashMap<String, JsonValue>>().unwrap();
        // EXACTLY the four audit fields — nothing that could carry CLI output.
        let mut keys: Vec<String> = obj.keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "available".to_string(),
                "last_verified".to_string(),
                "provider".to_string(),
                "reason".to_string()
            ]
        );
        assert_eq!(obj.get("provider").unwrap().get::<String>().unwrap(), "claude-cli");
        assert!(*obj.get("available").unwrap().get::<bool>().unwrap());
        assert_eq!(obj.get("reason").unwrap().get::<String>().unwrap(), "verified");
    }

    #[test]
    fn breadcrumb_write_is_atomic_owner_only_and_leaves_no_temp() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("shrek-bc-{}-atomic", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = Availability { available: false, reason: Reason::AuthFailed, last_verified: 42 };
        write_availability_to(&dir, &a).expect("writes");
        let final_path = dir.join(BREADCRUMB_FILE);
        // 0600, owner-only.
        let mode = std::fs::metadata(&final_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "breadcrumb must be owner-only 0600, got {mode:o}");
        // No temp leftover — the atomic rename cleaned up.
        assert!(!dir.join(BREADCRUMB_TMP).exists(), "atomic rename must leave no .tmp");
        // Content round-trips.
        let read = std::fs::read_to_string(&final_path).unwrap();
        let v: JsonValue = read.parse().unwrap();
        let obj = v.get::<std::collections::HashMap<String, JsonValue>>().unwrap();
        assert_eq!(obj.get("reason").unwrap().get::<String>().unwrap(), "auth-failed");
        assert!(!*obj.get("available").unwrap().get::<bool>().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn breadcrumb_overwrite_replaces_and_stays_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("shrek-bc-{}-overwrite", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_availability_to(&dir, &Availability { available: true, reason: Reason::Verified, last_verified: 1 })
            .unwrap();
        write_availability_to(&dir, &Availability { available: false, reason: Reason::ProbeFailed, last_verified: 2 })
            .unwrap();
        let final_path = dir.join(BREADCRUMB_FILE);
        let mode = std::fs::metadata(&final_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let read = std::fs::read_to_string(&final_path).unwrap();
        assert!(read.contains("probe-failed") && read.contains("false"));
        assert!(!dir.join(BREADCRUMB_TMP).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
