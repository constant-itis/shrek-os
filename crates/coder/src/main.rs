//! coder — the first real coding-agent workload for a `shrek run` T2 session
//! (docs/phase6-slice2-coder-agent.md).
//!
//! This binary is NOT a plane and owns NO isolation. It runs AS the workload of a `shrek run`
//! session — inside the gVisor box, with a write-through project grant (noexec), an exec build grant,
//! and, when `--egress model-local` is named, a sealed one-destination egress to the model. Everything
//! it can touch is already bounded by that box; a bad/injected model tool-call cannot escape it
//! (agents.md §8/§11 — the confused-deputy residual the T2 wall bounds). The loop is deliberately
//! minimal and bounded to ONE task: inspect → model → edit → build/test → return.
//!
//! Protocol: a plain `chat/completions` POST over std TcpStream (the v1 endpoint is plain
//! HTTP — no TLS in-sandbox). The model replies with exactly one JSON tool-call object; the coder
//! executes it against the grants and appends the result to the transcript, until `done` or the step
//! cap trips (fail-closed). Outcomes are printed with anchored markers (`CODER-STEP`, `CODER-TOOL`,
//! `CODER-DONE`) so the acceptance gate greps outcomes, not model prose.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;
use tinyjson::JsonValue;

const DEFAULT_MODEL_URL: &str = "http://shrek-model:8100/v1/chat/completions";
const DEFAULT_MODEL: &str = "local";
const DEFAULT_MAX_STEPS: u32 = 8;
/// Cap on any single tool-result fed back into the transcript — keeps a runaway `run` from ballooning
/// the context (and the request body) without bound.
const RESULT_CAP: usize = 4000;

fn main() {
    std::process::exit(real_main());
}

fn real_main() -> i32 {
    let mut task: Option<String> = None;
    let mut model_url = std::env::var("SHREK_MODEL_URL").unwrap_or_else(|_| DEFAULT_MODEL_URL.into());
    let mut model = std::env::var("SHREK_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
    let mut max_steps = DEFAULT_MAX_STEPS;
    let mut live = false;

    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--task" => { i += 1; task = argv.get(i).cloned(); }
            "--model-url" => { i += 1; if let Some(v) = argv.get(i) { model_url = v.clone(); } }
            "--model" => { i += 1; if let Some(v) = argv.get(i) { model = v.clone(); } }
            "--max-steps" => {
                i += 1;
                match argv.get(i).and_then(|s| s.parse::<u32>().ok()) {
                    Some(n) if n >= 1 => max_steps = n,
                    _ => { eprintln!("coder: --max-steps needs a positive integer"); return 2; }
                }
            }
            "--live" => live = true,
            "-h" | "--help" => { usage(); return 0; }
            other => { eprintln!("coder: unknown arg `{other}`"); usage(); return 2; }
        }
        i += 1;
    }

    let Some(task) = task else {
        eprintln!("coder: --task \"<one-line task>\" is required");
        usage();
        return 2;
    };

    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| ".".into());
    println!(
        "CODER-START task={:?} model_url={} model={} max_steps={} live={} cwd={}",
        task, model_url, model, max_steps, live, cwd
    );

    run_loop(&task, &model_url, &model, max_steps)
}

fn usage() {
    eprintln!("coder — the coding-agent workload for `shrek run` (Phase-6 slice-2)");
    eprintln!();
    eprintln!("  coder --task \"<one-line task>\" [opts]");
    eprintln!("    --model-url URL   chat/completions endpoint");
    eprintln!("                      (default {DEFAULT_MODEL_URL}; env SHREK_MODEL_URL)");
    eprintln!("    --model NAME      model name field (default {DEFAULT_MODEL}; env SHREK_MODEL)");
    eprintln!("    --max-steps N     hard cap on loop iterations (default {DEFAULT_MAX_STEPS}); tripping it fails closed");
    eprintln!("    --live            informational marker for a real-model smoke (no behavior change)");
    eprintln!();
    eprintln!("  Runs inside the T2 box: reads/writes the CWD project grant, builds in /srv/build,");
    eprintln!("  reaches only the sealed egress the session was constructed with. Bounded to one task.");
}

/// The agent loop. Returns the process exit code: 0 = done ok, 1 = done not-ok, 3 = step cap
/// (fail-closed), 4 = model/transport/parse failure (fail-closed).
fn run_loop(task: &str, model_url: &str, model: &str, max_steps: u32) -> i32 {
    let mut messages: Vec<(String, String)> = vec![
        ("system".into(), system_prompt()),
        ("user".into(), initial_user_message(task)),
    ];

    for step in 1..=max_steps {
        println!("CODER-STEP {step}");

        let request = build_request(model, &messages);
        let reply_body = match http_post_json(model_url, &request) {
            Ok(b) => b,
            Err(e) => { eprintln!("CODER-ERROR transport: {e}"); return 4; }
        };
        let content = match extract_assistant_content(&reply_body) {
            Some(c) => c,
            None => { eprintln!("CODER-ERROR model reply had no assistant content"); return 4; }
        };
        messages.push(("assistant".into(), content.clone()));

        let call = match parse_tool_call(&content) {
            Some(c) => c,
            None => {
                // Let a real model recover from a malformed turn; a canned gate never hits this.
                eprintln!("CODER-WARN step {step}: no JSON tool-call parsed from reply");
                messages.push((
                    "user".into(),
                    "ERROR: your last reply was not a single JSON tool-call. Reply with exactly one \
                     JSON object: {\"tool\":\"...\",\"args\":{...}}."
                        .into(),
                ));
                continue;
            }
        };

        match call.tool.as_str() {
            "done" => {
                let ok = call.arg_bool("ok").unwrap_or(false);
                let summary = call.arg_str("summary").unwrap_or_default();
                println!("CODER-DONE ok={ok} summary={summary:?}");
                return if ok { 0 } else { 1 };
            }
            "read_file" => {
                let path = call.arg_str("path").unwrap_or_default();
                let result = match std::fs::read_to_string(&path) {
                    Ok(s) => format!("OK contents of {path}:\n{}", cap(&s)),
                    Err(e) => format!("ERROR reading {path}: {e}"),
                };
                println!("CODER-TOOL read_file path={path:?}");
                emit_result(&result);
                messages.push(("user".into(), tool_result(&result)));
            }
            "write_file" => {
                let path = call.arg_str("path").unwrap_or_default();
                let content = call.arg_str("content").unwrap_or_default();
                let result = match std::fs::write(&path, content.as_bytes()) {
                    Ok(()) => format!("OK wrote {} bytes to {path}", content.len()),
                    Err(e) => format!("ERROR writing {path}: {e}"),
                };
                println!("CODER-TOOL write_file path={path:?} bytes={}", content.len());
                emit_result(&result);
                messages.push(("user".into(), tool_result(&result)));
            }
            "run" => {
                let cmd = call.arg_str("cmd").unwrap_or_default();
                let result = run_shell(&cmd);
                println!("CODER-TOOL run cmd={cmd:?}");
                emit_result(&result);
                messages.push(("user".into(), tool_result(&result)));
            }
            other => {
                let result = format!(
                    "ERROR unknown tool {other:?}; valid tools: read_file, write_file, run, done"
                );
                println!("CODER-TOOL unknown={other:?}");
                emit_result(&result);
                messages.push(("user".into(), tool_result(&result)));
            }
        }
    }

    eprintln!("CODER-ERROR step cap ({max_steps}) reached without done — failing closed");
    3
}

// ---- the model protocol ------------------------------------------------------------------------

fn system_prompt() -> String {
    // The whole tool surface. The model must reply with exactly ONE JSON object and nothing else.
    "You are a coding agent running inside a locked sandbox. You can only touch the current project \
     directory and a build area at /srv/build. Work on ONE task, then finish.\n\n\
     Reply with EXACTLY ONE JSON object and no other text. Shape:\n\
       {\"tool\":\"read_file\",\"args\":{\"path\":\"buggy.c\"}}\n\
       {\"tool\":\"write_file\",\"args\":{\"path\":\"buggy.c\",\"content\":\"...full new file...\"}}\n\
       {\"tool\":\"run\",\"args\":{\"cmd\":\"tcc -nostdlib -static -o /srv/build/prog buggy.c && /srv/build/prog; echo exit=$?\"}}\n\
       {\"tool\":\"done\",\"args\":{\"ok\":true,\"summary\":\"what you changed and the test result\"}}\n\n\
     Rules: write_file replaces the whole file. Compile into /srv/build (the only place a binary can \
     run); the project dir is non-executable. Verify by running the program before you call done. \
     Call done with ok:true only once the task's success condition is observed."
        .into()
}

fn initial_user_message(task: &str) -> String {
    let listing = list_cwd();
    format!("TASK: {task}\n\nProject files in the current directory:\n{listing}\n\nBegin.")
}

/// A one-level listing of the CWD (the project grant) so the model knows what it is working with.
fn list_cwd() -> String {
    match std::fs::read_dir(".") {
        Ok(rd) => {
            let mut names: Vec<String> = rd
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            if names.is_empty() { "(empty)".into() } else { names.join("\n") }
        }
        Err(e) => format!("(could not list: {e})"),
    }
}

fn tool_result(s: &str) -> String {
    format!("TOOL RESULT:\n{}", cap(s))
}

/// Echo a tool's (capped) result to stdout inside anchored markers. This is honest transcript logging
/// — the operator (and the acceptance gate) sees exactly what the model's commands produced, including
/// build/test output and any wall-probe outcomes, without having to trust the model's own summary.
fn emit_result(result: &str) {
    println!("CODER-RESULT-BEGIN");
    println!("{}", cap(result));
    println!("CODER-RESULT-END");
}

fn cap(s: &str) -> String {
    if s.len() <= RESULT_CAP {
        s.to_string()
    } else {
        format!("{}\n…[truncated {} bytes]", &s[..RESULT_CAP], s.len() - RESULT_CAP)
    }
}

/// Run a build/test command through the rootfs shell, capturing stdout+stderr+exit. `run` is how the
/// model compiles into /srv/build and executes the artifact; the exec split is enforced by the mount
/// plane, not here.
fn run_shell(cmd: &str) -> String {
    match std::process::Command::new("/bin/sh").arg("-c").arg(cmd).output() {
        Ok(out) => {
            let mut s = String::new();
            if !out.stdout.is_empty() {
                s.push_str("stdout:\n");
                s.push_str(&String::from_utf8_lossy(&out.stdout));
            }
            if !out.stderr.is_empty() {
                s.push_str("\nstderr:\n");
                s.push_str(&String::from_utf8_lossy(&out.stderr));
            }
            s.push_str(&format!("\nexit={}", out.status.code().unwrap_or(-1)));
            s
        }
        Err(e) => format!("ERROR spawning shell: {e}"),
    }
}

// ---- JSON request/response (tinyjson) ----------------------------------------------------------

/// Build the `chat/completions` request body from the transcript. Constructed as a
/// `JsonValue` and stringified so message content is JSON-escaped correctly (never hand-formatted).
fn build_request(model: &str, messages: &[(String, String)]) -> String {
    use std::collections::HashMap;
    let msgs: Vec<JsonValue> = messages
        .iter()
        .map(|(role, content)| {
            let mut m = HashMap::new();
            m.insert("role".to_string(), JsonValue::String(role.clone()));
            m.insert("content".to_string(), JsonValue::String(content.clone()));
            JsonValue::Object(m)
        })
        .collect();
    let mut root = HashMap::new();
    root.insert("model".to_string(), JsonValue::String(model.to_string()));
    root.insert("messages".to_string(), JsonValue::Array(msgs));
    root.insert("temperature".to_string(), JsonValue::Number(0.0));
    root.insert("stream".to_string(), JsonValue::Boolean(false));
    JsonValue::Object(root)
        .stringify()
        .expect("request JSON always serializes")
}

/// Pull `choices[0].message.content` out of a chat/completions reply. `None` on any shape mismatch.
fn extract_assistant_content(body: &str) -> Option<String> {
    let v: JsonValue = body.parse().ok()?;
    let obj = v.get::<std::collections::HashMap<String, JsonValue>>()?;
    let choices = obj.get("choices")?.get::<Vec<JsonValue>>()?;
    let first = choices.first()?;
    let msg = first.get::<std::collections::HashMap<String, JsonValue>>()?.get("message")?;
    let content = msg.get::<std::collections::HashMap<String, JsonValue>>()?.get("content")?;
    content.get::<String>().cloned()
}

/// A parsed tool-call.
struct ToolCall {
    tool: String,
    args: std::collections::HashMap<String, JsonValue>,
}

impl ToolCall {
    fn arg_str(&self, k: &str) -> Option<String> {
        self.args.get(k)?.get::<String>().cloned()
    }
    fn arg_bool(&self, k: &str) -> Option<bool> {
        self.args.get(k)?.get::<bool>().copied()
    }
}

/// Parse the model's reply content into a tool-call. The content SHOULD be exactly one JSON object,
/// but a real model may wrap it in prose or ```json fences — so we extract the first balanced-brace
/// object and parse THAT. `None` if no object parses or it lacks a string `tool`.
fn parse_tool_call(content: &str) -> Option<ToolCall> {
    let obj_src = extract_json_object(content)?;
    let v: JsonValue = obj_src.parse().ok()?;
    let obj = v.get::<std::collections::HashMap<String, JsonValue>>()?;
    let tool = obj.get("tool")?.get::<String>()?.clone();
    let args = match obj.get("args") {
        Some(a) => a.get::<std::collections::HashMap<String, JsonValue>>().cloned().unwrap_or_default(),
        None => std::collections::HashMap::new(),
    };
    Some(ToolCall { tool, args })
}

/// Return the first top-level `{…}` substring, brace-balanced and string-literal-aware (so a `}` inside
/// a JSON string does not end it early). `None` if there is no balanced object.
fn extract_json_object(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    let start = bytes.iter().position(|&b| b == b'{')?;
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    let mut i = start;
    while i < bytes.len() {
        let c = bytes[i];
        if in_str {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_str = false;
            }
        } else {
            match c {
                b'"' => in_str = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&s[start..=i]);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

// ---- HTTP/1.1 POST over plain TCP (v1 endpoint is plaintext) ------------------------------------

/// POST a JSON body to `url` and return the response body. Plain HTTP only (no TLS) — the v1 model
/// endpoint is a LAN plaintext service reached over the sealed egress. Resolution of the host goes
/// through the system resolver, which reads the `/etc/hosts` gatekeeperd pinned at construction.
fn http_post_json(url: &str, body: &str) -> Result<String, String> {
    let (host, port, path) = parse_url(url)?;
    let addr = format!("{host}:{port}");
    let mut stream = TcpStream::connect(&addr).map_err(|e| format!("connect {addr}: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(120)))
        .map_err(|e| format!("set timeout: {e}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| format!("set timeout: {e}"))?;

    let req = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).map_err(|e| format!("write: {e}"))?;
    stream.flush().ok();

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).map_err(|e| format!("read: {e}"))?;
    let text = String::from_utf8_lossy(&raw).into_owned();

    // Split headers from body on the blank line. We asked for Connection: close, so the body runs to
    // EOF; Content-Length is honored when present but read_to_end already has the whole payload.
    let body_start = text
        .find("\r\n\r\n")
        .map(|p| p + 4)
        .or_else(|| text.find("\n\n").map(|p| p + 2))
        .ok_or_else(|| "malformed HTTP response (no header/body split)".to_string())?;
    let status_ok = text
        .lines()
        .next()
        .map(|l| l.contains(" 200"))
        .unwrap_or(false);
    if !status_ok {
        let status_line = text.lines().next().unwrap_or("(no status)");
        return Err(format!("model HTTP status: {status_line}"));
    }
    Ok(text[body_start..].to_string())
}

/// Split `http://host:port/path` into (host, port, path). Plain-HTTP only; `https://` is rejected
/// (no in-sandbox TLS in v1). Default port 80, default path `/`.
fn parse_url(url: &str) -> Result<(String, u16, String), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("only http:// URLs are supported in v1 (got {url:?})"))?;
    let (authority, path) = match rest.find('/') {
        Some(p) => (&rest[..p], &rest[p..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rfind(':') {
        Some(p) => {
            let port = authority[p + 1..]
                .parse::<u16>()
                .map_err(|_| format!("bad port in {url:?}"))?;
            (authority[..p].to_string(), port)
        }
        None => (authority.to_string(), 80),
    };
    if host.is_empty() {
        return Err(format!("empty host in {url:?}"));
    }
    Ok((host, port, path.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_url_splits_host_port_path() {
        assert_eq!(
            parse_url("http://shrek-model:8100/v1/chat/completions").unwrap(),
            ("shrek-model".to_string(), 8100, "/v1/chat/completions".to_string())
        );
        assert_eq!(parse_url("http://h/").unwrap(), ("h".to_string(), 80, "/".to_string()));
        assert_eq!(parse_url("http://h").unwrap(), ("h".to_string(), 80, "/".to_string()));
    }

    #[test]
    fn parse_url_rejects_https_and_garbage() {
        assert!(parse_url("https://h:443/").is_err()); // no in-sandbox TLS in v1
        assert!(parse_url("ftp://h/").is_err());
        assert!(parse_url("http://:8100/").is_err()); // empty host
        assert!(parse_url("http://h:notaport/").is_err());
    }

    #[test]
    fn build_request_is_valid_escaped_json() {
        let msgs = vec![
            ("system".to_string(), "be good".to_string()),
            ("user".to_string(), "quote \" and newline \n here".to_string()),
        ];
        let body = build_request("local", &msgs);
        // Round-trips: the tricky content survives escaping and re-parses.
        let v: JsonValue = body.parse().expect("request must be valid JSON");
        let obj = v.get::<std::collections::HashMap<String, JsonValue>>().unwrap();
        assert_eq!(obj.get("model").unwrap().get::<String>().unwrap(), "local");
        let arr = obj.get("messages").unwrap().get::<Vec<JsonValue>>().unwrap();
        assert_eq!(arr.len(), 2);
        let m1 = arr[1].get::<std::collections::HashMap<String, JsonValue>>().unwrap();
        assert_eq!(m1.get("content").unwrap().get::<String>().unwrap(), "quote \" and newline \n here");
    }

    #[test]
    fn extract_content_pulls_choices0_message_content() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"hello"}}]}"#;
        assert_eq!(extract_assistant_content(body).as_deref(), Some("hello"));
        assert!(extract_assistant_content(r#"{"choices":[]}"#).is_none());
        assert!(extract_assistant_content("not json").is_none());
    }

    #[test]
    fn extract_json_object_is_balanced_and_string_aware() {
        // Pure object.
        assert_eq!(extract_json_object(r#"{"a":1}"#), Some(r#"{"a":1}"#));
        // Wrapped in prose + fences (real-model shape).
        let wrapped = "Sure!\n```json\n{\"tool\":\"done\",\"args\":{\"ok\":true}}\n```\n";
        assert_eq!(extract_json_object(wrapped), Some(r#"{"tool":"done","args":{"ok":true}}"#));
        // A brace inside a string must not end the object early.
        let tricky = r#"{"content":"has a } brace"}"#;
        assert_eq!(extract_json_object(tricky), Some(tricky));
        // No object.
        assert_eq!(extract_json_object("no braces here"), None);
    }

    #[test]
    fn parse_tool_call_reads_tool_and_args() {
        let c = parse_tool_call(r#"{"tool":"write_file","args":{"path":"a.c","content":"x"}}"#).unwrap();
        assert_eq!(c.tool, "write_file");
        assert_eq!(c.arg_str("path").as_deref(), Some("a.c"));
        assert_eq!(c.arg_str("content").as_deref(), Some("x"));

        let d = parse_tool_call(r#"prose {"tool":"done","args":{"ok":true,"summary":"fixed"}} more"#).unwrap();
        assert_eq!(d.tool, "done");
        assert_eq!(d.arg_bool("ok"), Some(true));
        assert_eq!(d.arg_str("summary").as_deref(), Some("fixed"));

        // args optional.
        let e = parse_tool_call(r#"{"tool":"list"}"#).unwrap();
        assert_eq!(e.tool, "list");
        assert!(e.arg_str("path").is_none());
    }

    #[test]
    fn parse_tool_call_fails_closed_on_garbage() {
        assert!(parse_tool_call("no json").is_none());
        assert!(parse_tool_call(r#"{"no_tool_key":1}"#).is_none());
        assert!(parse_tool_call(r#"{"tool":123}"#).is_none()); // tool must be a string
        assert!(parse_tool_call("{ unbalanced").is_none());
    }

    #[test]
    fn cap_truncates_oversized_results() {
        let big = "x".repeat(RESULT_CAP + 500);
        let capped = cap(&big);
        assert!(capped.len() < big.len());
        assert!(capped.contains("truncated"));
        // Small stays intact.
        assert_eq!(cap("small"), "small");
    }
}
