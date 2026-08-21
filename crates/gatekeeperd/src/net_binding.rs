//! net_binding — the transport-identity binding for broker-routed in-sandbox `shrek find`
//! (Phase-6 Swamp slice-2, docs/phase6-swamp-slice2-broker-routed-find.md, amendments 1-4).
//!
//! When a T2 coding session queries the swamp, its query is ROUTED through `swamp-broker`, so
//! `swampd`'s `SO_PEERCRED` can no longer identify the workload across the hop. Caller authenticity
//! instead rests on the sandbox's un-forgeable `/30` source IP — made un-spoofable AT THE HOST by the
//! net_plane per-veth anti-spoof (amendment 2). This module records, at construction, the truth the
//! broker consults: `cont_ip → session`. The broker forwards a caller's opaque handle to `swampd`
//! ONLY IF `getpeername()→cont_ip` maps here to that same session (amendment 4: the broker is TCB for
//! session selection — the handle is identity, not bearer authority).
//!
//! Same trust shape as [`crate::authority_record`]: root-owned `root:swamp` 0640 in a `root:swamp`
//! 0750 dir, so the untrusted workload can neither forge nor read it, and `swamp-broker` (the swamp
//! group) can. Dep-free line-text, atomic temp+rename. Lifecycle (amendment 3): written BEFORE the
//! sandbox can emit traffic, REVOKED on every teardown path, and a create for a reused `cont_ip`
//! atomically REPLACES any stale binding so a dead session can never authorize a new sandbox that
//! lands on the same `/30`.

use crate::authority_record::{swamp_ids, valid_session_id};
use std::fs;
use std::io::{self, Write};
use std::net::Ipv4Addr;
use std::os::unix::fs::{chown, PermissionsExt};
use std::path::{Path, PathBuf};

/// Default location of the ephemeral `cont_ip → session` bindings. Overridable for the host/container
/// oracle (no systemd) via `SHREK_NET_BINDING_DIR`, mirroring the authority record's env override.
pub fn binding_dir() -> PathBuf {
    std::env::var("SHREK_NET_BINDING_DIR")
        .unwrap_or_else(|_| "/run/shrek/net-binding".to_string())
        .into()
}

/// The record filename for a `cont_ip` is its canonical dotted-quad. `Ipv4Addr::to_string` yields only
/// `[0-9.]`, a safe single path component (no traversal), so no extra validation is needed on the IP.
fn ip_file(cont_ip: Ipv4Addr) -> String {
    cont_ip.to_string()
}

/// Bind `cont_ip → session_id` (construction). Atomically REPLACES any prior binding for the same IP
/// (amendment 3: a reused `/30` slot must never resolve to a dead predecessor's session). Best-effort
/// `root:swamp` ownership; mode 0640 so a non-owner, non-group process cannot read it.
pub fn write_binding(dir: &Path, cont_ip: Ipv4Addr, session_id: &str) -> io::Result<PathBuf> {
    if !valid_session_id(session_id) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid session id"));
    }
    fs::create_dir_all(dir)?;
    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o750));
    if let Some((uid, gid)) = swamp_ids() {
        let _ = chown(dir, Some(uid), Some(gid));
    }

    let body = format!("SHREK-NET-BINDING 1\nip {cont_ip}\nsession {session_id}\nEND\n");
    let path = dir.join(ip_file(cont_ip));
    let tmp = dir.join(format!(".{}.tmp", ip_file(cont_ip)));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(body.as_bytes())?;
        f.sync_all()?;
    }
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o640))?;
    if let Some((uid, gid)) = swamp_ids() {
        let _ = chown(&tmp, Some(uid), Some(gid));
    }
    fs::rename(&tmp, &path)?; // atomic replace of any stale binding for this IP
    Ok(path)
}

/// Remove the binding for `cont_ip` (teardown). Idempotent — a missing binding is not an error. Called
/// on EVERY teardown path so a torn-down sandbox's IP resolves to nothing until it is re-bound.
pub fn remove_binding(dir: &Path, cont_ip: Ipv4Addr) -> io::Result<()> {
    match fs::remove_file(dir.join(ip_file(cont_ip))) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Resolve `cont_ip → session` (the broker's lookup). Fail-closed: a missing or malformed binding
/// returns `None` (the broker then refuses to forward — indistinguishable from no-match). Verifies the
/// record's own `ip` line matches the queried IP, so a file can never vouch for a different address.
pub fn load_binding(dir: &Path, cont_ip: Ipv4Addr) -> Option<String> {
    let body = fs::read_to_string(dir.join(ip_file(cont_ip))).ok()?;
    let mut lines = body.lines();
    if lines.next()? != "SHREK-NET-BINDING 1" {
        return None;
    }
    let ip_line = lines.next()?.strip_prefix("ip ")?;
    if ip_line != cont_ip.to_string() {
        return None; // the record must describe the very IP we looked it up by
    }
    let session = lines.next()?.strip_prefix("session ")?.to_string();
    if lines.next()? != "END" || !valid_session_id(&session) {
        return None;
    }
    Some(session)
}

/// CLI: `gatekeeperd net-binding --ip <cont_ip> --session <id> [--dir <dir>] [--rm]`. Writes (or with
/// `--rm`, removes) the `cont_ip→session` binding through the SAME writer construction uses, so the
/// record format is single-sourced. Privileged (run as the broker, root). Mirrors `authority-record`;
/// used by the host-side swamp-broker oracle to stand up + revoke bindings (production writes them
/// inline in the T2 construct, never via this CLI). Returns a process exit code.
pub fn cli(args: &[String]) -> i32 {
    let mut ip: Option<Ipv4Addr> = None;
    let mut session = String::new();
    let mut dir = binding_dir();
    let mut rm = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--ip" => ip = it.next().and_then(|s| s.parse().ok()),
            "--session" => session = it.next().cloned().unwrap_or_default(),
            "--dir" => {
                if let Some(d) = it.next() {
                    dir = PathBuf::from(d);
                }
            }
            "--rm" => rm = true,
            other => {
                eprintln!("net-binding: unknown arg {other}");
                return 2;
            }
        }
    }
    let Some(ip) = ip else {
        eprintln!("net-binding: --ip <ipv4> required");
        return 2;
    };
    if rm {
        return match remove_binding(&dir, ip) {
            Ok(()) => {
                println!("net-binding: removed binding for {ip}");
                0
            }
            Err(e) => {
                eprintln!("net-binding: rm failed: {e}");
                1
            }
        };
    }
    if session.is_empty() {
        eprintln!("net-binding: --session <id> required");
        return 2;
    }
    match write_binding(&dir, ip, &session) {
        Ok(p) => {
            println!("net-binding: wrote {} (ip {ip} -> session {session})", p.display());
            0
        }
        Err(e) => {
            eprintln!("net-binding: write failed: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> PathBuf {
        let base = std::env::var("CARGO_TARGET_TMPDIR").unwrap_or_else(|_| "/tmp".into());
        let d = PathBuf::from(base).join(format!("nb-{}", std::process::id()));
        let _ = fs::create_dir_all(&d);
        d
    }

    #[test]
    fn write_then_load_roundtrips_the_session() {
        let d = tmpdir();
        let ip = Ipv4Addr::new(10, 66, 1, 2);
        write_binding(&d, ip, "sessA").unwrap();
        assert_eq!(load_binding(&d, ip).as_deref(), Some("sessA"));
    }

    #[test]
    fn reused_ip_replaces_stale_session_no_leak() {
        // amendment 3: A binds X→SA, teardown removes it, B reuses X→SB. X must resolve SB, never SA;
        // and post-teardown / pre-reuse X resolves to nothing.
        let d = tmpdir();
        let ip = Ipv4Addr::new(10, 66, 9, 2);
        write_binding(&d, ip, "SA").unwrap();
        remove_binding(&d, ip).unwrap();
        assert_eq!(load_binding(&d, ip), None, "torn-down IP must fail closed");
        write_binding(&d, ip, "SB").unwrap();
        assert_eq!(load_binding(&d, ip).as_deref(), Some("SB"));
        // Even without an intervening remove, a fresh write replaces the prior session.
        write_binding(&d, ip, "SC").unwrap();
        assert_eq!(load_binding(&d, ip).as_deref(), Some("SC"));
    }

    #[test]
    fn missing_binding_fails_closed() {
        let d = tmpdir();
        assert_eq!(load_binding(&d, Ipv4Addr::new(10, 66, 7, 2)), None);
        // Idempotent remove of a nonexistent binding is Ok.
        remove_binding(&d, Ipv4Addr::new(10, 66, 7, 2)).unwrap();
    }

    #[test]
    fn record_must_describe_the_queried_ip() {
        // A binding file whose inner `ip` disagrees with its filename cannot vouch for the lookup IP.
        let d = tmpdir();
        let ip = Ipv4Addr::new(10, 66, 3, 2);
        let path = d.join(ip.to_string());
        fs::write(&path, "SHREK-NET-BINDING 1\nip 10.66.99.2\nsession EVIL\nEND\n").unwrap();
        assert_eq!(load_binding(&d, ip), None);
    }

    #[test]
    fn malformed_records_fail_closed() {
        let d = tmpdir();
        let ip = Ipv4Addr::new(10, 66, 4, 2);
        let path = d.join(ip.to_string());
        for bad in [
            "garbage",
            "SHREK-NET-BINDING 2\nip 10.66.4.2\nsession S\nEND\n",
            "SHREK-NET-BINDING 1\nip 10.66.4.2\nsession S\n",           // no END
            "SHREK-NET-BINDING 1\nip 10.66.4.2\nsession \nEND\n",        // empty session
        ] {
            fs::write(&path, bad).unwrap();
            assert_eq!(load_binding(&d, ip), None, "should reject: {bad:?}");
        }
    }

    #[test]
    fn invalid_session_id_is_rejected_on_write() {
        let d = tmpdir();
        let ip = Ipv4Addr::new(10, 66, 5, 2);
        assert!(write_binding(&d, ip, "../escape").is_err());
        assert!(write_binding(&d, ip, "").is_err());
    }
}
