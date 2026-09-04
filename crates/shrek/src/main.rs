//! shrek — the user-facing CLI. `shrek run` is the Phase-6 front door: the first single
//! command that composes a real coding session out of the already-shipped gatekeeperd planes
//! (provenance/tier derivation, project write-through grant, exec build area, T2/gVisor, named
//! egress, execution, persistent project changes, teardown). It is a THIN COMPOSER — it owns no
//! isolation mechanism and reimplements no plane; it resolves an ergonomic invocation into the
//! `gatekeeperd sandbox …` argv and execs that engine, whose decision plane remains the authority.
//!
//! Invariants this front door must not violate (system-index route rd_6d0fa36317c1):
//!   PG5 fail-closed-construction — gatekeeperd's exit code is propagated VERBATIM. A decision-plane
//!       refuse (non-zero) surfaces as failure here; success is never synthesized.
//!   PG2 / t2-no-falldown — the front door never claims a LOWER tier than the workload derives. The
//!       coding profile claims T2 and passes the real band inputs; the decision plane re-derives and
//!       refuses on mismatch. We never downgrade to dodge a refuse.
//!   PG6 sealed-immutable-policy — `--egress NAME` is a NAME resolved by gatekeeperd against SEALED
//!       profiles. This CLI never accepts raw hosts/ports/rules; an unknown name fails closed there.
//!
//! Dependency-free (minimal-deps): std only.

use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match argv.first().map(String::as_str) {
        Some("run") => std::process::exit(run(&argv[1..])),
        Some("find") => std::process::exit(find(&argv[1..])),
        // Phase-8 slice-1: the agent-session front door. `shrek session status <id>` reads the
        // effective-authority view; otherwise it composes a session and routes it through agentd (which
        // decides the tier + attaches an identity), so the session carries a visible authority record.
        Some("session") => std::process::exit(match argv.get(1).map(String::as_str) {
            Some("status") => session_status(&argv[2..]),
            _ => session(&argv[1..]),
        }),
        // ADR-003 Part 2: the Bench plane front door. A thin forwarder to `gatekeeperd bench …` (the
        // supervisor re-validates every verb + owns the privileged lifecycle/quota ops), mirroring
        // `shrek run` → `gatekeeperd sandbox`. Bench containers run as `dev`'s rootless podman; the
        // supervisor drops privilege internally.
        Some("bench") => std::process::exit(bench(&argv[1..])),
        Some("connectivity") => std::process::exit(connectivity(&argv[1..])),
        // ADR-006 Mode A: the on-device AI front door. A thin forwarder to
        // /usr/lib/shrek/ai/shrek-ai-front-door (mirrors `shrek run` → gatekeeperd). This CLI owns no AI
        // logic and adds no host-exec surface; the front door + the vendored no-exec shell are the boundary.
        Some("ai") => std::process::exit(ai(&argv[1..])),
        Some("-h") | Some("--help") | None => {
            usage();
            std::process::exit(0);
        }
        Some(other) => {
            eprintln!("shrek: unknown subcommand `{other}`");
            usage();
            std::process::exit(2);
        }
    }
}

fn usage() {
    eprintln!("shrek {} — user CLI", env!("CARGO_PKG_VERSION"));
    eprintln!();
    eprintln!("  shrek run --project DIR [opts] -- WORKLOAD...");
    eprintln!("      Run WORKLOAD in a T2 (gVisor) coding sandbox with write-through authority to");
    eprintln!("      DIR only, an exec-capable build area, and — with --egress — a sealed named");
    eprintln!("      egress profile. Everything else (rest of $HOME, network) stays unreachable.");
    eprintln!();
    eprintln!("  Options:");
    eprintln!("    --project DIR        the project; realized WRITE-THROUGH + host-noexec (required)");
    eprintln!("    --build DIR          exec-capable build area (default: <project>.build, created)");
    eprintln!("    --no-build           omit the build area (project stays the only writable grant)");
    eprintln!("    --egress NAME        attach a SEALED egress profile by name (repeatable — e.g.");
    eprintln!("                         --egress model-claude-cli --egress swamp-query; else loopback)");
    eprintln!("    --trust BAND         claimed trust band (default: T-hostile)");
    eprintln!("    --tier Tn            claimed tier the decision plane re-checks (default: T2)");
    eprintln!("    --no-ingest-harness  derive the band from the entrypoint arm, not the sealed harness");
    eprintln!("    --no-chdir           do not cd into the project before the workload runs");
    eprintln!("    --id NAME            sandbox id (default: derived from the project name)");
    eprintln!("    --guest-prefix DIR   in-guest mount prefix for grants (default: /srv)");
    eprintln!("    --dry-run            print the composed `gatekeeperd sandbox` argv and exit");
    eprintln!();
    eprintln!("  shrek find --session ID [opts] TERMS...");
    eprintln!("      Query the Swamp for objects in the SESSION's authority only. The session handle");
    eprintln!("      names a grant record swampd resolves independently; results never include an");
    eprintln!("      object outside that session's granted filesystem authority.");
    eprintln!("  Options:");
    eprintln!("    --session ID         session authority handle (default: $SHREK_SESSION)");
    eprintln!("    --intent search|discover   full-text (default) or path/name match");
    eprintln!("    --scope PATH         narrow within the session grants (never widens)");
    eprintln!("    --limit N            max hits (default 50)");
    eprintln!("    --socket PATH        swampd query socket (default: $SWAMP_QUERY_SOCK or /run/swamp/query.sock)");
    eprintln!();
    eprintln!();
    eprintln!("  shrek bench <verb> …    the Bench plane — a persistent, quota-capped rootless-container");
    eprintln!("      home you install tools into without touching the sealed /usr. Verbs:");
    eprintln!("    create <name> [--quota KiB]   make a Bench (default 4 GiB project quota)");
    eprintln!("    run <name> [-- CMD…]          run a container in the Bench (no network)");
    eprintln!("    enter <name>                  interactive shell in the Bench");
    eprintln!("    quota <name> [KiB]            show or set the Bench's disk cap");
    eprintln!("    reset <name>                  wipe the Bench's data, keep its identity + quota");
    eprintln!("    destroy <name>                remove the Bench entirely");
    eprintln!("    list                          list all Benches");
    eprintln!("      (grant/network arrive in step 5; promote → Workshop later)");
    eprintln!();
    eprintln!("  shrek ai [opts]         open the on-device AI front door (ADR-006 Mode A): the offline,");
    eprintln!("      hardened mycolink-shell wired to the on-box model + memory (loopback only, zero");
    eprintln!("      egress, no host-exec). Starts the model on demand. Opts: --model, --resume, --cartridge.");
    eprintln!();
    eprintln!("  (planned) shrek history | related | status; shrek audit --agent");
}

/// `shrek ai [ARGS…]` — the ADR-006 Mode-A on-device AI front door. A THIN forwarder to
/// /usr/lib/shrek/ai/shrek-ai-front-door (which wires the hardened mycolink-shell to on-box loopback
/// services and drops the operator in). Mirrors `shrek run` → gatekeeperd: no AI logic, no host-exec
/// surface of its own. `--help` is handled HERE (no model start) so `shrek ai --help` is a cheap offline
/// probe; a real invocation execs the front door with the operator's args passed through verbatim.
fn ai(args: &[String]) -> i32 {
    const FRONT_DOOR: &str = "/usr/lib/shrek/ai/shrek-ai-front-door";
    if matches!(args.first().map(String::as_str), Some("-h") | Some("--help")) {
        ai_usage();
        return 0;
    }
    if !Path::new(FRONT_DOOR).exists() {
        eprintln!("shrek ai: on-device AI layer not present (the shrek-ai Onion is not merged on this box).");
        eprintln!("          Build with INCLUDE_AI=1 and boot the shrek-ai layer to enable `shrek ai`.");
        return 1;
    }
    let err = Command::new(FRONT_DOOR).args(args).exec();
    // Only reached if exec itself failed (missing interpreter, not executable, …).
    eprintln!("shrek ai: cannot exec `{FRONT_DOOR}`: {err}");
    1
}

fn ai_usage() {
    eprintln!("shrek ai [--model tier2|tier3] [--resume SESSION_ID] [--cartridge ID]");
    eprintln!("      Open the on-device AI front door (ADR-006 Mode A): the hardened, OFFLINE");
    eprintln!("      mycolink-shell wired to the on-box model (127.0.0.1:8198) and memory");
    eprintln!("      (127.0.0.1:8199) — zero egress, no host-exec surface. Starts the model on demand.");
}

/// %-escape space/`%`/control so each argv arg is exactly one newline-free line on the count-framed BENCH
/// wire (mirrors `gatekeeperd::bench_plane::pct_encode`; kept inline so this front door stays dep-free).
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

/// `shrek bench <verb> …` — drive the Bench control plane over gatekeeperd's authenticated socket
/// (ADR-003 Part 2 authorization slice), NOT by exec'ing the privileged binary. The daemon runs the verb
/// as root behind the SO_PEERCRED gate and re-validates every argument; for the authority-increasing verbs
/// (grant/network/export) it runs the console consent ceremony. The verb + its argv go in the count-framed
/// `BENCH` request so an arbitrary workload after `--` survives spaces/newlines. Exit status mirrors the
/// supervisor's `END <rc>` (same fidelity as `shrek run`). This front door adds no logic.
fn bench(args: &[String]) -> i32 {
    if matches!(args.first().map(String::as_str), Some("-h") | Some("--help")) || args.is_empty() {
        usage();
        return if args.is_empty() { 2 } else { 0 };
    }
    frame_to_gatekeeper("BENCH", "shrek bench", &args[0], &args[1..])
}

/// `shrek connectivity <bless|unbless|add-raw|remove-raw> <profile-or-host:proto:port>` — drive the
/// ADR-007 S4 desktop-egress CONSOLE CEREMONY over gatekeeperd's socket. The DMS Connectivity panel
/// execs this (fixed argv, no shell). gatekeeperd runs the SAK/VT ceremony; on a confirmed OK it execs
/// the root-only `egressd confirmed-*` verb. Fail-closed: an unreachable supervisor is unavailability,
/// never a grant.
fn connectivity(args: &[String]) -> i32 {
    if matches!(args.first().map(String::as_str), Some("-h") | Some("--help")) || args.is_empty() {
        eprintln!("usage: shrek connectivity <bless|unbless|add-raw|remove-raw> <profile|host:proto:port>");
        eprintln!("  Grants the high-consequence desktop-egress tier at the CONSOLE: this switches to a");
        eprintln!("  secure text screen — press the Secure Attention key (Ctrl-Alt-Break) and type the");
        eprintln!("  shown code to approve. web-browsing opens broad internet access for the browser.");
        return if args.is_empty() { 2 } else { 0 };
    }
    frame_to_gatekeeper("DESKTOP-EGRESS", "shrek connectivity", &args[0], &args[1..])
}

/// Send a count-framed request (`<FRAME> <verb> <argc>` + one pct-encoded arg per line) to gatekeeperd's
/// authenticated socket and stream back its `RESULT …`/`END <rc>` reply. Shared by the bench + the
/// desktop-egress ceremony front doors — both go through the daemon (SO_PEERCRED-gated, re-validating,
/// ceremony-owning), never by exec'ing a privileged binary.
fn frame_to_gatekeeper(frame: &str, tool: &str, verb: &str, rest: &[String]) -> i32 {
    use std::io::{BufRead, BufReader, Write};
    let socket = std::env::var("SHREK_BROKER_SOCK").unwrap_or_else(|_| "/run/shrek-gk.sock".to_string());
    let mut stream = match std::os::unix::net::UnixStream::connect(&socket) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{tool}: gatekeeper socket unavailable ({socket}): {e}");
            return 1;
        }
    };
    let mut req = format!("{frame} {verb} {}\n", rest.len());
    for a in rest {
        req.push_str(&pct_encode(a));
        req.push('\n');
    }
    if let Err(e) = stream.write_all(req.as_bytes()) {
        eprintln!("{tool}: write failed: {e}");
        return 1;
    }
    let _ = stream.flush();
    let reader = BufReader::new(&stream);
    let mut rc: i32 = 1;
    let mut saw_end = false;
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => { eprintln!("{tool}: read failed: {e}"); return 1; }
        };
        if let Some(tail) = line.strip_prefix("END ") {
            rc = tail.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(1);
            saw_end = true;
            break;
        }
        if let Some(tail) = line.strip_prefix("RESULT ") {
            println!("{tail}");
        }
    }
    if !saw_end {
        eprintln!("{tool}: no well-formed response from the supervisor");
        return 1;
    }
    rc
}

/// Parsed `shrek run` request, resolved to what the engine needs.
struct Plan {
    anchor: PathBuf,
    rw_name: String,
    build_name: Option<String>,
    egress: Vec<String>,
    trust: String,
    tier: String,
    ingest_harness: bool,
    id: String,
    guest_prefix: String,
    workload: Vec<String>,
    dry_run: bool,
}

fn run(args: &[String]) -> i32 {
    let mut project: Option<String> = None;
    let mut build: Option<String> = None;
    let mut no_build = false;
    let mut egress: Vec<String> = Vec::new();
    let mut trust = String::from("T-hostile");
    let mut tier = String::from("T2");
    let mut ingest_harness = true;
    let mut no_chdir = false;
    let mut id: Option<String> = None;
    let mut guest_prefix = String::from("/srv");
    let mut dry_run = false;
    let mut workload: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--project" => { i += 1; project = args.get(i).cloned(); }
            "--build" => { i += 1; build = args.get(i).cloned(); }
            "--no-build" => no_build = true,
            // Repeatable (Phase-6 Swamp slice-2): each `--egress NAME` attaches one sealed profile; a
            // coding session can name its model broker AND swamp-query as two explicit grants.
            "--egress" => { i += 1; if let Some(v) = args.get(i) { egress.push(v.clone()); } }
            "--trust" => { i += 1; if let Some(v) = args.get(i) { trust = v.clone(); } }
            "--tier" => { i += 1; if let Some(v) = args.get(i) { tier = v.clone(); } }
            "--no-ingest-harness" => ingest_harness = false,
            "--no-chdir" => no_chdir = true,
            "--id" => { i += 1; id = args.get(i).cloned(); }
            "--guest-prefix" => { i += 1; if let Some(v) = args.get(i) { guest_prefix = v.clone(); } }
            "--dry-run" => dry_run = true,
            "-h" | "--help" => { usage(); return 0; }
            "--" => { workload = args[i + 1..].to_vec(); break; }
            other => { eprintln!("shrek run: unknown arg `{other}`"); return 2; }
        }
        i += 1;
    }

    let Some(project) = project else {
        eprintln!("shrek run: --project DIR is required");
        return 2;
    };
    if workload.is_empty() {
        eprintln!("shrek run: a workload is required (… -- CMD ARG…)");
        return 2;
    }

    // Anchor = the project's parent (the pin root gatekeeperd opens with RESOLVE_BENEATH); the grant
    // is the project's basename. Ungranted siblings under the anchor stay invisible in-sandbox — only
    // NAMED grants are relocated, so the anchor being a parent dir grants nothing on its own.
    let project = absolutize(Path::new(&project));
    if !project.is_dir() {
        eprintln!("shrek run: project is not a directory: {}", project.display());
        return 2;
    }
    let (anchor, rw_name) = match split_beneath(&project) {
        Some(v) => v,
        None => {
            eprintln!("shrek run: project must be a named directory beneath a parent: {}", project.display());
            return 2;
        }
    };

    // Build area: a SECOND grant beneath the SAME anchor (grants share one pin root). Default
    // `<project>.build`, created 0700 if absent. It is the only exec-capable writable surface; the
    // project grant stays host-noexec so source bytes written there are never runnable in-sandbox.
    let build_name = if no_build {
        None
    } else {
        let build_path = match build {
            Some(b) => absolutize(Path::new(&b)),
            None => anchor.join(format!("{rw_name}.build")),
        };
        match split_beneath(&build_path) {
            Some((b_anchor, b_name)) if b_anchor == anchor => {
                if let Err(e) = ensure_dir_0700(&build_path) {
                    eprintln!("shrek run: cannot prepare build area {}: {e}", build_path.display());
                    return 2;
                }
                Some(b_name)
            }
            Some(_) => {
                eprintln!("shrek run: --build must be beneath the project's parent ({}); grants share one anchor", anchor.display());
                return 2;
            }
            None => {
                eprintln!("shrek run: invalid --build path: {}", build_path.display());
                return 2;
            }
        }
    };

    let id = id.unwrap_or_else(|| format!("shrek-{rw_name}"));

    // Optionally cd into the project's in-guest path, preserving argv exactly (no lossy shell-join):
    // `sh -c 'cd GUEST && exec "$0" "$@"' arg0 arg1 …`.
    let workload = if no_chdir {
        workload
    } else {
        let guest_proj = format!("{}/{}", guest_prefix.trim_end_matches('/'), rw_name);
        let mut w = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            format!("cd {} || exit 40; exec \"$0\" \"$@\"", shquote(&guest_proj)),
        ];
        w.extend(workload);
        w
    };

    let plan = Plan {
        anchor,
        rw_name,
        build_name,
        egress,
        trust,
        tier,
        ingest_harness,
        id,
        guest_prefix,
        workload,
        dry_run,
    };
    dispatch(plan)
}

/// Compose the `gatekeeperd sandbox` argv and exec it. `caps` is `C-net` iff at least one named egress
/// profile is attached (the ordered lattice: C-net ⊇ C-proj-rw, so a netted coding cell keeps project
/// RW), else `C-proj-rw`. `--profile` mirrors `--caps` (the declared profile the decision plane
/// re-checks). Each `--egress` becomes one `--egress-profile` arg — gatekeeperd resolves and unions them.
fn dispatch(p: Plan) -> i32 {
    let caps = if p.egress.is_empty() { "C-proj-rw" } else { "C-net" };

    let gk = std::env::var("SHREK_GATEKEEPERD").unwrap_or_else(|_| "gatekeeperd".to_string());
    let mut a: Vec<String> = vec![
        "sandbox".into(),
        "--tier".into(), p.tier,
        "--trust".into(), p.trust,
        "--caps".into(), caps.into(),
        "--profile".into(), caps.into(),
        "--id".into(), p.id,
        "--anchor".into(), p.anchor.to_string_lossy().into_owned(),
        "--guest-prefix".into(), p.guest_prefix,
        "--rw-grant".into(), p.rw_name,
    ];
    if let Some(b) = p.build_name {
        a.push("--build-grant".into());
        a.push(b);
    }
    if p.ingest_harness {
        a.push("--ingest-harness".into());
    }
    for name in p.egress {
        // PG6: NAMES only. gatekeeperd resolves each against sealed profiles independently, unions the
        // resolved endpoints, and fails closed if ANY is unknown.
        a.push("--egress-profile".into());
        a.push(name);
    }
    a.push("--".into());
    a.extend(p.workload);

    if p.dry_run {
        // Print an inspectable, re-runnable line (used by the proof + for debugging without privilege).
        let mut line = shquote(&gk);
        for arg in &a {
            line.push(' ');
            line.push_str(&shquote(arg));
        }
        println!("{line}");
        return 0;
    }

    // Exec the engine, replacing this process so the workload's exit status IS shrek's (PG5 fidelity:
    // a decision-plane refuse or construction failure propagates verbatim; success is never synthesized).
    let err = Command::new(&gk).args(&a).exec();
    // Only reached if exec itself failed (binary missing / not privileged to run it).
    eprintln!("shrek run: cannot exec `{gk}`: {err}");
    eprintln!("           set SHREK_GATEKEEPERD or ensure gatekeeperd is on PATH, and run with the");
    eprintln!("           privilege sandbox construction needs (mounts + netns), as the proofs do.");
    127
}

/// `shrek session --project DIR [--egress NAME]... [--subject S] [--live] [opts] -- WORKLOAD…`
///
/// Same isolation shape as `shrek run` (T2 write-through project + exec build area + sealed named
/// egress), but routed through `agentd session`: agentd makes the deterministic tier decision, attaches
/// the attested-subject stand-in, and execs gatekeeperd — which RE-CHECKS and, because the session
/// carries an identity, authors the effective-authority VIEW (`shrek session status`). agentd owns the
/// tier, so this front door does NOT claim one.
fn session(args: &[String]) -> i32 {
    let mut project: Option<String> = None;
    let mut build: Option<String> = None;
    let mut no_build = false;
    let mut egress: Vec<String> = Vec::new();
    let mut trust = String::from("T-hostile");
    let mut subject: Option<String> = None;
    let mut live = false;
    let mut ingest_harness = true;
    let mut no_chdir = false;
    let mut id: Option<String> = None;
    let mut guest_prefix = String::from("/srv");
    let mut dry_run = false;
    let mut workload: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--project" => { i += 1; project = args.get(i).cloned(); }
            "--build" => { i += 1; build = args.get(i).cloned(); }
            "--no-build" => no_build = true,
            "--egress" => { i += 1; if let Some(v) = args.get(i) { egress.push(v.clone()); } }
            "--trust" => { i += 1; if let Some(v) = args.get(i) { trust = v.clone(); } }
            "--subject" => { i += 1; subject = args.get(i).cloned(); }
            "--live" => live = true,
            "--no-ingest-harness" => ingest_harness = false,
            "--no-chdir" => no_chdir = true,
            "--id" => { i += 1; id = args.get(i).cloned(); }
            "--guest-prefix" => { i += 1; if let Some(v) = args.get(i) { guest_prefix = v.clone(); } }
            "--dry-run" => dry_run = true,
            "-h" | "--help" => { usage(); return 0; }
            "--" => { workload = args[i + 1..].to_vec(); break; }
            other => { eprintln!("shrek session: unknown arg `{other}`"); return 2; }
        }
        i += 1;
    }

    let Some(project) = project else {
        eprintln!("shrek session: --project DIR is required");
        return 2;
    };
    if workload.is_empty() {
        eprintln!("shrek session: a workload is required (… -- CMD ARG…)");
        return 2;
    }
    let project = absolutize(Path::new(&project));
    if !project.is_dir() {
        eprintln!("shrek session: project is not a directory: {}", project.display());
        return 2;
    }
    let (anchor, rw_name) = match split_beneath(&project) {
        Some(v) => v,
        None => {
            eprintln!("shrek session: project must be a named directory beneath a parent: {}", project.display());
            return 2;
        }
    };
    // Build area: a second grant beneath the same anchor (default <project>.build), created 0700.
    let build_name = if no_build {
        None
    } else {
        let build_path = match build {
            Some(b) => absolutize(Path::new(&b)),
            None => anchor.join(format!("{rw_name}.build")),
        };
        match split_beneath(&build_path) {
            Some((b_anchor, b_name)) if b_anchor == anchor => {
                if let Err(e) = ensure_dir_0700(&build_path) {
                    eprintln!("shrek session: cannot prepare build area {}: {e}", build_path.display());
                    return 2;
                }
                Some(b_name)
            }
            _ => {
                eprintln!("shrek session: --build must be a valid path beneath the project's parent ({})", anchor.display());
                return 2;
            }
        }
    };
    let id = id.unwrap_or_else(|| format!("shrek-{rw_name}"));
    let subject = subject.unwrap_or_else(|| format!("agent:{id}"));

    // cd into the project's in-guest path (same argv-preserving wrapper as `shrek run`).
    let workload = if no_chdir {
        workload
    } else {
        let guest_proj = format!("{}/{}", guest_prefix.trim_end_matches('/'), rw_name);
        let mut w = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            format!("cd {} || exit 40; exec \"$0\" \"$@\"", shquote(&guest_proj)),
        ];
        w.extend(workload);
        w
    };

    // caps == C-net iff a named egress is attached (C-net ⊇ C-proj-rw keeps project RW), else C-proj-rw.
    let caps = if egress.is_empty() { "C-proj-rw" } else { "C-net" };

    // Compose the `agentd session` argv: decision inputs (trust/caps/profile/subject/live) + the
    // gatekeeperd-sandbox passthrough (anchor + rw/build grants + egress names + ingest harness). agentd
    // decides the tier, attaches the subject, and execs gatekeeperd. No `--tier` here — agentd owns it.
    let agentd = std::env::var("SHREK_AGENTD").unwrap_or_else(|_| "agentd".to_string());
    let mut a: Vec<String> = vec![
        "session".into(),
        "--trust".into(), trust,
        "--caps".into(), caps.into(),
        "--profile".into(), caps.into(),
        "--subject".into(), subject,
        "--id".into(), id,
        "--anchor".into(), anchor.to_string_lossy().into_owned(),
        "--guest-prefix".into(), guest_prefix,
        "--rw-grant".into(), rw_name,
    ];
    if live {
        a.push("--live".into());
    }
    if let Some(b) = build_name {
        a.push("--build-grant".into());
        a.push(b);
    }
    if ingest_harness {
        a.push("--ingest-harness".into());
    }
    for name in egress {
        a.push("--egress-profile".into());
        a.push(name);
    }
    a.push("--".into());
    a.extend(workload);

    if dry_run {
        let mut line = shquote(&agentd);
        for arg in &a {
            line.push(' ');
            line.push_str(&shquote(arg));
        }
        println!("{line}");
        return 0;
    }
    let err = Command::new(&agentd).args(&a).exec();
    eprintln!("shrek session: cannot exec `{agentd}`: {err}");
    eprintln!("               set SHREK_AGENTD or ensure agentd is on PATH.");
    127
}

/// `shrek session status <id>` — READ-ONLY view of the gatekeeperd-authored effective-authority record
/// `$SHREK_SESSION_DIR/<id>.json` (default `/run/shrek/session`). Fail-closed: a missing/malformed
/// record renders "no such session". Prints a one-line summary + the raw structured record (the same
/// `shrek-session/1` JSON the Quickshell Work drawer consumes). Reads only — mints no authority.
fn session_status(args: &[String]) -> i32 {
    let Some(id) = args.first() else {
        eprintln!("usage: shrek session status <id>");
        return 2;
    };
    // Guard the id as a single safe path component (never traverse out of the session dir).
    if id.is_empty() || id == "." || id == ".." || !id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')) {
        eprintln!("shrek session status: invalid session id");
        return 2;
    }
    let dir = std::env::var("SHREK_SESSION_DIR").unwrap_or_else(|_| "/run/shrek/session".to_string());
    let path = Path::new(&dir).join(format!("{id}.json"));
    let body = match std::fs::read_to_string(&path) {
        Ok(b) if b.contains("\"shrek-session/1\"") => b,
        _ => {
            eprintln!("shrek session status: no such session `{id}` (fail-closed)");
            return 4;
        }
    };
    // Dep-free field pluck for the summary line (the record is gatekeeper-authored, single-line values).
    let v = |k: &str| json_str_value(&body, k).unwrap_or_else(|| "-".into());
    println!(
        "SESSION {id}  state={}  subject={}  tier={}  trust={}  caps={}",
        v("state"), v("subject"), v("tier"), v("trust"), v("caps")
    );
    println!(
        "  egress={} -> {}   model={}/{}   semantic={}",
        v("egress_profile"), v("egress_dst"), v("provider"), v("mode"),
        if body.contains("\"available\": true") { "available" } else { "unavailable" }
    );
    print!("{body}");
    0
}

/// Extract the string value of the FIRST `"key": "…"` in the record (dep-free). Substring-scans the
/// whole body, so it also plucks values nested in a single-line object (e.g. `model.provider`). Keys
/// used here are gatekeeper-authored and unambiguous; `tier` resolves to the effective tier (first).
fn json_str_value(body: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":");
    let idx = body.find(&needle)?;
    let rest = body[idx + needle.len()..].trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Make a path absolute without resolving symlinks (gatekeeperd does its own TOCTOU-safe resolution
/// beneath the anchor; we only need a concrete anchor dir it can open).
fn absolutize(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().map(|c| c.join(p)).unwrap_or_else(|_| p.to_path_buf())
    }
}

/// Split an absolute path into (parent-anchor, basename) — the shape gatekeeperd's `--anchor DIR
/// --rw-grant NAME` wants. Returns None for a rootless/nameless path.
fn split_beneath(p: &Path) -> Option<(PathBuf, String)> {
    let name = p.file_name()?.to_str()?.to_string();
    let parent = p.parent()?.to_path_buf();
    if name.is_empty() || parent.as_os_str().is_empty() {
        return None;
    }
    Some((parent, name))
}

/// Create `dir` (and parents) if missing, then clamp it to 0700. Idempotent.
fn ensure_dir_0700(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// Minimal POSIX single-quote for display / dry-run reproduction. Wraps in '' and escapes embedded '.
fn shquote(s: &str) -> String {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"@%_+=:,./-".contains(&b)) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

// -------------------------------------------------------------------------------------------------
// `shrek find` — the Swamp query front door (Phase-6 Swamp slice-1). Sibling of `shrek run`. Owns no
// authority: it carries a session HANDLE + query to swampd's socket and prints the caller's
// projection. swampd resolves the session's grants independently and returns ONLY in-authority hits
// (swamp.md §9). std-only, like the rest of this CLI.
// -------------------------------------------------------------------------------------------------

fn find(args: &[String]) -> i32 {
    let mut session = std::env::var("SHREK_SESSION").unwrap_or_default();
    let mut intent = "search".to_string();
    let mut scope = String::new();
    let mut limit = 50usize;
    let mut socket = std::env::var("SWAMP_QUERY_SOCK").unwrap_or_else(|_| "/run/swamp/query.sock".into());
    let mut terms: Vec<String> = Vec::new();

    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--session" => session = it.next().cloned().unwrap_or_default(),
            "--intent" => intent = it.next().cloned().unwrap_or_default(),
            "--scope" => scope = it.next().cloned().unwrap_or_default(),
            "--limit" => {
                limit = it.next().and_then(|s| s.parse().ok()).unwrap_or(50);
            }
            "--socket" => socket = it.next().cloned().unwrap_or_default(),
            "-h" | "--help" => {
                usage();
                return 0;
            }
            other if other.starts_with("--") => {
                eprintln!("shrek find: unknown option {other}");
                return 2;
            }
            other => terms.push(other.to_string()),
        }
    }

    if session.is_empty() {
        eprintln!("shrek find: --session ID required (or set $SHREK_SESSION)");
        return 2;
    }
    if intent != "search" && intent != "discover" {
        eprintln!("shrek find: --intent must be search|discover");
        return 2;
    }
    if terms.is_empty() {
        eprintln!("shrek find: no query terms");
        return 2;
    }

    use std::io::{BufRead, BufReader, Write};
    let mut stream = match std::os::unix::net::UnixStream::connect(&socket) {
        Ok(s) => s,
        Err(e) => {
            // Availability plane (swamp.md §10): a down swampd means search is unavailable, which is
            // the safe direction — never a failure that grants authority. Report and exit non-zero.
            eprintln!("shrek find: swampd query socket unavailable ({socket}): {e}");
            return 1;
        }
    };

    let scope_field = if scope.is_empty() { "-".to_string() } else { scope };
    let req = format!(
        "QUERY 1\nsession {session}\nintent {intent}\nscope {scope_field}\nlimit {limit}\nq {}\nEND\n",
        terms.join(" ")
    );
    if let Err(e) = stream.write_all(req.as_bytes()) {
        eprintln!("shrek find: write failed: {e}");
        return 1;
    }
    let _ = stream.flush();

    let reader = BufReader::new(&stream);
    let mut count: Option<usize> = None;
    let mut freshness: Option<String> = None;
    let mut printed = 0usize;
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("shrek find: read failed: {e}");
                return 1;
            }
        };
        if let Some(rest) = line.strip_prefix("ERROR ") {
            eprintln!("shrek find: swampd refused: {rest}");
            return 1;
        }
        if let Some(rest) = line.strip_prefix("RESULT ") {
            count = rest.trim().parse().ok();
            continue;
        }
        // Slice-3: index freshness (fresh|stale). Surfaced so a miss against a stale index is not read
        // as proof of absence. A pre-slice-3 swampd omits it entirely.
        if let Some(rest) = line.strip_prefix("freshness ") {
            freshness = Some(rest.trim().to_string());
            continue;
        }
        if line == "END" {
            break;
        }
        if let Some(rest) = line.strip_prefix("hit ") {
            let (path, snippet) = rest.split_once('\t').unwrap_or((rest, ""));
            if snippet.is_empty() {
                println!("{path}");
            } else {
                println!("{path}\t{snippet}");
            }
            printed += 1;
        }
    }

    match count {
        Some(n) => {
            let fresh = freshness.as_deref().unwrap_or("fresh");
            if fresh == "fresh" {
                eprintln!("shrek find: {n} hit(s) in session {session}'s authority");
            } else {
                eprintln!(
                    "shrek find: {n} hit(s) in session {session}'s authority [index freshness={fresh}: \
                     results may be behind the filesystem — a miss is not proof of absence]"
                );
            }
            let _ = printed;
            0
        }
        None => {
            eprintln!("shrek find: no well-formed response from swampd");
            1
        }
    }
}
