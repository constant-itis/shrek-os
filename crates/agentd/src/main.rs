//! agentd — agent identity + isolation resolver (Phase 8; decision plane in Phase-5 slice-2).
//!
//! agentd is the UNPRIVILEGED resolver: it maps `(trust, caps) → tier` via the matrix + floor
//! (`shrek-tier`) and emits a construction request for gatekeeperd to RE-CHECK and build
//! (isolation.md §5, §7). It resolves; it never constructs a sandbox and never holds privilege.
//!
//! Phase-5 slice-2 scope — the `resolve` subcommand only:
//!   step 1: validate `caps ⊆ granted-profile`            (the grant ceiling; refuse if exceeded)
//!   step 2: `tier = max(matrix[trust][caps], floor(trust), escalation)`
//!   output: the gatekeeperd construction-request argv on stdout (the CLI seam; the socket verb +
//!           crypto seal are slice #5). A refusal emits NOTHING to stdout and a decision to stderr.
//!
//! Trust band must be integrity-sourced; unknown ⇒ `T-hostile` (fail-high) — realized by
//! `TrustBand::parse` (security-model.md §6/B1). HOW the band is derived/attested is OPEN (B1, owed
//! to agents.md) and out of this slice: here `--trust` is an explicit input, and the fail-high
//! parse guarantees a garbled/absent band can only raise the wall.
//!
//! With no subcommand the daemon is the disabled Phase-1 scaffold (holds no privilege).

use shrek_tier::{effective_tier, CapsProfile, Tier, TrustBand};

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().map(String::as_str) == Some("resolve") {
        std::process::exit(resolve_cli(&argv[1..]));
    }

    eprintln!("agentd: Phase-1 disabled scaffold — resolves nothing, holds no privilege");
    loop {
        std::thread::park();
    }
}

/// `agentd resolve --trust T --caps C [--profile C] [--escalate Tn] --id X --anchor DIR
/// --grant NAME [--grant NAME]... [--guest-prefix DIR] -- WORKLOAD...`
///
/// On success prints the `gatekeeperd sandbox` argv to stdout and exits 0. On a grant-check refusal
/// prints an `AGENTD-DECISION refused …` line to stderr, nothing to stdout, and exits nonzero.
fn resolve_cli(args: &[String]) -> i32 {
    let mut trust_s = String::new();
    let mut caps_s = String::new();
    let mut profile_s: Option<String> = None;
    let mut escalate_s: Option<String> = None;
    let mut id = String::from("s0");
    let mut anchor = String::new();
    let mut guest_prefix = String::from("/srv");
    let mut grants: Vec<String> = Vec::new();
    let mut workload: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--trust" => { i += 1; trust_s = args.get(i).cloned().unwrap_or_default(); }
            "--caps" => { i += 1; caps_s = args.get(i).cloned().unwrap_or_default(); }
            "--profile" => { i += 1; profile_s = args.get(i).cloned(); }
            "--escalate" => { i += 1; escalate_s = args.get(i).cloned(); }
            "--id" => { i += 1; id = args.get(i).cloned().unwrap_or(id); }
            "--anchor" => { i += 1; if let Some(v) = args.get(i) { anchor = v.clone(); } }
            "--guest-prefix" => { i += 1; if let Some(v) = args.get(i) { guest_prefix = v.clone(); } }
            "--grant" => { i += 1; if let Some(v) = args.get(i) { grants.push(v.clone()); } }
            "--" => { workload = args[i + 1..].to_vec(); break; }
            other => { eprintln!("agentd/resolve: unknown arg {other}"); return 2; }
        }
        i += 1;
    }

    if trust_s.is_empty() || caps_s.is_empty() || anchor.is_empty() || grants.is_empty() || workload.is_empty() {
        eprintln!(
            "usage: agentd resolve --trust T --caps C [--profile C] [--escalate Tn] \
             --anchor DIR --grant NAME [...] [--guest-prefix DIR] -- WORKLOAD..."
        );
        return 2;
    }

    // Fail-high parse: garbled/absent trust ⇒ Hostile, garbled/absent caps ⇒ Broad.
    let trust = TrustBand::parse(&trust_s);
    let caps = CapsProfile::parse(&caps_s);
    // The granted ceiling. Absent ⇒ default to the requested caps (grant == request, check passes
    // trivially). A real profile store is Phase 8 (agents.md); this is the explicit-input stand-in.
    let profile = profile_s.as_deref().map(CapsProfile::parse).unwrap_or(caps);
    let escalation: Option<Tier> = match escalate_s.as_deref() {
        None => None,
        Some(s) => match Tier::parse(s) {
            Some(t) => Some(t),
            None => { eprintln!("agentd/resolve: bad --escalate {s:?}"); return 2; }
        },
    };

    // Step 1: caps ⊆ granted profile. Exceeding the grant is refused BEFORE any tier is emitted.
    if !caps.subset_of(profile) {
        eprintln!(
            "AGENTD-DECISION refused reason=caps-exceed-profile trust={} caps={} profile={}",
            trust.label(), caps.label(), profile.label()
        );
        return 11;
    }

    // Step 2: deterministic tier. No LLM, no vibes (isolation.md §5).
    let tier = effective_tier(trust, caps, escalation);
    eprintln!(
        "AGENTD-DECISION resolved tier={} trust={} caps={} profile={} escalation={}",
        tier.label(), trust.label(), caps.label(), profile.label(),
        escalation.map(|t| t.label()).unwrap_or("-")
    );

    // Output: the construction-request argv for gatekeeperd. gatekeeperd RE-CHECKS all of this
    // against its own compiled-in (sealed) matrix — it trusts none of these numbers (isolation.md §7).
    let mut out: Vec<String> = vec![
        "--tier".into(), tier.label().into(),
        "--trust".into(), trust.label().into(),
        "--caps".into(), caps.label().into(),
        "--profile".into(), profile.label().into(),
        "--id".into(), id,
        "--anchor".into(), anchor,
        "--guest-prefix".into(), guest_prefix,
    ];
    for g in &grants {
        out.push("--grant".into());
        out.push(g.clone());
    }
    out.push("--".into());
    out.extend(workload);
    println!("{}", out.join(" "));
    0
}
