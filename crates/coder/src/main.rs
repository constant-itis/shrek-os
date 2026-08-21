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

const DEFAULT_MAX_STEPS: u32 = 8;
/// Cap on any single tool-result fed back into the transcript — keeps a runaway `run` from ballooning
/// the context (and the request body) without bound.
const RESULT_CAP: usize = 4000;
/// `max_tokens` for the Anthropic messages API (a required field there; the chat/completions path
/// leaves generation length to the server). Generous enough for a full rewritten file in one reply.
const ANTHROPIC_MAX_TOKENS: f64 = 8192.0;

/// The model PROVIDER — the ONLY thing that varies between a local Qwen and a hosted Claude session:
/// which sealed egress dst the box was constructed with, and which wire the adapter speaks. It adds
/// NO authority (the sealed egress-profile ∩ grants + the T2 wall are unchanged) and holds NO secret
/// (a hosted key lives ONLY in the broker-side proxy — crates/model-proxy — never in this box). See
/// docs/phase6-slice3-provider-abstraction.md. Deliberately just two concrete variants, not a plugin
/// framework: the seam is exactly what these two working implementations force.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Provider {
    /// Local LAN model, plaintext `chat/completions` direct to `shrek-model` (`model-local` egress).
    Local,
    /// Hosted Anthropic model, `messages` API. The box speaks PLAINTEXT to the broker proxy
    /// (`shrek-model-proxy`, `model-anthropic` egress); the proxy injects the key + does TLS to
    /// Anthropic. No key and no TLS ever enter this box.
    Anthropic,
}

impl Provider {
    fn parse(s: &str) -> Option<Provider> {
        match s {
            "local" => Some(Provider::Local),
            "anthropic" => Some(Provider::Anthropic),
            _ => None,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Provider::Local => "local",
            Provider::Anthropic => "anthropic",
        }
    }
    /// The default endpoint the box dials — ALWAYS plaintext http:// (no in-box TLS). For Anthropic
    /// that is the broker proxy, NOT api.anthropic.com; the proxy is the sole TLS speaker.
    fn default_model_url(self) -> &'static str {
        match self {
            Provider::Local => "http://shrek-model:8100/v1/chat/completions",
            Provider::Anthropic => "http://shrek-model-proxy:8200/v1/messages",
        }
    }
    /// The default model-name field. Overridable with `--model` / `SHREK_MODEL` (the LIVE smoke picks
    /// the exact id). The canned oracle ignores it.
    fn default_model(self) -> &'static str {
        match self {
            Provider::Local => "local",
            Provider::Anthropic => "claude-sonnet-5",
        }
    }
    /// The sealed egress profile a `shrek run` session for this provider must be constructed with — the
    /// box↔endpoint pairing the wall enforces. Informational here (gatekeeperd owns egress); surfaced so
    /// the CODER-START line records the box's contract.
    fn egress_profile(self) -> &'static str {
        match self {
            Provider::Local => "model-local",
            Provider::Anthropic => "model-anthropic",
        }
    }
}

fn main() {
    std::process::exit(real_main());
}

fn real_main() -> i32 {
    let mut task: Option<String> = None;
    // Provider first (env is the baseline; --provider overrides). Explicit --model-url/--model
    // overrides win over the provider defaults, so collect them as options and resolve AFTER the loop
    // (argv order-independent: --provider may follow --model-url).
    let mut provider_arg = std::env::var("SHREK_PROVIDER").ok();
    let mut model_url_override = std::env::var("SHREK_MODEL_URL").ok();
    let mut model_override = std::env::var("SHREK_MODEL").ok();
    let mut max_steps = DEFAULT_MAX_STEPS;
    let mut live = false;

    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--task" => { i += 1; task = argv.get(i).cloned(); }
            "--provider" => { i += 1; provider_arg = argv.get(i).cloned(); }
            "--model-url" => { i += 1; model_url_override = argv.get(i).cloned(); }
            "--model" => { i += 1; model_override = argv.get(i).cloned(); }
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

    // Resolve the provider FAIL-CLOSED: an unknown provider name is a hard error, never a silent
    // fallback to a different backend (which could mismatch the sealed egress the session was built
    // with). Default is `local` — the frozen slice-2 behavior is unchanged when nothing is specified.
    let provider = match provider_arg.as_deref() {
        None | Some("") => Provider::Local,
        Some(s) => match Provider::parse(s) {
            Some(p) => p,
            None => { eprintln!("coder: unknown --provider {s:?}; valid: local, anthropic"); return 2; }
        },
    };
    let model_url = model_url_override.unwrap_or_else(|| provider.default_model_url().into());
    let model = model_override.unwrap_or_else(|| provider.default_model().into());

    let Some(task) = task else {
        eprintln!("coder: --task \"<one-line task>\" is required");
        usage();
        return 2;
    };

    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| ".".into());
    println!(
        "CODER-START provider={} egress={} task={:?} model_url={} model={} max_steps={} live={} cwd={}",
        provider.label(), provider.egress_profile(), task, model_url, model, max_steps, live, cwd
    );

    run_loop(provider, &task, &model_url, &model, max_steps)
}

fn usage() {
    eprintln!("coder — the coding-agent workload for `shrek run` (Phase-6 slice-2)");
    eprintln!();
    eprintln!("  coder --task \"<one-line task>\" [opts]");
    eprintln!("    --provider NAME   local | anthropic (default local; env SHREK_PROVIDER).");
    eprintln!("                      local = plaintext chat/completions to shrek-model (model-local egress).");
    eprintln!("                      anthropic = messages API, plaintext to the broker proxy shrek-model-proxy");
    eprintln!("                      (model-anthropic egress); the proxy holds the key + does TLS. No secret in-box.");
    eprintln!("    --model-url URL   override the endpoint the box dials (plaintext http:// only; env SHREK_MODEL_URL)");
    eprintln!("    --model NAME      model name field (env SHREK_MODEL; default per provider)");
    eprintln!("    --max-steps N     hard cap on loop iterations (default {DEFAULT_MAX_STEPS}); tripping it fails closed");
    eprintln!("    --live            informational marker for a real-model smoke (no behavior change)");
    eprintln!();
    eprintln!("  Runs inside the T2 box: reads/writes the CWD project grant, builds in /srv/build,");
    eprintln!("  reaches only the sealed egress the session was constructed with. Bounded to one task.");
}

/// The agent loop. Returns the process exit code: 0 = done ok, 1 = done not-ok, 3 = step cap
/// (fail-closed), 4 = model/transport/parse failure (fail-closed).
fn run_loop(provider: Provider, task: &str, model_url: &str, model: &str, max_steps: u32) -> i32 {
    // Swamp slice-2: this session can query the swamp IFF gatekeeperd injected `SHREK_SESSION` — which it
    // does ONLY for a swamp-capable construct (one whose sealed egress grants `swamp-query`). That same
    // env is also the handle the broker will match against its `cont_ip→session` binding, so its presence
    // is the exact, single signal to arm the `swamp_find` tool. A model-only session lacks it → the tool
    // is neither advertised nor accepted, and the system prompt is byte-for-byte the pre-swamp prompt.
    let swamp_enabled = std::env::var("SHREK_SESSION").ok().filter(|v| !v.is_empty()).is_some();
    let swamp_url = std::env::var("SHREK_SWAMP_URL")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "http://shrek-swamp-broker:8400/".to_string());

    let mut messages: Vec<(String, String)> = vec![
        ("system".into(), system_prompt(swamp_enabled)),
        ("user".into(), initial_user_message(task)),
    ];

    // One opaque session handle per coder run (slice-7). The broker maps it to a real native CLI session
    // and forwards only the new tail turn each step; we keep sending the full transcript (stateless
    // client), and the broker collapses it. The handle is opaque and carries no authority. For a
    // swamp-capable session this resolves to the gatekeeper-injected `SHREK_SESSION` (== the bound
    // handle), so `swamp_find` presents to the swamp broker the SAME identity gatekeeperd bound.
    let session = mint_session_handle();

    for step in 1..=max_steps {
        println!("CODER-STEP {step}");

        let request = build_request(provider, model, &messages);
        let reply_body = match http_post_json(model_url, &request, &session) {
            Ok(b) => b,
            Err(e) => { eprintln!("CODER-ERROR transport: {e}"); return 4; }
        };
        let content = match extract_assistant_content(provider, &reply_body) {
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
            // Swamp slice-2: search the project's swamp through the broker-routed sealed `swamp-query`
            // egress, carrying this session's handle. gatekeeperd bound `cont_ip→handle` and wrote the
            // handle-keyed authority record, so the broker returns EXACTLY this session's authorized
            // projection — never a wall hole, never wider than the session's filesystem grants.
            "swamp_find" if swamp_enabled => {
                let q = call.arg_str("q").unwrap_or_default();
                let result = if q.trim().is_empty() {
                    "ERROR swamp_find requires a non-empty \"q\" (search terms).".to_string()
                } else {
                    let body = build_swamp_body(&q, call.arg_str("intent").as_deref(), call.arg_str("scope").as_deref(), call.arg_u64("limit"));
                    match http_post_json(&swamp_url, &body, &session) {
                        Ok(raw) => format_swamp_result(&raw),
                        Err(e) => format!("ERROR swamp_find transport: {e}"),
                    }
                };
                println!("CODER-TOOL swamp_find q={q:?}");
                emit_result(&result);
                messages.push(("user".into(), tool_result(&result)));
            }
            other => {
                let valid = if swamp_enabled {
                    "read_file, write_file, run, swamp_find, done"
                } else {
                    "read_file, write_file, run, done"
                };
                let result = format!("ERROR unknown tool {other:?}; valid tools: {valid}");
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

fn system_prompt(swamp_enabled: bool) -> String {
    // The whole tool surface. The model must reply with exactly ONE JSON object and nothing else. The
    // `swamp_find` line is present ONLY for a swamp-capable session, so a model-only session's prompt is
    // byte-for-byte the pre-swamp prompt.
    let swamp_shape = if swamp_enabled {
        "       {\"tool\":\"swamp_find\",\"args\":{\"q\":\"error handling in the parser\",\"limit\":20}}\n"
    } else {
        ""
    };
    let swamp_rule = if swamp_enabled {
        " Use swamp_find to search the project's indexed \
         knowledge for relevant code/notes by meaning (args: q required; optional intent search|discover, \
         scope, limit) — it returns only files within this session's authority."
    } else {
        ""
    };
    format!(
        "You are a coding agent running inside a locked sandbox. You can only touch the current project \
         directory and a build area at /srv/build. Work on ONE task, then finish.\n\n\
         Reply with EXACTLY ONE JSON object and no other text. Shape:\n\
           {{\"tool\":\"read_file\",\"args\":{{\"path\":\"buggy.c\"}}}}\n\
           {{\"tool\":\"write_file\",\"args\":{{\"path\":\"buggy.c\",\"content\":\"...full new file...\"}}}}\n\
           {{\"tool\":\"run\",\"args\":{{\"cmd\":\"tcc -nostdlib -static -o /srv/build/prog buggy.c && /srv/build/prog; echo exit=$?\"}}}}\n\
        {swamp_shape}\
           {{\"tool\":\"done\",\"args\":{{\"ok\":true,\"summary\":\"what you changed and the test result\"}}}}\n\n\
         Rules: write_file replaces the whole file. Compile into /srv/build (the only place a binary can \
         run); the project dir is non-executable. Verify by running the program before you call done. \
         Call done with ok:true only once the task's success condition is observed.{swamp_rule}"
    )
}

/// Build the swamp-broker request body (line-text, the swampd wire idiom). `q` is REQUIRED; `intent`,
/// `scope`, `limit` are optional (the broker defaults them: search / whole-authority / 50). The `q`
/// free-text is flattened to a single line — any control byte would corrupt the broker's line wire, so
/// it is replaced with a space (the model's search terms never legitimately contain control chars).
fn build_swamp_body(q: &str, intent: Option<&str>, scope: Option<&str>, limit: Option<u64>) -> String {
    let mut b = String::new();
    if let Some(i) = intent {
        b.push_str(&format!("intent {i}\n"));
    }
    if let Some(s) = scope {
        b.push_str(&format!("scope {s}\n"));
    }
    if let Some(n) = limit {
        b.push_str(&format!("limit {n}\n"));
    }
    let q1: String = q.chars().map(|c| if (c as u32) < 0x20 { ' ' } else { c }).collect();
    b.push_str(&format!("q {}\n", q1.trim()));
    b
}

/// Render the swamp broker's `RESULT n / freshness x / semantic y / hit path\tsnippet / END` wire into a
/// compact, model-readable tool result. A missing/garbled `RESULT` header is surfaced as an error; a
/// fail-closed empty (`RESULT 0`) reads as an honest "nothing matched" (indistinguishable from a denied
/// query, by design). Two capability signals are surfaced EXPLICITLY:
///   * `freshness` (slice-3): when not `fresh`, a caution so the model never treats a miss as proof of
///     absence — a stale/unknown index may simply be behind the filesystem.
///   * `semantic` (slice-4): when not `available`, a note that similarity ranking was NOT applied, so the
///     results are lexical FTS only — a paraphrase/concept miss is "no semantic tier today," not absence.
/// The broker relays swampd's headers verbatim and never rewrites them; `unknown`/`unavailable` mean the
/// broker reached no healthy index (deny / swampd down) or no provider is configured. A pre-slice-3/-4
/// index omits these lines → treated as fresh / (semantic not-in-play), no note.
fn format_swamp_result(raw: &str) -> String {
    let mut lines = raw.lines();
    let n = lines.next().and_then(|l| l.strip_prefix("RESULT ")).and_then(|s| s.trim().parse::<usize>().ok());
    // Scan the body once for the freshness/semantic headers and the hits (all live between RESULT and
    // END). A pre-slice index omits these header lines entirely → treated as fresh / no semantic note.
    let mut freshness: Option<&str> = None;
    let mut semantic: Option<&str> = None;
    let mut hit_lines: Vec<String> = Vec::new();
    for line in lines {
        if line == "END" {
            break;
        }
        if let Some(f) = line.strip_prefix("freshness ") {
            freshness = Some(f.trim());
            continue;
        }
        if let Some(s) = line.strip_prefix("semantic ") {
            semantic = Some(s.trim());
            continue;
        }
        if let Some(rest) = line.strip_prefix("hit ") {
            let (path, snippet) = rest.split_once('\t').unwrap_or((rest, ""));
            hit_lines.push(format!("- {path}: {snippet}"));
        }
    }
    let mut caution = match freshness {
        None | Some("fresh") => String::new(),
        Some(state) => format!(
            "NOTE swamp_find index freshness={state}: the index may be behind the filesystem — treat a \
             miss as 'not found in a possibly-stale index', NOT proof a file/term is absent.\n"
        ),
    };
    // Semantic-availability note (additive; capability, not correctness). `available` (or absent) = no
    // note. Anything else = ranking is lexical-only right now.
    if let Some(state) = semantic {
        if state != "available" {
            caution.push_str(&format!(
                "NOTE swamp_find semantic={state}: similarity ranking was NOT applied — results are \
                 lexical (keyword) only; a concept/paraphrase miss is 'no semantic tier now', not absence.\n"
            ));
        }
    }
    match n {
        None => format!("ERROR swamp_find: malformed or failed result:\n{}", cap(raw)),
        Some(0) => {
            format!("{caution}OK swamp_find: 0 hits (nothing in this session's authority matched).")
        }
        Some(count) => {
            let mut out = format!("{caution}OK swamp_find: {count} hit(s):\n");
            for h in &hit_lines {
                out.push_str(h);
                out.push('\n');
            }
            out
        }
    }
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

/// Build the request body for the selected provider. This + [`extract_assistant_content`] are the
/// ENTIRE provider seam: the transcript, the tool loop, and the grants are provider-agnostic. Both
/// are constructed as a `JsonValue` and stringified so content is JSON-escaped correctly.
fn build_request(provider: Provider, model: &str, messages: &[(String, String)]) -> String {
    match provider {
        Provider::Local => build_request_chat(model, messages),
        Provider::Anthropic => build_request_anthropic(model, messages),
    }
}

/// The `chat/completions` wire: a flat `messages` array (system role included inline).
fn build_request_chat(model: &str, messages: &[(String, String)]) -> String {
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

/// Anthropic `messages` API: `system` is a TOP-LEVEL field (not a message role), the `messages` array
/// carries only user/assistant turns, and `max_tokens` is REQUIRED. Our transcript is
/// `[system, user, (assistant, user)*]`, so lifting the system turn out leaves a valid user-first
/// alternating array. The Authorization/`x-api-key` header is NOT set here — the broker proxy injects
/// it; this body is plaintext and secret-free.
fn build_request_anthropic(model: &str, messages: &[(String, String)]) -> String {
    use std::collections::HashMap;
    let mut system = String::new();
    let mut msgs: Vec<JsonValue> = Vec::new();
    for (role, content) in messages {
        if role == "system" {
            if !system.is_empty() {
                system.push_str("\n\n");
            }
            system.push_str(content);
        } else {
            let mut m = HashMap::new();
            m.insert("role".to_string(), JsonValue::String(role.clone()));
            m.insert("content".to_string(), JsonValue::String(content.clone()));
            msgs.push(JsonValue::Object(m));
        }
    }
    let mut root = HashMap::new();
    root.insert("model".to_string(), JsonValue::String(model.to_string()));
    root.insert("max_tokens".to_string(), JsonValue::Number(ANTHROPIC_MAX_TOKENS));
    if !system.is_empty() {
        root.insert("system".to_string(), JsonValue::String(system));
    }
    root.insert("messages".to_string(), JsonValue::Array(msgs));
    root.insert("temperature".to_string(), JsonValue::Number(0.0));
    JsonValue::Object(root)
        .stringify()
        .expect("request JSON always serializes")
}

/// Extract the assistant's reply text (the tool-call JSON string) for the selected provider.
fn extract_assistant_content(provider: Provider, body: &str) -> Option<String> {
    match provider {
        Provider::Local => extract_chat_content(body),
        Provider::Anthropic => extract_anthropic_content(body),
    }
}

/// Pull `choices[0].message.content` out of a chat/completions reply. `None` on any shape mismatch.
fn extract_chat_content(body: &str) -> Option<String> {
    let v: JsonValue = body.parse().ok()?;
    let obj = v.get::<std::collections::HashMap<String, JsonValue>>()?;
    let choices = obj.get("choices")?.get::<Vec<JsonValue>>()?;
    let first = choices.first()?;
    let msg = first.get::<std::collections::HashMap<String, JsonValue>>()?.get("message")?;
    let content = msg.get::<std::collections::HashMap<String, JsonValue>>()?.get("content")?;
    content.get::<String>().cloned()
}

/// Concatenate the `text` blocks of an Anthropic `messages` reply (`content: [{type:text,text:…}]`).
/// The tool-call JSON lives in that text, exactly as the chat path's `message.content` does. `None`
/// on any shape mismatch (fails closed to the loop's "no assistant content" path).
fn extract_anthropic_content(body: &str) -> Option<String> {
    use std::collections::HashMap;
    let v: JsonValue = body.parse().ok()?;
    let obj = v.get::<HashMap<String, JsonValue>>()?;
    let blocks = obj.get("content")?.get::<Vec<JsonValue>>()?;
    let mut out = String::new();
    for block in blocks {
        let b = match block.get::<HashMap<String, JsonValue>>() {
            Some(b) => b,
            None => continue,
        };
        let is_text = b
            .get("type")
            .and_then(|t| t.get::<String>())
            .map(|s| s == "text")
            .unwrap_or(false);
        if is_text {
            if let Some(t) = b.get("text").and_then(|t| t.get::<String>()) {
                out.push_str(t);
            }
        }
    }
    if out.is_empty() { None } else { Some(out) }
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
    /// A non-negative integer arg. tinyjson numbers are `f64`; also accept a stringified number so a
    /// model that emits `"limit":"20"` still works. `None` if absent, negative, or unparsable.
    fn arg_u64(&self, k: &str) -> Option<u64> {
        if let Some(n) = self.args.get(k).and_then(|v| v.get::<f64>()) {
            if *n >= 0.0 && n.is_finite() {
                return Some(*n as u64);
            }
        }
        self.arg_str(k).and_then(|s| s.trim().parse::<u64>().ok())
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

/// Mint an opaque per-session handle: `SHREK_SESSION` (validated) if the launcher set one, else 16 bytes
/// of `/dev/urandom` hex-encoded. Opaque, unique, no authority — the broker uses it ONLY to bind
/// conversational state. Falls back to a pid-derived value if urandom is unreadable (still opaque).
fn mint_session_handle() -> String {
    if let Ok(s) = std::env::var("SHREK_SESSION") {
        let s = s.trim();
        let ok = !s.is_empty()
            && s.len() <= 128
            && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if ok {
            return s.to_string();
        }
    }
    let mut b = [0u8; 16];
    let filled = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .is_ok();
    if !filled {
        let pid = std::process::id();
        for (i, x) in b.iter_mut().enumerate() {
            *x = (pid >> ((i % 4) * 8)) as u8 ^ (i as u8).wrapping_mul(31);
        }
    }
    let mut s = String::with_capacity(32);
    for byte in b {
        use std::fmt::Write as _;
        let _ = write!(s, "{byte:02x}");
    }
    s
}

// ---- HTTP/1.1 POST over plain TCP (v1 endpoint is plaintext) ------------------------------------

/// POST a JSON body to `url` and return the response body. Plain HTTP only (no TLS) — the v1 model
/// endpoint is a LAN plaintext service reached over the sealed egress. Resolution of the host goes
/// through the system resolver, which reads the `/etc/hosts` gatekeeperd pinned at construction.
///
/// `session` is the opaque per-session handle sent as `X-Shrek-Session` (Phase-6 slice-7): it lets the
/// broker bind this call to a REAL native CLI session and forward only the new tail turn. It is opaque
/// and carries no authority; an empty string omits the header (stateless, slice-6 behavior).
fn http_post_json(url: &str, body: &str, session: &str) -> Result<String, String> {
    let (host, port, path) = parse_url(url)?;
    let addr = format!("{host}:{port}");
    let mut stream = TcpStream::connect(&addr).map_err(|e| format!("connect {addr}: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(120)))
        .map_err(|e| format!("set timeout: {e}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| format!("set timeout: {e}"))?;

    let session_header = if session.is_empty() {
        String::new()
    } else {
        format!("X-Shrek-Session: {session}\r\n")
    };
    let req = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Content-Type: application/json\r\n\
         {session_header}\
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
        let body = build_request(Provider::Local, "local", &msgs);
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
        assert_eq!(extract_assistant_content(Provider::Local, body).as_deref(), Some("hello"));
        assert!(extract_assistant_content(Provider::Local, r#"{"choices":[]}"#).is_none());
        assert!(extract_assistant_content(Provider::Local, "not json").is_none());
    }

    #[test]
    fn provider_parse_is_strict() {
        assert_eq!(Provider::parse("local"), Some(Provider::Local));
        assert_eq!(Provider::parse("anthropic"), Some(Provider::Anthropic));
        // Unknown ⇒ None (real_main turns this into a hard error, never a silent backend swap).
        assert_eq!(Provider::parse("bogus"), None);
        assert_eq!(Provider::parse(""), None);
        assert_eq!(Provider::parse("Anthropic"), None); // case-sensitive
        // Each provider names its own sealed egress profile + plaintext (never https://) endpoint.
        assert_eq!(Provider::Anthropic.egress_profile(), "model-anthropic");
        assert!(Provider::Anthropic.default_model_url().starts_with("http://"));
        assert!(Provider::Anthropic.default_model_url().contains("shrek-model-proxy"));
    }

    #[test]
    fn build_request_anthropic_lifts_system_and_requires_max_tokens() {
        let msgs = vec![
            ("system".to_string(), "sys A".to_string()),
            ("user".to_string(), "u1".to_string()),
            ("assistant".to_string(), "a1".to_string()),
            ("user".to_string(), "u2 with \" quote".to_string()),
        ];
        let body = build_request_anthropic("claude-sonnet-5", &msgs);
        let v: JsonValue = body.parse().expect("anthropic request must be valid JSON");
        let obj = v.get::<std::collections::HashMap<String, JsonValue>>().unwrap();
        // system lifted to a TOP-LEVEL field, not a message role.
        assert_eq!(obj.get("system").unwrap().get::<String>().unwrap(), "sys A");
        assert!(obj.contains_key("max_tokens"), "messages API requires max_tokens");
        let arr = obj.get("messages").unwrap().get::<Vec<JsonValue>>().unwrap();
        // Only the non-system turns remain, user-first + alternating.
        assert_eq!(arr.len(), 3);
        let first = arr[0].get::<std::collections::HashMap<String, JsonValue>>().unwrap();
        assert_eq!(first.get("role").unwrap().get::<String>().unwrap(), "user");
        let last = arr[2].get::<std::collections::HashMap<String, JsonValue>>().unwrap();
        assert_eq!(last.get("content").unwrap().get::<String>().unwrap(), "u2 with \" quote");
    }

    #[test]
    fn extract_anthropic_content_concatenates_text_blocks() {
        let body = r#"{"id":"msg_1","type":"message","role":"assistant",
            "content":[{"type":"text","text":"{\"tool\":\"done\",\"args\":{\"ok\":true}}"}],
            "stop_reason":"end_turn"}"#;
        assert_eq!(
            extract_anthropic_content(body).as_deref(),
            Some(r#"{"tool":"done","args":{"ok":true}}"#)
        );
        // Two text blocks concatenate; non-text blocks are ignored.
        let two = r#"{"content":[{"type":"text","text":"ab"},{"type":"text","text":"cd"}]}"#;
        assert_eq!(extract_anthropic_content(two).as_deref(), Some("abcd"));
        // Shape mismatches ⇒ None (fail closed).
        assert!(extract_anthropic_content(r#"{"content":[]}"#).is_none());
        assert!(extract_anthropic_content(r#"{"choices":[]}"#).is_none()); // chat shape ≠ messages shape
        assert!(extract_anthropic_content("not json").is_none());
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

    #[test]
    fn swamp_body_q_only_defaults_the_rest() {
        // q alone: no intent/scope/limit lines (the broker defaults them), q last.
        assert_eq!(build_swamp_body("parser errors", None, None, None), "q parser errors\n");
    }

    #[test]
    fn swamp_body_emits_all_fields_and_flattens_control_chars() {
        let b = build_swamp_body("foo\nbar\tbaz", Some("discover"), Some("/srv/project"), Some(20));
        assert_eq!(b, "intent discover\nscope /srv/project\nlimit 20\nq foo bar baz\n");
        // The q line is single: exactly one `q ` and no stray newlines that could inject a wire line.
        assert_eq!(b.matches("\nq ").count() + b.starts_with("q ") as usize, 1);
    }

    #[test]
    fn swamp_result_formats_hits_zero_and_malformed() {
        // Pre-slice-3 wire (no freshness line) still parses; absent freshness ⇒ treated as fresh.
        let hits = "RESULT 2\nhit /srv/project/a.c\tint main()\nhit /srv/project/b.c\treturn 0\nEND\n";
        let f = format_swamp_result(hits);
        assert!(f.contains("2 hit(s)") && f.contains("- /srv/project/a.c: int main()") && f.contains("- /srv/project/b.c: return 0"));
        assert!(!f.contains("NOTE swamp_find index freshness"), "no caution when freshness is absent/fresh");
        assert!(format_swamp_result("RESULT 0\nEND\n").contains("0 hits"));
        // A denied/empty query is indistinguishable from a legitimate no-match (both RESULT 0).
        assert!(format_swamp_result("garbage").starts_with("ERROR swamp_find"));
    }

    #[test]
    fn swamp_result_surfaces_freshness_state() {
        // Slice-3 wire: a `freshness` header after RESULT is parsed, not shown as a hit.
        let fresh = "RESULT 1\nfreshness fresh\nhit /srv/project/a.c\tint main()\nEND\n";
        let ff = format_swamp_result(fresh);
        assert!(ff.contains("1 hit(s)") && ff.contains("- /srv/project/a.c: int main()"));
        assert!(!ff.contains("freshness=fresh"), "fresh state adds no caution");
        assert!(!ff.contains("- freshness"), "the freshness line is never rendered as a hit");

        // STALE: a caution is prefixed so the model does not read a miss as proof of absence.
        let stale = "RESULT 0\nfreshness stale\nEND\n";
        let fs = format_swamp_result(stale);
        assert!(fs.starts_with("NOTE swamp_find index freshness=stale"));
        assert!(fs.contains("0 hits"));
        assert!(fs.contains("NOT proof"));

        // UNKNOWN (broker fail-closed / swampd down): same conservative caution.
        let unknown = "RESULT 0\nfreshness unknown\nEND\n";
        assert!(format_swamp_result(unknown).starts_with("NOTE swamp_find index freshness=unknown"));
    }

    #[test]
    fn swamp_result_surfaces_semantic_availability() {
        // Slice-4 wire: a `semantic` header rides after freshness. `available` adds NO note.
        let avail = "RESULT 1\nfreshness fresh\nsemantic available\nhit /srv/p/a.c\tint main()\nEND\n";
        let fa = format_swamp_result(avail);
        assert!(fa.contains("1 hit(s)") && fa.contains("- /srv/p/a.c: int main()"));
        assert!(!fa.contains("semantic="), "available adds no note");
        assert!(!fa.contains("- semantic"), "the semantic line is never rendered as a hit");

        // UNAVAILABLE: a capability note is surfaced (results are lexical only), WITHOUT blocking hits.
        let unavail = "RESULT 1\nfreshness fresh\nsemantic unavailable\nhit /srv/p/a.c\tint main()\nEND\n";
        let fu = format_swamp_result(unavail);
        assert!(fu.contains("NOTE swamp_find semantic=unavailable"));
        assert!(fu.contains("lexical (keyword) only"));
        assert!(fu.contains("1 hit(s)"), "the note is additive — hits still render");

        // Absent semantic line (pre-slice-4 swampd) ⇒ no semantic note (backward compatible).
        let old = "RESULT 1\nfreshness fresh\nhit /srv/p/a.c\tint main()\nEND\n";
        assert!(!format_swamp_result(old).contains("semantic="));

        // Both cautions can stack: stale index AND semantic unavailable.
        let both = "RESULT 0\nfreshness stale\nsemantic unavailable\nEND\n";
        let fb = format_swamp_result(both);
        assert!(fb.contains("freshness=stale") && fb.contains("semantic=unavailable"));
    }

    #[test]
    fn arg_u64_reads_number_or_stringified_number() {
        let c = parse_tool_call(r#"{"tool":"swamp_find","args":{"q":"x","limit":20}}"#).unwrap();
        assert_eq!(c.arg_u64("limit"), Some(20));
        let c2 = parse_tool_call(r#"{"tool":"swamp_find","args":{"q":"x","limit":"35"}}"#).unwrap();
        assert_eq!(c2.arg_u64("limit"), Some(35));
        assert_eq!(c2.arg_u64("absent"), None);
    }

    #[test]
    fn system_prompt_advertises_swamp_find_only_when_enabled() {
        // Swamp-capable session: swamp_find is in the tool surface.
        assert!(system_prompt(true).contains("swamp_find"));
        // Model-only session: the prompt is byte-for-byte the pre-swamp prompt (no swamp mention).
        let off = system_prompt(false);
        assert!(!off.contains("swamp_find") && !off.to_lowercase().contains("swamp"));
    }
}
