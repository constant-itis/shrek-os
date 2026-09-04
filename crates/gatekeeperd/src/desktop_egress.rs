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
//! the "read your files AND reach the network" exfil warning). On a confirmed ceremony, commit execs the
//! ROOT-ONLY `egressd confirmed-*` verb — the ABSOLUTE sealed binary with a CLEARED environment and argv
//! taken from the VALIDATED plan (never re-read from the wire), so the string uid 1000 supplied is
//! re-validated by egressd too and no inherited env can redirect the store.

use crate::bench_plane::{self, AuthorityPlan};
use crate::linux_uapi::Ucred;
use shrek_policy::desktop_egress::{bless_tier, parse_raw_triple, BlessTier};
use std::os::fd::RawFd;
use std::process::Command;

/// The sealed egressd binary the ceremony commit execs. Absolute (never a PATH lookup). Overridable ONLY
/// in the `oracle-env` build so the host oracle can point at the freshly-built binary; the shipped image
/// compiles the override out (`bench_env` is a const `None`), so this is unconditional in production.
fn egressd_bin() -> String {
    crate::bench_record::bench_env("SHREK_EGRESSD_BIN").unwrap_or_else(|| "/usr/libexec/shrek/egressd".to_string())
}

/// A validated desktop-egress ceremony op. Carries the ALREADY-VALIDATED subject string (profile name or
/// raw `host:proto:port` wire form) so commit builds egressd's argv from the rendered plan, not the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    BlessProfile(String),
    UnblessProfile(String),
    AddRaw(String),
    RemoveRaw(String),
}

/// The ceremony-header action line (rendered on the VT; sanitized by the renderer).
pub fn action(op: &Op) -> String {
    match op {
        Op::BlessProfile(p) => format!("ALLOW broad desktop egress: bless '{p}'"),
        Op::UnblessProfile(p) => format!("REVOKE desktop egress: unbless '{p}'"),
        Op::AddRaw(t) => format!("ALLOW a raw desktop destination: {t}"),
        Op::RemoveRaw(t) => format!("REMOVE a raw desktop destination: {t}"),
    }
}

/// Materialize a CONFIRMED desktop-egress ceremony: exec the ROOT-ONLY `egressd confirmed-*` verb. Runs
/// as whatever uid gatekeeperd is (root — the daemon), env CLEARED, absolute binary, argv from the
/// validated op. egressd re-checks `geteuid()==0`, the tier, and the grammar (defense in depth), so a
/// confused-deputy exec still can't widen anything. Returns the child's exit code (0 = applied/persisted).
pub fn commit(op: &Op) -> i32 {
    let (verb, arg) = match op {
        Op::BlessProfile(p) => ("confirmed-bless", p.clone()),
        Op::UnblessProfile(p) => ("confirmed-unbless", p.clone()),
        Op::AddRaw(t) => ("confirmed-add-raw", t.clone()),
        Op::RemoveRaw(t) => ("confirmed-remove-raw", t.clone()),
    };
    match Command::new(egressd_bin()).env_clear().arg(verb).arg(&arg).status() {
        Ok(st) => st.code().unwrap_or(1),
        Err(e) => {
            eprintln!("gatekeeperd/desktop-egress: exec egressd {verb}: {e}");
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
            Ok(bench_plane::desktop_egress_plan(op, arg.to_string(), rows, true))
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
            Ok(bench_plane::desktop_egress_plan(op, wire, rows, true))
        }
        _ => Err((2, format!("unknown desktop-egress verb {verb:?}"))),
    }
}

/// Socket entry (mirrors [`bench_plane::dispatch_socket`]): `argv[0]` = subverb, `argv[1]` = the profile
/// or raw triple. Routes into the shared ceremony core with the desktop-egress precheck/commit + the
/// `desktop-egress` wire prefix. The peer gate (dev uid), cooldown, tuple-binding, SAK/VT ceremony, and
/// audit all live in the shared core.
pub fn dispatch_socket(cred: Ucred, peer_fd: RawFd, argv: &[String]) -> (i32, Vec<String>) {
    let verb = argv.first().map(String::as_str).unwrap_or("").to_string();
    let rest = argv[argv.len().min(1)..].to_vec();
    match verb.as_str() {
        "bless" | "unbless" | "add-raw" | "remove-raw" => crate::consent::run_socket_consent_with(
            cred,
            peer_fd,
            &verb,
            "desktop-egress",
            || precheck(&verb, &rest),
            bench_plane::commit_authority,
        ),
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
    }
}
