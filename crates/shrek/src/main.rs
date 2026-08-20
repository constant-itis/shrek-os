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
    eprintln!("    --egress NAME        attach a SEALED egress profile by name (else loopback-only)");
    eprintln!("    --trust BAND         claimed trust band (default: T-hostile)");
    eprintln!("    --tier Tn            claimed tier the decision plane re-checks (default: T2)");
    eprintln!("    --no-ingest-harness  derive the band from the entrypoint arm, not the sealed harness");
    eprintln!("    --no-chdir           do not cd into the project before the workload runs");
    eprintln!("    --id NAME            sandbox id (default: derived from the project name)");
    eprintln!("    --guest-prefix DIR   in-guest mount prefix for grants (default: /srv)");
    eprintln!("    --dry-run            print the composed `gatekeeperd sandbox` argv and exit");
    eprintln!();
    eprintln!("  (planned) shrek find | history | related | status; shrek audit --agent");
}

/// Parsed `shrek run` request, resolved to what the engine needs.
struct Plan {
    anchor: PathBuf,
    rw_name: String,
    build_name: Option<String>,
    egress: Option<String>,
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
    let mut egress: Option<String> = None;
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
            "--egress" => { i += 1; egress = args.get(i).cloned(); }
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

/// Compose the `gatekeeperd sandbox` argv and exec it. `caps` is `C-net` iff a named egress profile
/// is attached (the ordered lattice: C-net ⊇ C-proj-rw, so a netted coding cell keeps project RW),
/// else `C-proj-rw`. `--profile` mirrors `--caps` (the declared profile the decision plane re-checks).
fn dispatch(p: Plan) -> i32 {
    let caps = if p.egress.is_some() { "C-net" } else { "C-proj-rw" };

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
    if let Some(name) = p.egress {
        // PG6: a NAME only. gatekeeperd resolves it against sealed profiles and fails closed if unknown.
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
