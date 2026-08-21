//! session_view — gatekeeperd authors the ephemeral **effective-authority view** for a constructed
//! agent session (Phase 8 · Slice 1, docs/phase8-slice1-agent-session.md §2.3). This is the DISPLAY
//! projection of the same re-checked decision that writes `authority_record` (grants) and
//! `net_binding` (egress) — the enforcement authority stays those two; this record is the read-only
//! view a human (`shrek session status`) or the future Quickshell Work drawer reads.
//!
//! Trust shape mirrors `authority_record`: the privileged broker (the trust anchor) writes it into
//! `/run/shrek/session/<id>.json`, `root:swamp` mode 0640 inside a `root:swamp` 0750 dir — a non-root
//! workload (a different uid) can neither forge (C1) nor widen (C2) it, and it is NOT mounted into the
//! sandbox, so the workload cannot even see it. Ephemeral: written once at CONSTRUCT, removed at
//! TEARDOWN (C3). The schema `shrek-session/1` is stable structured JSON (additive-only) because it is
//! also the Work-drawer data-model prototype — a second read-only consumer of this exact record.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{chown, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::authority_record::{swamp_ids, valid_session_id};

/// Default location of the ephemeral session-view records. Overridable for the host/container repro
/// (no systemd) via `SHREK_SESSION_DIR`, mirroring `SHREK_AUTHORITY_DIR`.
pub fn view_dir() -> PathBuf {
    std::env::var("SHREK_SESSION_DIR")
        .unwrap_or_else(|_| "/run/shrek/session".to_string())
        .into()
}

/// The re-checked effective decision, projected for display. gatekeeperd fills this from the SAME
/// decision it constructs against (sandbox.rs), so the view can never diverge from enforcement truth.
/// All fields are descriptive labels — this struct mints no capability (D4: `semantic ≤ data`).
#[derive(Clone, Debug, Default)]
pub struct SessionView {
    /// Session id (== the record filename stem; the caller's `--id`, e.g. `s0`).
    pub session: String,
    /// Attested-subject stand-in (`--subject`); non-authoritative display only this slice.
    pub subject: String,
    pub tier: String,
    pub trust: String,
    pub caps: String,
    pub profile: String,
    /// Canonical host grant paths (the realized mounts == `authority_record`'s grants).
    pub grants: Vec<String>,
    pub egress_profile: String,
    pub egress_dst: String,
    pub workload: Vec<String>,
    pub provider: String,
    /// `deterministic` (the gate, canned responder) or `live` (opt-in smoke).
    pub model_mode: String,
    pub semantic_available: bool,
    /// `fts+semantic` when swamp-capable, else `fts`.
    pub semantic_tier: String,
}

/// The subset of the display projection known at DECISION time (`sandbox.rs`): the re-checked
/// tier/trust/caps/profile, the attested-subject stand-in, and the model mode. `t2_plane` fills the
/// rest (grants, egress, workload, semantic) from the `T2Spec` at the construct seam, then writes one
/// `SessionView`. Carried on `T2Spec` as `Option` — only the decision-plane (T2) path sets it.
#[derive(Clone, Debug, Default)]
pub struct SessionMeta {
    pub subject: String,
    pub tier: String,
    pub trust: String,
    pub caps: String,
    pub profile: String,
    /// `deterministic` (the gate, canned responder) or `live` (opt-in smoke).
    pub model_mode: String,
}

/// Minimal, correct JSON string escaping (dep-free): backslash, double-quote, and control chars. A
/// workload arg or path could carry a control byte; without escaping that yields invalid JSON.
fn jstr(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn jarr(items: &[String]) -> String {
    let parts: Vec<String> = items.iter().map(|s| jstr(s)).collect();
    format!("[{}]", parts.join(", "))
}

/// Serialize the view to the stable `shrek-session/1` JSON. Deterministic: fixed field order, no
/// wall-clock timestamp (the gate must be reproducible; `state` is always `active` because the record
/// exists IFF the session is live — absence == ended).
pub fn to_json(v: &SessionView) -> String {
    format!(
        concat!(
            "{{\n",
            "  \"schema\": \"shrek-session/1\",\n",
            "  \"session\": {session},\n",
            "  \"state\": \"active\",\n",
            "  \"subject\": {subject},\n",
            "  \"effective\": {{\n",
            "    \"tier\": {tier},\n",
            "    \"trust\": {trust},\n",
            "    \"caps\": {caps},\n",
            "    \"profile\": {profile},\n",
            "    \"grants\": {grants},\n",
            "    \"egress_profile\": {egress_profile},\n",
            "    \"egress_dst\": {egress_dst}\n",
            "  }},\n",
            "  \"workload\": {workload},\n",
            "  \"model\": {{ \"provider\": {provider}, \"path\": \"brokered\", \"mode\": {mode} }},\n",
            "  \"semantic\": {{ \"available\": {avail}, \"freshness\": \"live\", \"tier\": {stier} }}\n",
            "}}\n",
        ),
        session = jstr(&v.session),
        subject = jstr(&v.subject),
        tier = jstr(&v.tier),
        trust = jstr(&v.trust),
        caps = jstr(&v.caps),
        profile = jstr(&v.profile),
        grants = jarr(&v.grants),
        egress_profile = jstr(&v.egress_profile),
        egress_dst = jstr(&v.egress_dst),
        workload = jarr(&v.workload),
        provider = jstr(&v.provider),
        mode = jstr(&v.model_mode),
        avail = if v.semantic_available { "true" } else { "false" },
        stier = jstr(&v.semantic_tier),
    )
}

/// The record's filename: `<id>.json`. The id is validated as a single safe path component first.
fn record_path(dir: &Path, session_id: &str) -> PathBuf {
    dir.join(format!("{session_id}.json"))
}

/// Write the session-view record for `view.session`. Overwrites any prior record for the same session
/// (a session's authority is set once at construction). Best-effort `root:swamp` ownership; mode is
/// always 0640 so a non-owner, non-group process cannot read it (nor write it — the C1/C2 basis).
pub fn write_view(dir: &Path, view: &SessionView) -> io::Result<PathBuf> {
    if !valid_session_id(&view.session) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid session id"));
    }
    fs::create_dir_all(dir)?;
    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o750));
    if let Some((uid, gid)) = swamp_ids() {
        let _ = chown(dir, Some(uid), Some(gid));
    }

    let body = to_json(view);
    let path = record_path(dir, &view.session);
    // temp + rename so a reader never sees a half-written record.
    let tmp = dir.join(format!(".{}.json.tmp", view.session));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(body.as_bytes())?;
        f.sync_all()?;
    }
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o640))?;
    if let Some((uid, gid)) = swamp_ids() {
        let _ = chown(&tmp, Some(uid), Some(gid));
    }
    fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Remove a session's view record (teardown). Idempotent — a missing record is not an error (C3).
pub fn remove_view(dir: &Path, session_id: &str) -> io::Result<()> {
    if !valid_session_id(session_id) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid session id"));
    }
    match fs::remove_file(record_path(dir, session_id)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// CLI: `gatekeeperd session-view --session <id> [--rm] [--dir D] --subject S --tier T --trust Tr
/// --caps C --profile P [--grant CANON]... --egress-profile N --egress-dst D [--workload-arg A]...
/// --provider P --mode M [--semantic-available] [--semantic-tier T]`. Privileged (run as the broker).
/// Exists so the host oracle (C1–C3) and tests can drive write/remove directly, exactly as
/// `authority-record` / `net-binding` do. Returns a process exit code.
pub fn cli(args: &[String]) -> i32 {
    let mut v = SessionView::default();
    let mut dir = view_dir();
    let mut rm = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut next = || it.next().cloned().unwrap_or_default();
        match a.as_str() {
            "--session" => v.session = next(),
            "--dir" => dir = PathBuf::from(next()),
            "--rm" => rm = true,
            "--subject" => v.subject = next(),
            "--tier" => v.tier = next(),
            "--trust" => v.trust = next(),
            "--caps" => v.caps = next(),
            "--profile" => v.profile = next(),
            "--grant" => v.grants.push(next()),
            "--egress-profile" => v.egress_profile = next(),
            "--egress-dst" => v.egress_dst = next(),
            "--workload-arg" => v.workload.push(next()),
            "--provider" => v.provider = next(),
            "--mode" => v.model_mode = next(),
            "--semantic-available" => v.semantic_available = true,
            "--semantic-tier" => v.semantic_tier = next(),
            other => {
                eprintln!("session-view: unknown arg {other}");
                return 2;
            }
        }
    }
    if v.session.is_empty() {
        eprintln!("session-view: --session <id> required");
        return 2;
    }
    if rm {
        return match remove_view(&dir, &v.session) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("session-view: rm failed: {e}");
                1
            }
        };
    }
    match write_view(&dir, &v) {
        Ok(p) => {
            println!("session-view: wrote {}", p.display());
            0
        }
        Err(e) => {
            eprintln!("session-view: write failed: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str) -> SessionView {
        SessionView {
            session: id.to_string(),
            subject: "dev-standin".into(),
            tier: "T2".into(),
            trust: "T-untrust".into(),
            caps: "cnet".into(),
            profile: "cnet".into(),
            grants: vec!["/canon/project".into()],
            egress_profile: "model-anthropic".into(),
            egress_dst: "shrek-model-proxy:8200".into(),
            workload: vec!["coder".into(), "--provider".into(), "anthropic".into()],
            provider: "anthropic".into(),
            model_mode: "deterministic".into(),
            semantic_available: true,
            semantic_tier: "fts+semantic".into(),
        }
    }

    #[test]
    fn json_is_wellformed_and_stable() {
        let j = to_json(&sample("s0"));
        assert!(j.contains("\"schema\": \"shrek-session/1\""));
        assert!(j.contains("\"session\": \"s0\""));
        assert!(j.contains("\"state\": \"active\""));
        assert!(j.contains("\"egress_dst\": \"shrek-model-proxy:8200\""));
        assert!(j.contains("\"mode\": \"deterministic\""));
        assert!(j.contains("\"available\": true"));
        // deterministic: two serializations are byte-identical (no timestamp/nondeterminism).
        assert_eq!(j, to_json(&sample("s0")));
    }

    #[test]
    fn json_escapes_control_and_quotes() {
        let mut v = sample("s0");
        v.workload = vec!["arg\"with\"quotes".into(), "line\nbreak".into()];
        let j = to_json(&v);
        assert!(j.contains("arg\\\"with\\\"quotes"));
        assert!(j.contains("line\\nbreak"));
        assert!(!j.contains("line\nbreak\","));
    }

    #[test]
    fn write_then_read_roundtrip_mode_0640() {
        let tmp = std::env::temp_dir().join(format!("sess-view-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let rec = write_view(&tmp, &sample("s0")).unwrap();
        assert!(rec.ends_with("s0.json"));
        let body = fs::read_to_string(&rec).unwrap();
        assert!(body.contains("\"schema\": \"shrek-session/1\""));
        let mode = fs::metadata(&rec).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
        // rm is idempotent (C3)
        remove_view(&tmp, "s0").unwrap();
        remove_view(&tmp, "s0").unwrap();
        assert!(!rec.exists());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn invalid_session_id_refused() {
        let tmp = std::env::temp_dir().join(format!("sess-view-test2-{}", std::process::id()));
        let mut v = sample("../escape");
        assert!(write_view(&tmp, &v).is_err());
        v.session = "".into();
        assert!(write_view(&tmp, &v).is_err());
        assert!(remove_view(&tmp, "a/b").is_err());
    }
}
