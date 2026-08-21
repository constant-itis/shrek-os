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

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;

use tinyjson::JsonValue;

const DEFAULT_LISTEN: &str = "127.0.0.1:8300";
const DEFAULT_CLAUDE_BIN: &str = "claude";
const DEFAULT_MODEL: &str = "sonnet";
/// Guard: refuse to read an absurdly large request so a broken peer cannot OOM the broker.
const MAX_BODY: usize = 16 * 1024 * 1024;

fn main() {
    std::process::exit(real_main());
}

fn real_main() -> i32 {
    let listen = env_or("SHREK_CLAUDE_BROKER_LISTEN", DEFAULT_LISTEN);
    let claude_bin = env_or("SHREK_CLAUDE_BIN", DEFAULT_CLAUDE_BIN);
    let default_model = map_model(Some(&env_or("SHREK_CLAUDE_DEFAULT_MODEL", DEFAULT_MODEL)), "sonnet");

    let listener = match TcpListener::bind(&listen) {
        Ok(l) => l,
        Err(e) => { eprintln!("CLAUDE-BROKER-ERROR bind {listen}: {e}"); return 2; }
    };
    println!("CLAUDE-BROKER-LISTEN {listen} claude_bin={claude_bin} default_model={default_model}");

    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                let (bin, dm) = (claude_bin.clone(), default_model);
                std::thread::spawn(move || {
                    if let Err(e) = handle(stream, &bin, dm) {
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
fn handle(mut box_stream: TcpStream, claude_bin: &str, default_model: &'static str) -> std::io::Result<()> {
    let (method, path, body) = match read_http_request(&mut box_stream) {
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
    println!(
        "CLAUDE-BROKER-FWD {method} {path} model_req={:?} -> claude --model {model} (prompt={}B system={}B)",
        req.model, req.prompt.len(), req.system.len()
    );

    // Invoke the LOGGED-IN CLI broker-side. This is the ONLY authority on token health: a real round-trip.
    let result = match invoke_claude(claude_bin, model, &req.system, &req.prompt) {
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

/// The messages-API request fields the broker cares about, already flattened for a single-shot CLI call.
struct ParsedRequest {
    /// The top-level `system` field, concatenated (empty string if absent).
    system: String,
    /// The `messages` array rendered to a single prompt string (role-labelled transcript).
    prompt: String,
    /// The requested `model` id, if present — mapped through the allowlist before use.
    model: Option<String>,
}

/// Parse the coder's Anthropic messages-API body into the pieces a `claude -p` call needs. `system` is
/// lifted verbatim; the `messages` array is flattened into a role-labelled transcript so a stateless
/// single-shot CLI call reproduces the conversation. Returns `None` on any shape mismatch (→ fail closed).
///
/// v1 renders the transcript as text (`User:`/`Assistant:` turns). A stateful `--resume` session that
/// preserves structured turns is a tracked follow (docs §6), not needed for the bounded coder loop.
fn parse_messages_request(body: &[u8]) -> Option<ParsedRequest> {
    use std::collections::HashMap;
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
    let mut prompt = String::new();
    for m in msgs {
        let mo = m.get::<HashMap<String, JsonValue>>()?;
        let role = mo.get("role").and_then(|r| r.get::<String>()).map(String::as_str).unwrap_or("user");
        let content = mo.get("content").and_then(|c| c.get::<String>()).cloned().unwrap_or_default();
        let label = match role {
            "assistant" => "Assistant",
            _ => "User",
        };
        if !prompt.is_empty() {
            prompt.push_str("\n\n");
        }
        prompt.push_str(label);
        prompt.push_str(": ");
        prompt.push_str(&content);
    }
    if prompt.is_empty() {
        return None;
    }
    Some(ParsedRequest { system, prompt, model })
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
fn invoke_claude(bin: &str, model: &'static str, system: &str, prompt: &str) -> Result<String, String> {
    let mut cmd = Command::new(bin);
    cmd.arg("-p").arg(prompt)
        .arg("--output-format").arg("json")
        .arg("--model").arg(model);
    if !system.is_empty() {
        // Replace the default system prompt with the coder's protocol (the proven CLAUDE-SUB-AS-LLM
        // pattern, #1788): the CLI acts as a raw protocol-following LLM, not Claude-Code-the-agent.
        cmd.arg("--system-prompt").arg(system);
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

fn read_http_request(stream: &mut TcpStream) -> std::io::Result<(String, String, Vec<u8>)> {
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
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                content_length = v.trim().parse().map_err(|_| ioerr("bad content-length"))?;
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
    Ok((method, path, body))
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
        assert_eq!(r.prompt, "User: fix the bug\n\nAssistant: {\"tool\":\"read_file\"}\n\nUser: OK contents");
    }

    #[test]
    fn parse_messages_absent_system_is_empty() {
        let body = br#"{"messages":[{"role":"user","content":"hi"}]}"#;
        let r = parse_messages_request(body).expect("parses");
        assert_eq!(r.system, "");
        assert_eq!(r.prompt, "User: hi");
        assert!(r.model.is_none());
    }

    #[test]
    fn parse_messages_fails_closed_on_junk() {
        assert!(parse_messages_request(b"not json").is_none());
        assert!(parse_messages_request(b"{\"messages\":[]}").is_none()); // empty prompt → None
        assert!(parse_messages_request(b"{\"no_messages\":true}").is_none());
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
}
