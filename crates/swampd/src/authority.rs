//! authority — swampd resolves a caller's session grants from the root-owned authority record that
//! gatekeeperd wrote (gatekeeperd/authority_record.rs). This is the "resolve INDEPENDENTLY, from
//! privileged state, NEVER from the request" half of swamp.md §9: the query carries only a session
//! HANDLE; the grants come from HERE. Fail-closed by construction — an absent, malformed, or
//! wrong-version record yields NO grants, so the caller's projection is empty (it discovers nothing),
//! never an error that would reveal whether the session exists.

use std::fs;
use std::path::{Path, PathBuf};

/// Same safe-token rule gatekeeperd enforces on write — a session id is a single path component, so a
/// crafted handle can never traverse out of the authority dir when used as the record filename.
pub fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && id.len() <= 128
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Load the canonical grant roots for `session_id` from `dir`. Returns an empty vec (NOT an error) for
/// any fail-closed condition — invalid id, missing record, bad header — so an unauthorized query is
/// indistinguishable from one that simply matched nothing.
pub fn load_grants(dir: &Path, session_id: &str) -> Vec<PathBuf> {
    if !valid_session_id(session_id) {
        return Vec::new();
    }
    let Ok(text) = fs::read_to_string(dir.join(session_id)) else {
        return Vec::new();
    };
    let mut lines = text.lines();
    if lines.next() != Some("SHREK-AUTHORITY 1") {
        return Vec::new();
    }
    let mut grants = Vec::new();
    for l in lines {
        if let Some(rest) = l.strip_prefix("grant ") {
            if !rest.is_empty() {
                grants.push(PathBuf::from(rest));
            }
        }
    }
    grants
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(dir: &Path, name: &str, body: &str) {
        fs::create_dir_all(dir).unwrap();
        let mut f = fs::File::create(dir.join(name)).unwrap();
        f.write_all(body.as_bytes()).unwrap();
    }

    #[test]
    fn loads_grants_from_wellformed_record() {
        let dir = std::env::temp_dir().join(format!("swamp-auth-r-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        write(&dir, "s1", "SHREK-AUTHORITY 1\nsession s1\ngrant /home/u/Projects/app-a\ngrant /home/u/Projects/app-b\nEND\n");
        let g = load_grants(&dir, "s1");
        assert_eq!(g, vec![PathBuf::from("/home/u/Projects/app-a"), PathBuf::from("/home/u/Projects/app-b")]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_record_is_empty_not_error() {
        let dir = std::env::temp_dir().join(format!("swamp-auth-m-{}", std::process::id()));
        assert!(load_grants(&dir, "nope").is_empty());
    }

    #[test]
    fn bad_header_yields_no_grants() {
        let dir = std::env::temp_dir().join(format!("swamp-auth-b-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        write(&dir, "s2", "GARBAGE\ngrant /home/u/Projects\nEND\n");
        assert!(load_grants(&dir, "s2").is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn traversal_handle_is_rejected() {
        let dir = std::env::temp_dir().join("swamp-auth-t");
        assert!(load_grants(&dir, "../../etc/passwd").is_empty());
        assert!(load_grants(&dir, "a/b").is_empty());
    }
}
