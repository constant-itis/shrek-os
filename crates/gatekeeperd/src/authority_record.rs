//! authority_record — gatekeeperd records a constructed session's canonical filesystem grants into a
//! root-owned, ephemeral authority record that `swampd` resolves INDEPENDENTLY (swamp.md §9).
//!
//! This is the load-bearing half of the Swamp query gate's authority model: a `shrek find` query
//! carries only a session HANDLE, never grants. The AUTHORITY lives here, written by the privileged
//! broker (the trust anchor) into `/run/shrek/authority/<session>`, owned `root:swamp` mode 0640
//! inside a `root:swamp` 0750 dir — so the untrusted workload (a different uid) can neither forge nor
//! widen it, and `swampd` (the `swamp` user) can read it. swampd trusts the RECORD, never the caller:
//! `SO_PEERCRED` authenticates who is asking, this record says what that session may reach.
//!
//! Grants are stored CANONICAL (realpath): the crawler canonicalizes object paths too, so the query
//! gate's prefix match aligns. The record is a dep-free line-text file, mirroring the broker's wire
//! idiom. Minimal + ephemeral by design (`--rm` on teardown) — NOT a general grant protocol.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{chown, PermissionsExt};
use std::path::{Path, PathBuf};

/// Default location of the ephemeral session-authority records. Overridable for the host/container
/// repro (no systemd) via `SHREK_AUTHORITY_DIR`, mirroring the broker's other env overrides.
pub fn authority_dir() -> PathBuf {
    std::env::var("SHREK_AUTHORITY_DIR")
        .unwrap_or_else(|_| "/run/shrek/authority".to_string())
        .into()
}

/// A session id must be a safe, single-path-component token so it can never traverse out of the
/// authority dir when used as the record filename. Alnum plus `._-`, non-empty, not `.`/`..`.
pub fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && id.len() <= 128
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Resolve `swamp` uid/gid from /etc/passwd,/etc/group so the record is owned `root:swamp`. Returns
/// `None` if the user/group is absent (the caller then falls back to a root-only-readable record).
pub(crate) fn swamp_ids() -> Option<(u32, u32)> {
    let passwd = fs::read_to_string("/etc/passwd").ok()?;
    let uid = passwd.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() >= 4 && f[0] == "swamp").then(|| f[2].parse::<u32>().ok()).flatten()
    })?;
    let group = fs::read_to_string("/etc/group").ok()?;
    let gid = group
        .lines()
        .find_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            (f.len() >= 3 && f[0] == "swamp").then(|| f[2].parse::<u32>().ok()).flatten()
        })
        .unwrap_or(uid);
    Some((uid, gid))
}

/// Write the authority record for `session_id` with `grants` (each canonicalized). Overwrites any
/// prior record for the same session (a session's grants are set once at construction). Best-effort
/// `root:swamp` ownership; the mode is always 0640 so a non-owner, non-group process cannot read it.
pub fn write_record(dir: &Path, session_id: &str, grants: &[PathBuf]) -> io::Result<PathBuf> {
    if !valid_session_id(session_id) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid session id"));
    }
    fs::create_dir_all(dir)?;
    // Lock the dir down: only root + the swamp group may traverse it (records are not world-listable).
    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o750));
    if let Some((uid, gid)) = swamp_ids() {
        let _ = chown(dir, Some(uid), Some(gid));
    }

    // Canonicalize every grant — a grant to a nonexistent tree is a construction error, not an empty
    // scope, so fail rather than silently record an unresolvable prefix.
    let mut canon = Vec::with_capacity(grants.len());
    for g in grants {
        let c = fs::canonicalize(g)
            .map_err(|e| io::Error::new(e.kind(), format!("grant {:?} unresolvable: {e}", g)))?;
        canon.push(c);
    }

    let mut body = String::from("SHREK-AUTHORITY 1\n");
    body.push_str(&format!("session {session_id}\n"));
    for c in &canon {
        // A canonical path with a newline is impossible on Linux (newline is a legal byte in a path,
        // but canonicalize of a real inode can contain one). Guard: reject — the line format demands it.
        let s = c.to_string_lossy();
        if s.contains('\n') {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "grant path contains newline"));
        }
        body.push_str(&format!("grant {s}\n"));
    }
    body.push_str("END\n");

    let path = dir.join(session_id);
    // Write via a temp + rename so a reader never sees a half-written record.
    let tmp = dir.join(format!(".{session_id}.tmp"));
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

/// Remove a session's authority record (teardown). Idempotent — a missing record is not an error.
pub fn remove_record(dir: &Path, session_id: &str) -> io::Result<()> {
    if !valid_session_id(session_id) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid session id"));
    }
    match fs::remove_file(dir.join(session_id)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// CLI: `gatekeeperd authority-record --session <id> [--grant <path>]... [--dir <dir>] [--rm]`.
/// Writes (or with `--rm`, removes) the record. Privileged: run as the broker (root). Returns a
/// process exit code.
pub fn cli(args: &[String]) -> i32 {
    let mut session = String::new();
    let mut grants: Vec<PathBuf> = Vec::new();
    let mut dir = authority_dir();
    let mut rm = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--session" => session = it.next().cloned().unwrap_or_default(),
            "--grant" => {
                if let Some(g) = it.next() {
                    grants.push(PathBuf::from(g));
                }
            }
            "--dir" => {
                if let Some(d) = it.next() {
                    dir = PathBuf::from(d);
                }
            }
            "--rm" => rm = true,
            other => {
                eprintln!("authority-record: unknown arg {other}");
                return 2;
            }
        }
    }
    if session.is_empty() {
        eprintln!("authority-record: --session <id> required");
        return 2;
    }
    if rm {
        return match remove_record(&dir, &session) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("authority-record: rm failed: {e}");
                1
            }
        };
    }
    if grants.is_empty() {
        eprintln!("authority-record: at least one --grant required");
        return 2;
    }
    match write_record(&dir, &session, &grants) {
        Ok(p) => {
            println!("authority-record: wrote {} ({} grant(s))", p.display(), grants.len());
            0
        }
        Err(e) => {
            eprintln!("authority-record: write failed: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_validation_blocks_traversal() {
        assert!(valid_session_id("sess-abc_123"));
        assert!(!valid_session_id(""));
        assert!(!valid_session_id("."));
        assert!(!valid_session_id(".."));
        assert!(!valid_session_id("a/b"));
        assert!(!valid_session_id("../etc/passwd"));
        assert!(!valid_session_id("a b"));
    }

    #[test]
    fn write_then_read_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("swamp-auth-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let proj = tmp.join("proj");
        fs::create_dir_all(&proj).unwrap();
        let rec = write_record(&tmp, "sess1", &[proj.clone()]).unwrap();
        let body = fs::read_to_string(&rec).unwrap();
        assert!(body.starts_with("SHREK-AUTHORITY 1\n"));
        assert!(body.contains("session sess1\n"));
        assert!(body.contains(&format!("grant {}\n", fs::canonicalize(&proj).unwrap().display())));
        // mode is 0640
        let mode = fs::metadata(&rec).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
        // rm is idempotent
        remove_record(&tmp, "sess1").unwrap();
        remove_record(&tmp, "sess1").unwrap();
        assert!(!rec.exists());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn unresolvable_grant_fails() {
        let tmp = std::env::temp_dir().join(format!("swamp-auth-test2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let r = write_record(&tmp, "sess2", &[tmp.join("does-not-exist")]);
        assert!(r.is_err());
        let _ = fs::remove_dir_all(&tmp);
    }
}
