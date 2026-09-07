//! desktop_egress — the gatekeeperd side of the ADR-007 S4 CONSOLE-CEREMONY egress tier.
//!
//! The one-click desktop-egress socket (egressd, uid-1000) admits only Tier-B (`weather`). The
//! high-consequence tier — `web-browsing` (broad egress) and any user-authored raw `host:proto:port`
//! destination — is granted ONLY through the full SAK/VT console ceremony. This module is that ceremony's
//! request family: it reuses the shared, security-critical ceremony core (`consent::run_socket_consent_with`
//! — SAK arm, kernel-owned VT, diff render, typed 6-digit confirmation, PID-reuse/peer-liveness binding,
//! escalating anti-flood cooldown), supplying only the desktop-egress precheck (what to render) and commit
//! (what to do on a confirmed OK).
//!
//! Boundary (MF-1/MF-6): a NEW verb family (`desktop-egress`, NOT the bench `network` verb — no
//! bench-name collision, no shared cooldown key), and `high_authority() == true` for every op (each
//! demands the typed code, never a bare `y`; `web-browsing` also sets `trifecta` so the renderer shows
//! the "read your files AND reach the network" exfil warning). On a confirmed ceremony, commit RELAYS the
//! op to the egressd daemon over its root-gated socket (`egressd ask confirmed-*`, a capless client) —
//! the ABSOLUTE sealed binary with a CLEARED environment and argv taken from the VALIDATED plan (never
//! re-read from the wire). The DAEMON is the sole nft mutator, so gatekeeperd holds no `CAP_NET_ADMIN`;
//! egressd re-validates the tier + grammar (defense in depth) and no inherited env can redirect the client.

use crate::bench_plane::{self, AuthorityPlan};
use crate::linux_uapi::Ucred;
use shrek_policy::desktop_egress::{bless_tier, parse_raw_triple, BlessTier};
use shrek_policy::egress_capability::{
    is_storage_host, is_system_reserved_host, parse_manifest, valid_capability_token, Deliver,
};
use std::io::Write;
use std::os::fd::RawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

/// The sealed egressd binary the ceremony commit execs. Absolute (never a PATH lookup). Overridable ONLY
/// in the `oracle-env` build so the host oracle can point at the freshly-built binary; the shipped image
/// compiles the override out (`bench_env` is a const `None`), so this is unconditional in production.
fn egressd_bin() -> String {
    crate::bench_record::bench_env("SHREK_EGRESSD_BIN").unwrap_or_else(|| "/usr/libexec/shrek/egressd".to_string())
}

/// The VOLATILE owner-manifest staging dir (`root:root 0700` under `/run`, cleared on reboot). This is the
/// CONTRACT boundary with egressd's `catalog::staging_cap_dir` (ADR-009 §4.2, #3198 decision-a) — a
/// confirmed manifest install writes `<name>.capability` HERE; the daemon's `confirmed-manifest-install`
/// verb reads it back, re-parses, and re-validates (only the NAME rides egressd's socket, never the bytes).
/// The path + the `oracle-env` override variable are kept byte-identical to egressd's so the host oracle
/// can redirect BOTH sides at once; production compiles the override out (`bench_env` is a const `None`).
fn staging_dir() -> PathBuf {
    crate::bench_record::bench_env("SHREK_EGRESS_CAP_STAGING")
        .unwrap_or_else(|| "/run/shrek/egress-manifest-staging".to_string())
        .into()
}

/// Atomically stage a CONFIRMED owner-manifest candidate to `<staging>/<name>.capability` (`root:root
/// 0600` in the `0700` dir) so the daemon's `confirmed-manifest-install` verb can read it. `name` is an
/// already-validated [`valid_capability_token`], so the join is a single path component — no traversal.
/// Best-effort chown/mode (gatekeeperd runs as root); a write failure ⇒ `Err` and the caller does NOT
/// relay (fail-closed: no half-staged candidate is ever handed to the daemon).
fn stage_manifest(name: &str, text: &str) -> std::io::Result<()> {
    let dir = staging_dir();
    std::fs::create_dir_all(&dir)?;
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    let path = dir.join(format!("{name}.capability"));
    let tmp = dir.join(format!(".{name}.capability.tmp"));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// A validated desktop-egress ceremony op. Carries the ALREADY-VALIDATED subject string (profile name or
/// raw `host:proto:port` wire form) so commit builds egressd's argv from the rendered plan, not the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    BlessProfile(String),
    UnblessProfile(String),
    AddRaw(String),
    RemoveRaw(String),
    /// ADR-009 §4.2 S3 — install an OWNER capability manifest. Carries the VALIDATED candidate bytes the
    /// human saw rendered on the console, so commit stages exactly those (never a re-read that could drift
    /// between the OK and the apply — the raw-triple "argv from the plan" doctrine, extended to the file).
    ManifestInstall { name: String, text: String },
    /// ADR-009 §4.2 S3 — remove an OWNER capability manifest by name (authority-REDUCING; no bytes).
    ManifestRemove(String),
}

/// The ceremony-header action line (rendered on the VT; sanitized by the renderer).
pub fn action(op: &Op) -> String {
    match op {
        Op::BlessProfile(p) => format!("ALLOW broad desktop egress: bless '{p}'"),
        Op::UnblessProfile(p) => format!("REVOKE desktop egress: unbless '{p}'"),
        Op::AddRaw(t) => format!("ALLOW a raw desktop destination: {t}"),
        Op::RemoveRaw(t) => format!("REMOVE a raw desktop destination: {t}"),
        Op::ManifestInstall { name, .. } => format!("INSTALL the owner network capability '{name}'"),
        Op::ManifestRemove(name) => format!("REMOVE the owner network capability '{name}'"),
    }
}

/// Materialize a CONFIRMED desktop-egress ceremony by RELAYING it to the egressd daemon over its
/// root-gated socket (ADR-007 S6 fix #4). We exec `egressd ask confirmed-*` — a CAPLESS socket client
/// that connects, writes one request line, and reads the reply — NOT a CLI that mutates nft. The daemon
/// (the sole nft mutator, already holding `CAP_NET_ADMIN`) does the store write + apply in-process and
/// authorizes on our ROOT peer uid. So gatekeeperd needs NO `CAP_NET_ADMIN`, and no transient process
/// edits the ROOT-netns table under the broker's cap umbrella. Runs env-CLEARED with the absolute binary
/// and argv from the VALIDATED op (never re-read from the wire); the daemon re-checks tier + grammar
/// (defense in depth). Returns the client's exit code (0 = daemon replied OK / applied+persisted).
pub fn commit(op: &Op) -> i32 {
    let (verb, arg) = match op {
        Op::BlessProfile(p) => ("confirmed-bless", p.clone()),
        Op::UnblessProfile(p) => ("confirmed-unbless", p.clone()),
        Op::AddRaw(t) => ("confirmed-add-raw", t.clone()),
        Op::RemoveRaw(t) => ("confirmed-remove-raw", t.clone()),
        // Owner-manifest install: STAGE the confirmed bytes first (fail-closed — never relay a
        // half-staged candidate), then relay only the NAME. The daemon reads staging, re-parses, enforces
        // the §4.4 install-refuses (defense in depth), commits, and clears staging.
        Op::ManifestInstall { name, text } => {
            if let Err(e) = stage_manifest(name, text) {
                eprintln!("gatekeeperd/desktop-egress: stage manifest {name}: {e}");
                return 1;
            }
            ("confirmed-manifest-install", name.clone())
        }
        Op::ManifestRemove(name) => ("confirmed-manifest-remove", name.clone()),
    };
    match Command::new(egressd_bin()).env_clear().arg("ask").arg(verb).arg(&arg).status() {
        Ok(st) => st.code().unwrap_or(1),
        Err(e) => {
            eprintln!("gatekeeperd/desktop-egress: exec egressd ask {verb}: {e}");
            1
        }
    }
}

/// Validate a desktop-egress ceremony request and build the plan the renderer shows. Fail-closed: an
/// invalid profile/triple denies BEFORE the human is ever asked (no SAK, no VT). `verb` ∈
/// {bless, unbless, add-raw, remove-raw}; `rest[0]` is the profile name or the raw `host:proto:port`.
pub(crate) fn precheck(verb: &str, rest: &[String]) -> Result<AuthorityPlan, (i32, String)> {
    let arg = rest.first().map(String::as_str).unwrap_or("");
    if arg.is_empty() {
        return Err((2, format!("desktop-egress {verb} needs an argument")));
    }
    match verb {
        "bless" | "unbless" => {
            // Only a sealed CEREMONY-tier profile (web-browsing today) is grantable here. weather is
            // one-click (the socket path); baseline is always-on; unknown is fail-closed. This keeps the
            // ceremony verb from becoming a second front door for the lower tiers (tier-matrix integrity).
            if bless_tier(arg) != Some(BlessTier::Ceremony) {
                return Err((2, format!("{arg} is not a console-ceremony profile")));
            }
            let rows = vec![
                ("Profile".to_string(), arg.to_string()),
                (
                    "Effect".to_string(),
                    "opens BROAD internet access for the browser — it can reach ANY host".to_string(),
                ),
            ];
            let op = if verb == "bless" {
                Op::BlessProfile(arg.to_string())
            } else {
                Op::UnblessProfile(arg.to_string())
            };
            // trifecta=true → the renderer adds the "can READ your files AND reach the network" warning;
            // a broad-egress bless on a desktop that already reads the user's files completes that pair.
            Ok(bench_plane::desktop_egress_plan(op, arg.to_string(), rows, true, Vec::new()))
        }
        "add-raw" | "remove-raw" => {
            // Re-parse through THE one sealed grammar; the wire string is uid-1000-authored.
            let t = parse_raw_triple(arg).map_err(|e| (2, format!("raw destination: {e}")))?;
            let wire = t.to_wire();
            let rows = vec![
                ("Host".to_string(), t.host.clone()),
                ("Protocol".to_string(), t.proto.label().to_string()),
                ("Port".to_string(), t.port.to_string()),
            ];
            let op = if verb == "add-raw" { Op::AddRaw(wire.clone()) } else { Op::RemoveRaw(wire.clone()) };
            Ok(bench_plane::desktop_egress_plan(op, wire, rows, true, Vec::new()))
        }
        "manifest-install" => manifest_install_precheck(arg, rest.get(1).map(String::as_str)),
        "manifest-remove" => {
            // Authority-REDUCING: name only, no bytes. Refuse a malformed token before the human is asked;
            // the daemon's remove is idempotent + defensive so a name for an absent capability is a no-op.
            if !valid_capability_token(arg) {
                return Err((2, format!("`{arg}` is not a capability name")));
            }
            let rows = vec![("Capability".to_string(), arg.to_string())];
            Ok(bench_plane::desktop_egress_plan(Op::ManifestRemove(arg.to_string()), arg.to_string(), rows, false, Vec::new()))
        }
        _ => Err((2, format!("unknown desktop-egress verb {verb:?}"))),
    }
}

/// The manifest-install precheck (ADR-009 S3). `name` is the staging/relay key; `text` is the candidate
/// manifest the panel pct-encoded onto the wire. Fail-closed BEFORE any SAK/VT (no human is asked for a
/// doomed install): parse through the ONE sealed grammar, require the parsed `name` match the wire key,
/// and run the PURE §4.4 belt (`deliver hosts` refused; any [`is_system_reserved_host`] host refused).
/// The daemon RE-validates authoritatively at commit (incl. the sealed-CATALOG collision that needs an
/// fs read — kept there, not duplicated here). Builds the full root-authored CARD (title/purpose/feature/
/// hosts, each sanitized by the renderer) + two warnings: the ADR-009 §8 storage-host nudge (advisory,
/// per matching host) and the ALWAYS-present "toggle ≠ live intent" line (OQ-1: installing mints a
/// one-click toggle a compromised session could later flip — the ceremony text must say so).
fn manifest_install_precheck(name: &str, text: Option<&str>) -> Result<AuthorityPlan, (i32, String)> {
    if !valid_capability_token(name) {
        return Err((2, format!("`{name}` is not a capability name")));
    }
    let Some(text) = text else {
        return Err((2, "manifest-install needs the manifest text".to_string()));
    };
    let m = parse_manifest(text).map_err(|e| (2, format!("invalid manifest: {}", e.reason())))?;
    if m.name != name {
        return Err((2, format!("staged name `{name}` != manifest name `{}`", m.name)));
    }
    // PURE §4.4 belt (the daemon re-checks + adds the sealed-catalog collision at commit).
    if m.deliver == Deliver::Hosts {
        return Err((2, "`deliver hosts` is a sealed-only affordance; an owner manifest must use `deliver none`".to_string()));
    }
    for r in &m.rules {
        if is_system_reserved_host(&r.host) {
            return Err((2, format!("host `{}` is reserved by sealed/system machinery", r.host)));
        }
    }
    // The root-authored card the human reads on the un-spoofable console.
    let mut rows = vec![
        ("Capability".to_string(), m.name.clone()),
        ("Title".to_string(), m.title.clone()),
        ("Purpose".to_string(), m.purpose.clone()),
        ("Feature".to_string(), m.feature.clone()),
        ("Tier".to_string(), m.tier.as_str().to_string()),
    ];
    for r in &m.rules {
        rows.push(("Reaches".to_string(), format!("{} {}/{}", r.host, r.proto.label(), r.port)));
    }
    // Warnings: the always-present toggle≠live-intent line first, then a storage-host nudge per match.
    let mut warnings = vec![
        "After install, this can be switched on with ONE click — including by a program in your session, \
         not only by you. A toggle is not proof a human is here."
            .to_string(),
    ];
    for r in &m.rules {
        if is_storage_host(&r.host) {
            warnings.push(format!(
                "'{}' is a general-purpose upload/download host — anything with this capability could use \
                 it to move data off this box.",
                r.host
            ));
        }
    }
    let op = Op::ManifestInstall { name: m.name.clone(), text: text.to_string() };
    Ok(bench_plane::desktop_egress_plan(op, m.name, rows, false, warnings))
}

/// Socket entry (mirrors [`bench_plane::dispatch_socket`]): `argv[0]` = subverb, `argv[1]` = the profile
/// or raw triple. Routes into the shared ceremony core with the desktop-egress precheck/commit + the
/// `desktop-egress` wire prefix. The peer gate (dev uid), cooldown, tuple-binding, SAK/VT ceremony, and
/// audit all live in the shared core.
pub fn dispatch_socket(cred: Ucred, peer_fd: RawFd, argv: &[String]) -> (i32, Vec<String>) {
    let verb = argv.first().map(String::as_str).unwrap_or("").to_string();
    let rest = argv[argv.len().min(1)..].to_vec();
    match verb.as_str() {
        "bless" | "unbless" | "add-raw" | "remove-raw" | "manifest-install" | "manifest-remove" => {
            crate::consent::run_socket_consent_with(
                cred,
                peer_fd,
                &verb,
                "desktop-egress",
                || precheck(&verb, &rest),
                bench_plane::commit_authority,
            )
        }
        other => (2, vec![format!("RESULT desktop-egress-{other} - refused unknown-verb")]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precheck_bless_only_admits_ceremony_tier() {
        // web-browsing is ceremony-tier → a plan is built, high-authority (typed code), trifecta warning.
        let p = precheck("bless", &["web-browsing".into()]).unwrap();
        assert!(p.high_authority(), "web-browsing bless demands the typed code, not a bare y");
        assert!(p.trifecta, "broad egress + desktop file read ⇒ the exfil warning renders");
        assert!(p.action().contains("web-browsing"));
        // weather is one-click (the socket path) → REFUSED here, so the ceremony isn't a weather backdoor.
        assert!(precheck("bless", &["weather".into()]).is_err());
        // baseline + unknown are refused too.
        assert!(precheck("bless", &["desktop-ntp".into()]).is_err());
        assert!(precheck("bless", &["evil".into()]).is_err());
        // no argument → refused before any ceremony.
        assert!(precheck("bless", &[]).is_err());
    }

    #[test]
    fn precheck_raw_validates_through_the_one_grammar() {
        let p = precheck("add-raw", &["example.com:tcp:8443".into()]).unwrap();
        assert!(p.high_authority());
        assert!(p.action().contains("example.com:tcp:8443"));
        // the diff rows carry the parsed fields (rendered + sanitized by consent).
        assert!(p.diff_rows.iter().any(|(k, v)| k == "Host" && v == "example.com"));
        assert!(p.diff_rows.iter().any(|(k, v)| k == "Port" && v == "8443"));
        // hostile / malformed triples are refused before the human is asked.
        for bad in ["-evil.com:tcp:443", "singlelabel:tcp:443", "e.com:icmp:0", "e.com:tcp:70000"] {
            assert!(precheck("add-raw", &[bad.into()]).is_err(), "must refuse {bad}");
        }
    }

    #[test]
    fn commit_maps_ops_to_the_confirmed_verbs() {
        // The op→(verb,arg) mapping is what commit execs — argv from the VALIDATED plan, never the wire.
        assert!(matches!(&Op::BlessProfile("web-browsing".into()), Op::BlessProfile(p) if p == "web-browsing"));
        // (the exec itself is proven in the sealed-VM gate; here we pin the op variants + action text.)
        assert_eq!(action(&Op::AddRaw("a.com:tcp:443".into())), "ALLOW a raw desktop destination: a.com:tcp:443");
        assert_eq!(action(&Op::UnblessProfile("web-browsing".into())), "REVOKE desktop egress: unbless 'web-browsing'");
        // ADR-009 S3 manifest ops.
        assert_eq!(
            action(&Op::ManifestInstall { name: "radar".into(), text: String::new() }),
            "INSTALL the owner network capability 'radar'"
        );
        assert_eq!(action(&Op::ManifestRemove("radar".into())), "REMOVE the owner network capability 'radar'");
    }

    /// A well-formed OWNER manifest (`deliver none`, tier one-click ⇒ every host tcp:443) reaching `host`.
    fn owner_manifest(name: &str, title: &str, host: &str) -> String {
        format!(
            "schema shrek-egress-capability/1\nname {name}\ntitle {title}\npurpose Test capability\n\
             feature dms:{name}\ntier one-click\ndeliver none\nhost {host} tcp 443\n"
        )
    }

    #[test]
    fn manifest_install_builds_the_card_and_the_toggle_warning() {
        let text = owner_manifest("radar", "Rain Radar", "radar.example.test");
        let p = precheck("manifest-install", &["radar".into(), text.clone()]).unwrap();
        // High-authority (typed code, never a bare y) — installing mints durable vocabulary.
        assert!(p.high_authority());
        assert!(p.action().contains("radar"));
        // The root-authored card the human reads: title/purpose/feature + a Reaches row per host.
        assert!(p.diff_rows.iter().any(|(k, v)| k == "Title" && v == "Rain Radar"));
        assert!(p.diff_rows.iter().any(|(k, v)| k == "Feature" && v == "dms:radar"));
        assert!(p.diff_rows.iter().any(|(k, v)| k == "Reaches" && v == "radar.example.test tcp/443"));
        // The always-present "toggle ≠ live intent" line (OQ-1). Not a storage host ⇒ only that one warning.
        assert_eq!(p.warnings().len(), 1);
        assert!(p.warnings()[0].contains("ONE click"));
        assert!(p.warnings()[0].to_lowercase().contains("toggle") || p.warnings()[0].contains("not proof"));
    }

    #[test]
    fn manifest_install_adds_the_storage_host_warning() {
        // A capability reaching a world-writable object store trips the §8 exfil nudge ON TOP of the toggle
        // line (s3 is not system-reserved, so it is not refused — the warning is advisory, not a gate).
        let text = owner_manifest("mybucket", "My Bucket", "mybucket.s3.amazonaws.com");
        let p = precheck("manifest-install", &["mybucket".into(), text]).unwrap();
        assert_eq!(p.warnings().len(), 2);
        assert!(p.warnings().iter().any(|w| w.contains("upload/download") && w.contains("s3.amazonaws.com")));
    }

    #[test]
    fn manifest_install_fail_closed_refusals_never_reach_the_human() {
        // deliver hosts is a sealed-only affordance.
        let deliver_hosts = "schema shrek-egress-capability/1\nname radar\ntitle R\npurpose P\nfeature dms:radar\ntier one-click\ndeliver hosts\nhost radar.example.test tcp 443\n";
        assert!(precheck("manifest-install", &["radar".into(), deliver_hosts.into()]).err().unwrap().1.contains("deliver hosts"));
        // A host reserved by sealed/system machinery (open-meteo is a sealed weather host) is REFUSED.
        let reserved = owner_manifest("radar", "R", "api.open-meteo.com");
        assert!(precheck("manifest-install", &["radar".into(), reserved]).err().unwrap().1.contains("reserved"));
        // Staged name must match the manifest's own name.
        let mismatch = owner_manifest("radar", "R", "radar.example.test");
        assert!(precheck("manifest-install", &["notradar".into(), mismatch]).err().unwrap().1.contains("!="));
        // A malformed manifest denies before any ceremony (no partial parse).
        assert!(precheck("manifest-install", &["radar".into(), "garbage\nlines\n".into()]).is_err());
        // Missing the text entirely, or a bad name token, denies.
        assert!(precheck("manifest-install", &["radar".into()]).is_err());
        assert!(precheck("manifest-install", &["-bad".into(), owner_manifest("x", "X", "x.example.test")]).is_err());
    }

    #[test]
    fn manifest_remove_is_name_only_and_reduces_authority() {
        let p = precheck("manifest-remove", &["radar".into()]).unwrap();
        assert!(p.high_authority()); // still the typed code (a desktop-egress ceremony op)
        assert!(p.action().contains("REMOVE"));
        assert!(p.warnings().is_empty()); // removal carries no exfil/toggle nudge
        assert!(p.diff_rows.iter().any(|(k, v)| k == "Capability" && v == "radar"));
        // A malformed name is refused before the ceremony.
        assert!(precheck("manifest-remove", &["-bad".into()]).is_err());
    }
}
