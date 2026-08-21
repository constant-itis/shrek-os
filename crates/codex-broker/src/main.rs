//! codex-broker — the broker-side SECOND subscription-model provider for the coder (Phase-6 slice-6,
//! docs/phase6-slice6-codex-cli-broker.md, security-model.md §7). A SIBLING of `claude-broker` that
//! proves the broker/provider pattern GENERALIZES beyond Claude: same messages-API wire in, same
//! sealed-one-destination egress out, same login/health/availability shape — a different CLI behind it.
//!
//! WHY IT EXISTS. "Sign in with Codex" for Shrek means: log into the official `codex` CLI ONCE, then
//! Shrek invokes the logged-in CLI. The CLI owns its own subscription credential — Shrek never sees,
//! stores, or manages the token. This broker is the seam that lets the sandboxed coder drive that CLI
//! without any of it entering the box, and WITHOUT any change to `crates/coder`.
//!
//! HOW IT FITS — zero change to the coder or to the api-key/claude paths. The coder speaks the SAME
//! plaintext messages-API wire it already speaks to the api-key proxy and the claude broker, but over
//! the sealed one-destination `model-codex-cli` egress. This broker ACCEPTS that request, ADAPTS it into
//! a `codex exec` invocation broker-side, and WRAPS the reply back into the messages-API shape the
//! coder's extractor already parses.
//!
//! THE ONE THING THAT DIFFERS FROM claude-broker. The `codex` CLI is an AGENTIC EXECUTOR — it has its
//! own shell/exec tool surface and its own sandbox, unlike `claude -p` (a plain completion). Two guards,
//! layered, keep it a pure per-turn text oracle that can never leak the credential:
//!   1. HOST CONFINEMENT (authoritative). The spawned `codex` runs inside an unprivileged `bubblewrap`
//!      STERILE VIEW: only the node/codex runtime (ro) + a ro-bound `auth.json` + ONE rw scratch file
//!      are visible. No project, no $HOME, no vault, no writable host path. `--clearenv` + `--new-session`.
//!   2. TOOL-DISABLE (the credential guard). The model is offered NO tools — an empty `[tools]` allowlist
//!      (`tools.default_tools_enabled=false`, `tools.enabled_tools=[]`) plus `--disable shell_tool
//!      /unified_exec/view_image`. So the model cannot invoke a reader to `cat` the ro-bound `auth.json`
//!      and exfiltrate it through the reply. `-s read-only -a never --ephemeral` are defense-in-depth.
//! The deterministic oracle PROVES the tools array codex sends is empty (request capture), so the guard
//! is proven, not trusted.
//!
//! WHAT IT IS NOT. Not a control plane, not sealed into the appliance image (it shells the host's
//! `codex` under the host's `bwrap`), not a credential handler. It reads no token and never calls
//! `codex login status` / `codex doctor` (which read cached state and lie — #1567): the ONLY authority
//! on token health is a real `codex exec` round-trip, whose error IS surfaced fail-closed here.
//!
//! ARGV SAFETY. Both `bwrap` and `codex` are invoked with a std::process argv VECTOR, never a shell
//! string. The caller's model id is mapped through a fixed broker-side ALLOWLIST to a `&'static` value —
//! a raw caller string never becomes a command argument. Prompt/system text are single argv elements.
//!
//! Config (all env; broker-side):
//!   SHREK_CODEX_BROKER_LISTEN   plaintext listen addr for the box    (default 127.0.0.1:8301)
//!   SHREK_CODEX_BIN             path/name of the `codex` binary       (default `codex`)
//!   SHREK_BWRAP_BIN             path/name of the `bwrap` binary        (default `bwrap`)
//!   SHREK_CODEX_DEFAULT_MODEL   model alias when the request's id is absent/unmapped (default `gpt-5.5`)
//!   SHREK_CODEX_HOME            the REAL codex home whose auth.json is ro-bound (default $HOME/.codex)
//!   SHREK_CODEX_RUNTIME_DIR     runtime tree to ro-bind (node+codex); derived from the bin if unset
//!   SHREK_CODEX_STATE_DIR       broker-side dir for the availability breadcrumb
//!                               (default $HOME/.local/state/shrek-codex-cli)
//! Oracle-only passthroughs (NEVER set in production; forwarded into the sterile view when present so a
//! canned LOCAL endpoint + STUB key can capture the request without the real subscription credential):
//!   OPENAI_BASE_URL, OPENAI_API_KEY
//!
//! Subcommands:
//!   (none) | serve   accept box requests and adapt them to `codex exec` — the serve loop.
//!   login            run the official `codex login` in the OPERATOR'S REAL TERMINAL (the CLI owns ALL
//!                    credential state), then verify with ONE real `codex exec` round-trip and record an
//!                    AUDIT-ONLY availability breadcrumb. No token is ever read, parsed, or stored.
//!   health           run just the `codex exec` round-trip probe and update the breadcrumb.

use std::fs::{self, File, OpenOptions};
use std::io::{IsTerminal, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tinyjson::JsonValue;

const DEFAULT_LISTEN: &str = "127.0.0.1:8301";
const DEFAULT_CODEX_BIN: &str = "codex";
const DEFAULT_BWRAP_BIN: &str = "bwrap";
const DEFAULT_MODEL: &str = "gpt-5.5";
/// Guard: refuse to read an absurdly large request so a broken peer cannot OOM the broker.
const MAX_BODY: usize = 16 * 1024 * 1024;

// Fixed paths INSIDE the sterile view. The host binds map onto these; the sandbox never sees host paths.
const SANDBOX_CODEX_HOME: &str = "/codexhome";
const SANDBOX_AUTH: &str = "/codexhome/auth.json";
const SANDBOX_WORK: &str = "/work";
const SANDBOX_OUT: &str = "/work/last.txt";

fn main() {
    std::process::exit(dispatch());
}

/// The broker-side confinement + invocation config, resolved once from env. All paths are broker-side.
struct Confine {
    bwrap_bin: String,
    codex_bin: String,
    /// The node/codex runtime tree to ro-bind so `codex` can run (node + the codex package + resources).
    runtime_dir: PathBuf,
    /// The REAL codex home on the host; ONLY its `auth.json` is ro-bound into the sterile view.
    real_codex_home: PathBuf,
    /// Oracle-only env passthroughs (OPENAI_BASE_URL / OPENAI_API_KEY) forwarded into the view when set.
    /// Empty in production (subscription auth comes from the ro-bound auth.json, not an env key).
    extra_setenv: Vec<(String, String)>,
    /// Oracle-only extra `codex exec` args (newline-separated in SHREK_CODEX_EXTRA_ARGS). Used by the
    /// request-capture proof to point codex at a LOCAL fake endpoint. Positioned BEFORE the
    /// security-critical reader-disable + read-only args so a passthrough can never override them.
    /// Empty in production.
    extra_args: Vec<String>,
}

/// Subcommand dispatch. No args (or `serve`) runs the serve loop; `login`/`health` are the operator
/// commands. All three read the same broker-side env config.
fn dispatch() -> i32 {
    let cfg = Confine::from_env();
    let default_model = map_model(
        Some(&env_or("SHREK_CODEX_DEFAULT_MODEL", DEFAULT_MODEL)),
        "gpt-5.5",
    );
    match std::env::args().nth(1).as_deref() {
        None | Some("serve") => serve(&cfg, default_model),
        Some("login") => cmd_login(&cfg, default_model),
        Some("health") => cmd_health(&cfg, default_model),
        Some(other) => {
            eprintln!("CODEX-BROKER-USAGE unknown subcommand {other:?}; use: serve | login | health");
            2
        }
    }
}

impl Confine {
    fn from_env() -> Self {
        let codex_bin = env_or("SHREK_CODEX_BIN", DEFAULT_CODEX_BIN);
        let runtime_dir = std::env::var("SHREK_CODEX_RUNTIME_DIR")
            .ok()
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| derive_runtime_dir(&codex_bin))
            .unwrap_or_else(|| PathBuf::from("/usr"));
        let real_codex_home = std::env::var("SHREK_CODEX_HOME")
            .ok()
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let home = std::env::var("HOME").unwrap_or_default();
                Path::new(&home).join(".codex")
            });
        // Oracle-only: forward a local endpoint + stub key into the view IF present broker-side. This is
        // how the deterministic proof captures the request WITHOUT the real subscription credential.
        let mut extra_setenv = Vec::new();
        for k in ["OPENAI_BASE_URL", "OPENAI_API_KEY"] {
            if let Ok(v) = std::env::var(k) {
                if !v.is_empty() {
                    extra_setenv.push((k.to_string(), v));
                }
            }
        }
        let extra_args = std::env::var("SHREK_CODEX_EXTRA_ARGS")
            .ok()
            .filter(|v| !v.is_empty())
            .map(|v| v.lines().map(str::to_string).filter(|l| !l.is_empty()).collect())
            .unwrap_or_default();
        Confine {
            bwrap_bin: env_or("SHREK_BWRAP_BIN", DEFAULT_BWRAP_BIN),
            codex_bin,
            runtime_dir,
            real_codex_home,
            extra_setenv,
            extra_args,
        }
    }
}

/// The serve loop: accept the box's plaintext messages-API requests and adapt each to a confined
/// `codex exec` invocation.
fn serve(cfg: &Confine, default_model: &'static str) -> i32 {
    let listen = env_or("SHREK_CODEX_BROKER_LISTEN", DEFAULT_LISTEN);
    let listener = match TcpListener::bind(&listen) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("CODEX-BROKER-ERROR bind {listen}: {e}");
            return 2;
        }
    };
    println!(
        "CODEX-BROKER-LISTEN {listen} codex_bin={} bwrap_bin={} runtime_dir={} default_model={default_model}",
        cfg.codex_bin,
        cfg.bwrap_bin,
        cfg.runtime_dir.display()
    );

    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                // Confine is cheap to clone-by-value for the thread (a handful of owned strings).
                let cfg = cfg.clone_for_thread();
                std::thread::spawn(move || {
                    if let Err(e) = handle(stream, &cfg, default_model) {
                        eprintln!("CODEX-BROKER-ERROR conn: {e}");
                    }
                });
            }
            Err(e) => eprintln!("CODEX-BROKER-ERROR accept: {e}"),
        }
    }
    0
}

impl Confine {
    fn clone_for_thread(&self) -> Confine {
        Confine {
            bwrap_bin: self.bwrap_bin.clone(),
            codex_bin: self.codex_bin.clone(),
            runtime_dir: self.runtime_dir.clone(),
            real_codex_home: self.real_codex_home.clone(),
            extra_setenv: self.extra_setenv.clone(),
            extra_args: self.extra_args.clone(),
        }
    }
}

/// Handle one box→broker→codex exchange. Reads the box's plaintext messages-API request, adapts it to a
/// confined `codex exec` invocation, and writes the messages-API-shaped reply back plaintext. On ANY
/// failure — malformed request, CLI non-zero exit, empty output — fails CLOSED with a 502 to the box
/// (never a fabricated success). No credential is ever read or logged.
fn handle(mut box_stream: TcpStream, cfg: &Confine, default_model: &'static str) -> std::io::Result<()> {
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
            eprintln!("CODEX-BROKER-ERROR unparseable messages-API request body");
            write_plain(&mut box_stream, 400, "unparseable messages request")?;
            return Ok(());
        }
    };
    let model = map_model(req.model.as_deref(), default_model);
    println!(
        "CODEX-BROKER-FWD {method} {path} model_req={:?} -> codex exec --model {model} (prompt={}B system={}B)",
        req.model,
        req.prompt.len(),
        req.system.len()
    );

    // Invoke the LOGGED-IN CLI broker-side under confinement. The ONLY authority on token health: a real
    // round-trip.
    let result = match invoke_codex(cfg, model, &req.system, &req.prompt) {
        Ok(text) => text,
        Err(e) => {
            if looks_like_auth_failure(&e) {
                eprintln!("CODEX-BROKER-UPSTREAM-AUTH-FAIL codex login appears invalid: {e}");
            } else {
                eprintln!("CODEX-BROKER-CLI-ERROR {e}");
            }
            write_plain(&mut box_stream, 502, "codex cli invocation failed")?;
            return Ok(());
        }
    };
    println!("CODEX-BROKER-CLI-OK result={}B", result.len());

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
    system: String,
    prompt: String,
    model: Option<String>,
}

/// Parse the coder's Anthropic messages-API body into the pieces a `codex exec` call needs. `system` is
/// lifted verbatim; the `messages` array is flattened into a role-labelled transcript. Returns `None` on
/// any shape mismatch (→ fail closed). Identical wire shape to the claude broker (the coder is unchanged).
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
        let role = mo
            .get("role")
            .and_then(|r| r.get::<String>())
            .map(String::as_str)
            .unwrap_or("user");
        let content = mo
            .get("content")
            .and_then(|c| c.get::<String>())
            .cloned()
            .unwrap_or_default();
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
/// this allowlist — a raw caller string NEVER reaches the CLI argv (the argv-injection guard). An absent
/// or unrecognized id maps to `default` (itself an allowlist value). The caller can *select* among known
/// Codex models but cannot *introduce* one. (Codex model ids are OpenAI ids; `gpt-5.5` is the default.)
fn map_model(requested: Option<&str>, default: &'static str) -> &'static str {
    match requested.map(str::trim).unwrap_or("") {
        "gpt-5.5" => "gpt-5.5",
        "gpt-5.6-sol" => "gpt-5.6-sol",
        "gpt-5.6-luna" => "gpt-5.6-luna",
        "gpt-5.6-terra" => "gpt-5.6-terra",
        "gpt-5.4" => "gpt-5.4",
        "gpt-5.4-mini" => "gpt-5.4-mini",
        _ => default,
    }
}

/// Build the FULL confined argv vector: `bwrap <sterile-view args> -- codex exec <tool-disabled args>`.
/// Pure and deterministic (no I/O) so it is exhaustively unit-testable — the security of this slice
/// lives here. `host_out` is the broker-side scratch file bound rw as the SOLE writable host path;
/// `prompt` is a single data argument; `model` is an allowlist `&'static str`.
///
/// Invariants the tests assert: the spawned `codex` sees NO project/$HOME/vault (only the runtime ro +
/// `auth.json` ro + the one rw scratch); the model is offered NO tools (empty `[tools]` allowlist +
/// `--disable` of the exec/read tools); env is cleared; the session is fresh.
fn build_confined_argv(cfg: &Confine, host_out: &str, model: &'static str, prompt: &str) -> Vec<String> {
    let runtime = cfg.runtime_dir.to_string_lossy().into_owned();
    let auth_src = cfg.real_codex_home.join("auth.json").to_string_lossy().into_owned();
    let path_env = format!("{}/bin:/usr/bin:/bin", runtime);

    let mut a: Vec<String> = Vec::new();
    let mut push = |s: &str| a.push(s.to_string());

    // --- bwrap sterile view -----------------------------------------------------------------------
    push(&cfg.bwrap_bin);
    // Namespaces: isolate everything the model does not need. NET is deliberately KEPT (hosted
    // inference; and the oracle's LOCAL capture endpoint is reached over localhost).
    for ns in [
        "--unshare-user",
        "--unshare-pid",
        "--unshare-ipc",
        "--unshare-uts",
        "--unshare-cgroup",
    ] {
        push(ns);
    }
    push("--die-with-parent");
    push("--new-session"); // no controlling terminal → no TIOCSTI injection back at the broker
    push("--clearenv"); // start from ZERO env; only the allowlist below is injected

    // Minimal env allowlist.
    for (k, v) in [
        ("CODEX_HOME", SANDBOX_CODEX_HOME),
        ("HOME", SANDBOX_WORK),
        ("PATH", path_env.as_str()),
        ("TERM", "dumb"),
    ] {
        push("--setenv");
        push(k);
        push(v);
    }
    // Oracle-only passthroughs (OPENAI_BASE_URL / OPENAI_API_KEY): present ONLY under the proof harness.
    for (k, v) in &cfg.extra_setenv {
        push("--setenv");
        push(k);
        push(v);
    }

    // Read-only system + runtime. `--ro-bind-try` tolerates a path being absent on a given host.
    push("--ro-bind");
    push("/usr");
    push("/usr");
    for p in [
        "/bin",
        "/lib",
        "/lib64",
        "/etc/ssl",
        "/etc/ca-certificates",
        "/etc/resolv.conf",
        "/etc/hosts",
        "/etc/nsswitch.conf",
    ] {
        push("--ro-bind-try");
        push(p);
        push(p);
    }
    // Kill even the username directory entry: $HOME is a fresh tmpfs, nothing of the operator's home.
    // MUST precede the runtime bind: the nvm runtime lives UNDER /home, so a later `--tmpfs /home` would
    // shadow it. bwrap auto-creates the bind target dir on top of this fresh tmpfs.
    push("--tmpfs");
    push("/home");

    // The node/codex runtime tree (may live under $HOME/.nvm) — bound ro as the ONLY $HOME subpath,
    // ON TOP of the /home tmpfs above.
    push("--ro-bind");
    push(&runtime);
    push(&runtime);

    // The confined CODEX_HOME: a tmpfs dir holding ONLY the ro-bound auth.json. codex reads its own
    // credential; the broker never reads a token byte. codex may write ephemeral state elsewhere in the
    // tmpfs, discarded on exit; the credential file itself is read-only so it cannot be corrupted.
    push("--dir");
    push(SANDBOX_CODEX_HOME);
    push("--ro-bind");
    push(&auth_src);
    push(SANDBOX_AUTH);

    // Writable scratch: tmpfs /tmp + /work, and the SOLE writable HOST path — the one -o scratch file.
    push("--tmpfs");
    push("/tmp");
    push("--tmpfs");
    push(SANDBOX_WORK);
    push("--bind");
    push(host_out);
    push(SANDBOX_OUT);
    push("--chdir");
    push(SANDBOX_WORK);

    // Minimal /proc + /dev for the node runtime (needs /dev/urandom, /dev/null, /proc/self).
    push("--proc");
    push("/proc");
    push("--dev");
    push("/dev");

    // --- the confined command: codex exec, TOOL-DISABLED, single-shot text --------------------------
    push("--");
    push(&cfg.codex_bin);
    push("exec");
    push(prompt); // single data argument (the flattened transcript, system prepended by invoke_codex)
    push("--model");
    push(model);
    push("--skip-git-repo-check");
    push("--ephemeral"); // no session files persisted
    push("--ignore-user-config"); // drops ~/.codex/config.toml (its MCP servers, project trust)
    push("--ignore-rules"); // no user/project execpolicy .rules

    // Oracle-only extra args (e.g. a LOCAL fake provider for the request-capture proof). Placed BEFORE
    // the security-critical reader-disable + read-only below so a passthrough can never re-enable a
    // reader. Empty in production.
    for arg in &cfg.extra_args {
        push(arg);
    }

    // READER-DISABLE — the credential guard. The model must not be able to READ the ro-bound auth.json
    // and exfil it through the reply. codex 0.148.0's `[tools]` allowlist is NOT honored via `-c`; the
    // SUPPORTED mechanism is the feature flags. Disabling `shell_tool` (removes the `exec_command` +
    // `write_stdin` shell tools), `unified_exec`, and `view_image` removes EVERY tool that can read a
    // file's contents. The residual tools codex still offers — update_plan / request_user_input /
    // apply_patch / tool_search / web_search — have NO filesystem-read primitive (apply_patch is
    // write-only and further blocked by `-s read-only`). The oracle PROVES the captured request carries
    // no reader tool and only known non-reading tools.
    for feat in ["shell_tool", "unified_exec", "view_image"] {
        push("--disable");
        push(feat);
    }

    // Defense-in-depth (host confinement is authoritative): read-only sandbox neuters apply_patch's
    // writes. `codex exec` is non-interactive by construction, so it never prompts — no `-a` flag here.
    push("-s");
    push("read-only");

    // Final message text goes to the rw scratch the broker reads back.
    push("-o");
    push(SANDBOX_OUT);

    a
}

/// Broker-side unique scratch path for one request's `-o` file. Uniqueness from pid + an atomic counter
/// (no wall-clock dependency). Broker-side only; the sandbox sees it exclusively at `/work/last.txt`.
fn unique_scratch_path() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("shrek-codex-out-{}-{}.txt", std::process::id(), n))
}

/// Invoke the logged-in `codex` under the sterile view, returning its final-message text. Creates a
/// fresh 0600 scratch file (the rw bind target must exist), runs `bwrap … -- codex exec …`, then reads
/// the final message back. A non-zero exit or empty output ⇒ `Err` (fails closed upstream). The
/// subscription credential is entirely the CLI's; this process reads none of it.
fn invoke_codex(cfg: &Confine, model: &'static str, system: &str, prompt: &str) -> Result<String, String> {
    // Codex `exec` has no `--system-prompt`; fold the coder's protocol (its `system`) into the prompt as
    // a leading block. With tools disabled the model simply FOLLOWS it and replies with text.
    let full_prompt = if system.is_empty() {
        prompt.to_string()
    } else {
        format!("{system}\n\n{prompt}")
    };

    let out_path = unique_scratch_path();
    let out_str = out_path.to_string_lossy().into_owned();
    // Create the scratch file fresh + owner-only so the rw bind target exists before bwrap binds it.
    let _ = fs::remove_file(&out_path);
    {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&out_path)
            .map_err(|e| format!("scratch create: {e}"))?;
    }

    let argv = build_confined_argv(cfg, &out_str, model, &full_prompt);
    let run = Command::new(&argv[0]).args(&argv[1..]).output();

    let result = (|| {
        let out = run.map_err(|e| format!("spawn {}: {e}", cfg.bwrap_bin))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("codex exit={:?}: {}", out.status.code(), cap(&err, 400)));
        }
        let text = fs::read_to_string(&out_path).map_err(|e| format!("read scratch: {e}"))?;
        if text.trim().is_empty() {
            return Err("codex produced no final message".to_string());
        }
        Ok(text)
    })();

    let _ = fs::remove_file(&out_path); // never leave the reply on disk
    result
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
/// only; the failure is fail-closed regardless). Grounded in the REAL error text, never a status cache.
/// Covers Codex phrasings ("ChatGPT login is required", "sign in again", "logged out") as well as HTTP.
fn looks_like_auth_failure(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("authentication")
        || e.contains("401")
        || e.contains("unauthorized")
        || e.contains("invalid api key")
        || e.contains("oauth")
        || e.contains("login")
        || e.contains("sign in")
        || e.contains("logged out")
        || e.contains("access token")
}

fn cap(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n])
    }
}

/// Derive the runtime tree to ro-bind from the codex binary path. For an nvm layout
/// (`…/node/vX/bin/codex`), that is the node version dir `…/node/vX` (contains `bin/node`, the codex
/// package, and codex's bundled resources). Returns `None` for a bare name (caller falls back / the
/// operator sets SHREK_CODEX_RUNTIME_DIR).
fn derive_runtime_dir(codex_bin: &str) -> Option<PathBuf> {
    let p = Path::new(codex_bin);
    if !codex_bin.contains('/') {
        // Bare name: try to locate it on PATH (…/bin/codex), then take its bin dir's parent.
        let found = which(codex_bin)?;
        return found.parent()?.parent().map(Path::to_path_buf);
    }
    p.parent()?.parent().map(Path::to_path_buf)
}

/// Minimal PATH search (no external dep) for a bare binary name.
fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

// ---- minimal HTTP/1.1 request reader (box→broker is a controlled, single-request path) -------------
// Mirrors crates/claude-broker: request line + headers + exactly Content-Length body.

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

// ---- the login UX + the audit-only availability breadcrumb (generalized from slice-5) --------------
//
// "Sign in with Codex" WITHOUT a preexisting manual login: `codex-broker login` hands the operator's
// REAL terminal to the official `codex login`, which owns 100% of the credential state (it writes its
// own ~/.codex; Shrek never sees a token). Completion is then VERIFIED by one real `codex exec`
// round-trip — never `codex login status`/`codex doctor` (they read cached state and lie — #1567) — and
// observed as a STATE TRANSITION recorded in an audit-only breadcrumb. This is a broker-host OPERATOR
// ceremony, deliberately NOT the sandboxed-agent grant path (grant-protocol.md). NOTE: `login` runs the
// CLI UNconfined (it must write the real ~/.codex and complete the browser callback); only the
// model-inference `exec` path is confined under bwrap.

const PROVIDER: &str = "codex-cli";
const PROBE_PROMPT: &str = "ping";
const BREADCRUMB_FILE: &str = "availability.json";
const BREADCRUMB_TMP: &str = ".availability.json.tmp";

/// The breadcrumb's `reason` — a FIXED enum, never free text. The guard against the breadcrumb becoming
/// an accidental credential/logging surface: raw `codex` stdout/stderr NEVER reaches the file — only one
/// of these five stable strings does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reason {
    Verified,
    AuthFailed,
    LoginFailed,
    NonTty,
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

struct Availability {
    available: bool,
    reason: Reason,
    last_verified: u64,
}

/// Serialize the breadcrumb via the JSON encoder with EXACTLY the four audit fields. No field carries
/// CLI output or a credential.
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

/// Broker-side directory for the breadcrumb: `$SHREK_CODEX_STATE_DIR`, else
/// `$HOME/.local/state/shrek-codex-cli`, else a cwd-relative fallback. Always broker-side.
fn state_dir() -> PathBuf {
    if let Some(d) = std::env::var("SHREK_CODEX_STATE_DIR").ok().filter(|v| !v.is_empty()) {
        return PathBuf::from(d);
    }
    if let Some(h) = std::env::var("HOME").ok().filter(|v| !v.is_empty()) {
        return Path::new(&h).join(".local/state/shrek-codex-cli");
    }
    PathBuf::from("shrek-codex-cli-state")
}

/// Write the breadcrumb ATOMICALLY and OWNER-ONLY: fresh 0600 temp → write → fsync → rename → fsync dir.
/// A crash can never leave believable partial state. The dir is 0700.
fn write_availability_to(dir: &Path, a: &Availability) -> std::io::Result<()> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let tmp = dir.join(BREADCRUMB_TMP);
    let final_path = dir.join(BREADCRUMB_FILE);
    let _ = fs::remove_file(&tmp);
    {
        let mut f = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        f.write_all(availability_json(a).as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, &final_path)?;
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Record an observation to the breadcrumb (audit-only). A write failure is logged with a fixed marker
/// and otherwise ignored — the breadcrumb never gates behavior.
fn record(available: bool, reason: Reason) {
    let a = Availability { available, reason, last_verified: now_epoch() };
    if let Err(e) = write_availability_to(&state_dir(), &a) {
        eprintln!("CODEX-BROKER-BREADCRUMB-WRITE-FAIL kind={:?}", e.kind());
    }
}

/// One real `codex exec` round-trip (confined, tool-disabled). Returns `Ok` if the CLI round-tripped; on
/// failure returns a FIXED `Reason` from the error CLASSIFIER — the raw error text is DROPPED here, never
/// returned or stored. The only authority on login health; `codex login status` is never consulted (#1567).
fn probe(cfg: &Confine, model: &'static str) -> Result<(), Reason> {
    match invoke_codex(cfg, model, "", PROBE_PROMPT) {
        Ok(_) => Ok(()),
        Err(e) if looks_like_auth_failure(&e) => Err(Reason::AuthFailed),
        Err(_) => Err(Reason::ProbeFailed),
    }
}

/// `codex-broker login` — the trusted OPERATOR path. Refuses fast if not on a real terminal (never hangs
/// on a browser callback — #595), hands the terminal to the official `codex login` (which owns all
/// credential state and whose output we never capture), then folds in the round-trip health check.
fn cmd_login(cfg: &Confine, model: &'static str) -> i32 {
    if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        record(false, Reason::NonTty);
        eprintln!(
            "CODEX-BROKER-LOGIN-REFUSED reason=non-tty (run at a real console; the official OAuth flow \
             needs an interactive terminal — a headless run would hang on the browser callback)"
        );
        return 3;
    }
    println!(
        "CODEX-BROKER-LOGIN-BEGIN handing the terminal to `codex login` \
         (the CLI owns ALL credential state; Shrek captures nothing)"
    );
    // Fixed argv vector, inherited stdio (never captured). Runs UNconfined so the CLI can write the real
    // ~/.codex and complete the browser callback.
    match Command::new(&cfg.codex_bin).arg("login").status() {
        Ok(s) if s.success() => {}
        Ok(s) => {
            record(false, Reason::LoginFailed);
            eprintln!("CODEX-BROKER-LOGIN-FAIL reason=login-failed exit={:?}", s.code());
            return 5;
        }
        Err(_e) => {
            record(false, Reason::LoginFailed);
            eprintln!("CODEX-BROKER-LOGIN-FAIL reason=login-failed (could not spawn the codex CLI)");
            return 5;
        }
    }
    println!("CODEX-BROKER-LOGIN-DONE verifying with one real `codex exec` round-trip (never `login status`)");
    match probe(cfg, model) {
        Ok(()) => {
            record(true, Reason::Verified);
            println!("CODEX-BROKER-LOGIN-VERIFIED reason=verified provider={PROVIDER}");
            0
        }
        Err(r) => {
            record(false, r);
            eprintln!(
                "CODEX-BROKER-LOGIN-UNVERIFIED reason={} (login exited 0 but the round-trip failed)",
                r.as_str()
            );
            4
        }
    }
}

/// `codex-broker health` — run just the round-trip probe and update the breadcrumb.
fn cmd_health(cfg: &Confine, model: &'static str) -> i32 {
    match probe(cfg, model) {
        Ok(()) => {
            record(true, Reason::Verified);
            println!("CODEX-BROKER-PROBE-OK reason=verified provider={PROVIDER}");
            0
        }
        Err(r) => {
            record(false, r);
            eprintln!("CODEX-BROKER-PROBE-FAIL reason={}", r.as_str());
            4
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cfg() -> Confine {
        Confine {
            bwrap_bin: "bwrap".to_string(),
            codex_bin: "codex".to_string(),
            runtime_dir: PathBuf::from("/home/op/.nvm/versions/node/v24/"),
            real_codex_home: PathBuf::from("/home/op/.codex"),
            extra_setenv: vec![],
            extra_args: vec![],
        }
    }

    /// Find the value that follows the FIRST occurrence of `flag` in an argv vector.
    fn val_after<'a>(argv: &'a [String], flag: &str) -> Option<&'a str> {
        argv.iter().position(|a| a == flag).and_then(|i| argv.get(i + 1)).map(String::as_str)
    }

    /// Every value that follows an occurrence of `flag` (for repeated flags like `--ro-bind`).
    fn all_vals_after(argv: &[String], flag: &str) -> Vec<String> {
        let mut out = Vec::new();
        for (i, a) in argv.iter().enumerate() {
            if a == flag {
                if let Some(v) = argv.get(i + 1) {
                    out.push(v.clone());
                }
            }
        }
        out
    }

    #[test]
    fn map_model_allowlists_and_never_passes_raw() {
        assert_eq!(map_model(Some("gpt-5.5"), "gpt-5.5"), "gpt-5.5");
        assert_eq!(map_model(Some("gpt-5.6-sol"), "gpt-5.5"), "gpt-5.6-sol");
        assert_eq!(map_model(Some("gpt-5.4-mini"), "gpt-5.5"), "gpt-5.4-mini");
        // Unknown / absent / injection-y strings ALL fall to the default — the raw string is discarded,
        // never returned (so it can never reach the CLI argv).
        assert_eq!(map_model(Some("evil; rm -rf /"), "gpt-5.5"), "gpt-5.5");
        assert_eq!(map_model(Some("--dangerously-bypass-approvals-and-sandbox"), "gpt-5.5"), "gpt-5.5");
        assert_eq!(map_model(Some("claude-opus-4-8"), "gpt-5.5"), "gpt-5.5"); // not a codex id
        assert_eq!(map_model(Some(""), "gpt-5.5"), "gpt-5.5");
        assert_eq!(map_model(None, "gpt-5.5"), "gpt-5.5");
    }

    #[test]
    fn parse_messages_flattens_transcript_and_lifts_system() {
        let body = br#"{"model":"gpt-5.5","max_tokens":2048,"system":"be terse",
            "messages":[{"role":"user","content":"fix the bug"},
                        {"role":"assistant","content":"{\"tool\":\"read_file\"}"},
                        {"role":"user","content":"OK contents"}]}"#;
        let r = parse_messages_request(body).expect("parses");
        assert_eq!(r.system, "be terse");
        assert_eq!(r.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(r.prompt, "User: fix the bug\n\nAssistant: {\"tool\":\"read_file\"}\n\nUser: OK contents");
    }

    #[test]
    fn parse_messages_fails_closed_on_junk() {
        assert!(parse_messages_request(b"not json").is_none());
        assert!(parse_messages_request(b"{\"messages\":[]}").is_none());
        assert!(parse_messages_request(b"{\"no_messages\":true}").is_none());
    }

    #[test]
    fn confined_argv_disables_the_reader_tools() {
        // THE credential guard: every FILE-READER tool must be disabled so the model can never read the
        // ro-bound auth.json and exfil it through the reply. The SUPPORTED mechanism in codex 0.148.0 is
        // the feature flags (the `[tools]` allowlist is not honored via -c). `shell_tool` removes the
        // exec_command/write_stdin shell tools; `view_image` removes the image reader.
        let argv = build_confined_argv(&test_cfg(), "/tmp/out.txt", "gpt-5.5", "hi");
        let joined = argv.join(" ");
        assert!(argv.windows(2).any(|w| w[0] == "--disable" && w[1] == "shell_tool"));
        assert!(argv.windows(2).any(|w| w[0] == "--disable" && w[1] == "unified_exec"));
        assert!(argv.windows(2).any(|w| w[0] == "--disable" && w[1] == "view_image"));
        // Defense-in-depth is present but NOT the boundary. (`codex exec` is non-interactive, so it has
        // no `-a`/approval flag — the read-only sandbox neuters apply_patch's writes.)
        assert!(argv.windows(2).any(|w| w[0] == "-s" && w[1] == "read-only"));
        assert!(argv.contains(&"--ephemeral".to_string()));
        assert!(argv.contains(&"--ignore-user-config".to_string()));
        assert!(argv.contains(&"--ignore-rules".to_string()));
        // Never a full-access escape hatch.
        assert!(!joined.contains("danger-full-access"));
        assert!(!joined.contains("--dangerously-bypass"));
    }

    #[test]
    fn extra_args_precede_the_reader_disables_and_cannot_override_them() {
        // Oracle passthrough args are inserted BEFORE the reader-disable + read-only, so even a
        // hostile passthrough cannot re-enable a reader (the fixed disables always come last).
        let mut cfg = test_cfg();
        cfg.extra_args = vec!["-c".to_string(), "model_provider=fake".to_string()];
        let argv = build_confined_argv(&cfg, "/tmp/out.txt", "gpt-5.5", "hi");
        let extra_pos = argv.iter().position(|a| a == "model_provider=fake").expect("extra arg present");
        let first_disable = argv.iter().position(|a| a == "--disable").expect("a --disable present");
        let readonly = argv.windows(2).position(|w| w[0] == "-s" && w[1] == "read-only").expect("read-only");
        assert!(extra_pos < first_disable, "extra args must precede the reader-disable");
        assert!(extra_pos < readonly, "extra args must precede -s read-only");
    }

    #[test]
    fn confined_argv_sterile_view_hides_home_and_has_one_writable_host_path() {
        let cfg = test_cfg();
        let argv = build_confined_argv(&cfg, "/tmp/out.txt", "gpt-5.5", "hi");

        // The ONLY writable HOST bind (`--bind`) is the single -o scratch file → /work/last.txt.
        let rw = all_vals_after(&argv, "--bind");
        assert_eq!(rw, vec!["/tmp/out.txt".to_string()], "exactly one rw host bind: the scratch file");
        // …and it maps to the fixed sandbox path.
        let bind_i = argv.iter().position(|a| a == "--bind").unwrap();
        assert_eq!(argv[bind_i + 2], SANDBOX_OUT);

        // The credential is ro-bound (never a rw --bind), at the fixed sandbox auth path.
        let ro = all_vals_after(&argv, "--ro-bind");
        assert!(ro.iter().any(|p| p == "/home/op/.codex/auth.json"), "auth.json must be ro-bound");
        assert!(!rw.iter().any(|p| p.contains(".codex")), "auth.json must NOT be writable");
        // The auth ro-bind maps onto the sandbox auth path.
        assert!(argv.windows(3).any(|w| w[0] == "--ro-bind"
            && w[1] == "/home/op/.codex/auth.json"
            && w[2] == SANDBOX_AUTH));

        // $HOME is a fresh tmpfs (no username dir leak); no project/vault path is bound anywhere.
        assert!(argv.windows(2).any(|w| w[0] == "--tmpfs" && w[1] == "/home"));
        for a in &argv {
            assert!(!a.contains("/projects/"), "no project path may enter the view: {a}");
            assert!(!a.contains("/vault"), "no vault path may enter the view: {a}");
        }
        // The runtime tree IS ro-bound (so codex can run) — the one $HOME subpath allowed.
        assert!(ro.iter().any(|p| p == "/home/op/.nvm/versions/node/v24/"));
    }

    #[test]
    fn confined_argv_clears_env_and_isolates_namespaces() {
        let argv = build_confined_argv(&test_cfg(), "/tmp/out.txt", "gpt-5.5", "hi");
        assert!(argv.contains(&"--clearenv".to_string()), "env must be cleared");
        assert!(argv.contains(&"--new-session".to_string()), "fresh session (no TIOCSTI)");
        assert!(argv.contains(&"--die-with-parent".to_string()));
        for ns in ["--unshare-user", "--unshare-pid", "--unshare-ipc", "--unshare-uts", "--unshare-cgroup"] {
            assert!(argv.contains(&ns.to_string()), "missing namespace isolation: {ns}");
        }
        // NET is deliberately NOT unshared (hosted inference + localhost capture endpoint).
        assert!(!argv.contains(&"--unshare-net".to_string()));
        // Confined CODEX_HOME + HOME are the sandbox paths, not host paths.
        assert_eq!(val_after(&argv, "--setenv"), Some("CODEX_HOME")); // first setenv key
        let joined = argv.join(" ");
        assert!(joined.contains("--setenv CODEX_HOME /codexhome"));
        assert!(joined.contains("--setenv HOME /work"));
    }

    #[test]
    fn confined_argv_is_bwrap_then_codex_exec_with_prompt_as_single_arg() {
        let argv = build_confined_argv(&test_cfg(), "/tmp/out.txt", "gpt-5.5", "a whole\nmulti-line prompt");
        assert_eq!(argv[0], "bwrap");
        // The separator, then the confined program.
        let sep = argv.iter().position(|a| a == "--").expect("has -- separator");
        assert_eq!(argv[sep + 1], "codex");
        assert_eq!(argv[sep + 2], "exec");
        // The prompt is ONE argv element (data, never split into flags), even with newlines.
        assert!(argv.contains(&"a whole\nmulti-line prompt".to_string()));
        // The model is the allowlist &'static value, passed as one arg after --model.
        assert_eq!(val_after(&argv, "--model"), Some("gpt-5.5"));
        assert_eq!(val_after(&argv, "-o"), Some(SANDBOX_OUT));
    }

    #[test]
    fn oracle_passthrough_forwards_endpoint_into_the_view_only_when_present() {
        // Production: no OPENAI_* → nothing forwarded (subscription auth via the ro-bound auth.json).
        let prod = build_confined_argv(&test_cfg(), "/tmp/out.txt", "gpt-5.5", "hi");
        assert!(!prod.join(" ").contains("OPENAI_BASE_URL"));
        // Oracle: a local endpoint + stub key are forwarded so the request can be captured WITHOUT the
        // real credential.
        let mut cfg = test_cfg();
        cfg.extra_setenv = vec![
            ("OPENAI_BASE_URL".to_string(), "http://127.0.0.1:9".to_string()),
            ("OPENAI_API_KEY".to_string(), "sk-STUB-not-real".to_string()),
        ];
        let orc = build_confined_argv(&cfg, "/tmp/out.txt", "gpt-5.5", "hi");
        let j = orc.join(" ");
        assert!(j.contains("--setenv OPENAI_BASE_URL http://127.0.0.1:9"));
        assert!(j.contains("--setenv OPENAI_API_KEY sk-STUB-not-real"));
    }

    #[test]
    fn wrap_reply_is_valid_anthropic_shape_and_escapes() {
        let reply = wrap_reply("line1\n\"quoted\"");
        let v: JsonValue = reply.parse().expect("valid json");
        let obj = v.get::<std::collections::HashMap<String, JsonValue>>().unwrap();
        let blocks = obj.get("content").unwrap().get::<Vec<JsonValue>>().unwrap();
        let b0 = blocks[0].get::<std::collections::HashMap<String, JsonValue>>().unwrap();
        assert_eq!(b0.get("type").unwrap().get::<String>().unwrap(), "text");
        assert_eq!(b0.get("text").unwrap().get::<String>().unwrap(), "line1\n\"quoted\"");
    }

    #[test]
    fn auth_failure_classifier_covers_codex_phrasing() {
        assert!(looks_like_auth_failure("Error: 401 Unauthorized"));
        assert!(looks_like_auth_failure("ChatGPT login is required"));
        assert!(looks_like_auth_failure("Please sign in again"));
        assert!(looks_like_auth_failure("your access token could not be refreshed"));
        assert!(!looks_like_auth_failure("model produced invalid output"));
    }

    #[test]
    fn reason_maps_to_fixed_strings() {
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
        assert!(!s.contains("sk-"), "breadcrumb must never carry a token: {s}");
        let v: JsonValue = s.parse().expect("valid json");
        let obj = v.get::<HashMap<String, JsonValue>>().unwrap();
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
        assert_eq!(obj.get("provider").unwrap().get::<String>().unwrap(), "codex-cli");
    }

    #[test]
    fn breadcrumb_write_is_atomic_owner_only_and_leaves_no_temp() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("shrek-codex-bc-{}-atomic", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = Availability { available: false, reason: Reason::AuthFailed, last_verified: 42 };
        write_availability_to(&dir, &a).expect("writes");
        let final_path = dir.join(BREADCRUMB_FILE);
        let mode = std::fs::metadata(&final_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "breadcrumb must be owner-only 0600, got {mode:o}");
        assert!(!dir.join(BREADCRUMB_TMP).exists(), "atomic rename must leave no .tmp");
        let read = std::fs::read_to_string(&final_path).unwrap();
        let v: JsonValue = read.parse().unwrap();
        let obj = v.get::<std::collections::HashMap<String, JsonValue>>().unwrap();
        assert_eq!(obj.get("reason").unwrap().get::<String>().unwrap(), "auth-failed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_subslice_locates_blank_line() {
        assert_eq!(find_subslice(b"a\r\n\r\nb", b"\r\n\r\n"), Some(1));
        assert_eq!(find_subslice(b"noblank", b"\r\n\r\n"), None);
    }
}
