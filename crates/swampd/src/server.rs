//! server — the caller-scoped query API (swamp.md §9). A root-owned unix socket; every request is
//! authenticated by `SO_PEERCRED` (the peer's unspoofable uid, the same gate gatekeeperd uses) and
//! AUTHORIZED by the session's independently-resolved grant record ([`crate::authority`]). The
//! request carries a session HANDLE and query, never authority. The result is the caller's PROJECTION:
//! objects outside `session-grants ∩ domain-ceiling` are absent, built out by [`crate::index::Index::query`]
//! before retrieval — never a global search filtered after.
//!
//! Dep-free line-text wire protocol, mirroring the broker:
//! ```text
//! request:                          response (success):        response (fail-closed):
//!   QUERY 1                           RESULT <n>                  RESULT 0
//!   session <id>                      hit <path>\t<snippet>       END
//!   intent search|discover            ...
//!   scope <abs-path|->                END
//!   limit <n>
//!   q <terms...>
//!   END
//! ```
//! An unauthorized/unknown session is NOT an error — it returns `RESULT 0`, indistinguishable from a
//! query that matched nothing, so a caller cannot probe which sessions exist.

use crate::authority;
use crate::index::{Index, Intent};
use crate::linux_uapi;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

const MAX_REQUEST_LINES: usize = 64;
const MAX_LIMIT: usize = 500;
const DEFAULT_LIMIT: usize = 50;

pub struct Server<'a> {
    index: &'a Index,
    authority_dir: PathBuf,
    allowed_uids: Vec<u32>,
}

impl<'a> Server<'a> {
    pub fn new(index: &'a Index, authority_dir: PathBuf, allowed_uids: Vec<u32>) -> Self {
        Server { index, authority_dir, allowed_uids }
    }

    /// Bind the query socket and serve forever (single-threaded, synchronous — queries are fast and
    /// swampd is availability-plane). The socket is 0666 + SO_PEERCRED-gated, matching the broker.
    pub fn serve(&self, sock_path: &Path) -> std::io::Result<()> {
        let _ = std::fs::remove_file(sock_path);
        let listener = UnixListener::bind(sock_path)?;
        let _ = std::fs::set_permissions(sock_path, std::fs::Permissions::from_mode(0o666));
        eprintln!(
            "swampd: query socket {} (allowed uids {:?})",
            sock_path.display(),
            self.allowed_uids
        );
        for conn in listener.incoming() {
            match conn {
                Ok(stream) => {
                    if let Err(e) = self.handle(stream) {
                        eprintln!("swampd: query conn error: {e}");
                    }
                }
                Err(e) => eprintln!("swampd: accept error: {e}"),
            }
        }
        Ok(())
    }

    fn handle(&self, mut stream: UnixStream) -> std::io::Result<()> {
        // Authenticate the peer. Identity ONLY — authority comes from the session record, not this uid.
        let cred = linux_uapi::peer_cred(stream.as_raw_fd())?;
        if !self.allowed_uids.contains(&cred.uid) {
            let _ = writeln!(stream, "ERROR unauthorized-peer");
            return Ok(());
        }

        let req = match Request::read(&stream) {
            Some(r) => r,
            None => {
                let _ = writeln!(stream, "ERROR malformed-request");
                return Ok(());
            }
        };

        // Resolve authority INDEPENDENTLY from the root-owned record — never from the request.
        let grants = authority::load_grants(&self.authority_dir, &req.session);
        let effective = narrow_scope(&grants, req.scope.as_deref());

        // Domain ceiling: the domains whose sealed ceiling grants this verb.
        let verb_domains = domains_granting(req.intent);

        let hits = self
            .index
            .query(req.intent, &req.query, &effective, &verb_domains, req.limit)
            .unwrap_or_default();

        writeln!(stream, "RESULT {}", hits.len())?;
        for h in &hits {
            writeln!(stream, "hit {}\t{}", sanitize(&h.path), sanitize(&h.snippet))?;
        }
        writeln!(stream, "END")?;
        Ok(())
    }
}

struct Request {
    session: String,
    intent: Intent,
    scope: Option<String>,
    limit: usize,
    query: String,
}

impl Request {
    fn read(stream: &UnixStream) -> Option<Request> {
        let reader = BufReader::new(stream);
        let mut session = String::new();
        let mut intent = None;
        let mut scope = None;
        let mut limit = DEFAULT_LIMIT;
        let mut query = String::new();
        let mut saw_header = false;
        let mut saw_end = false;
        for (i, line) in reader.lines().enumerate() {
            if i >= MAX_REQUEST_LINES {
                return None;
            }
            let line = line.ok()?;
            if i == 0 {
                if line != "QUERY 1" {
                    return None;
                }
                saw_header = true;
                continue;
            }
            if line == "END" {
                saw_end = true;
                break;
            }
            let (k, v) = line.split_once(' ').unwrap_or((line.as_str(), ""));
            match k {
                "session" => session = v.to_string(),
                "intent" => {
                    intent = Some(match v {
                        "search" => Intent::Search,
                        "discover" => Intent::Discover,
                        _ => return None,
                    })
                }
                "scope" => {
                    if !v.is_empty() && v != "-" {
                        scope = Some(v.to_string());
                    }
                }
                "limit" => limit = v.parse::<usize>().ok()?.clamp(1, MAX_LIMIT),
                "q" => query = v.to_string(),
                _ => {} // ignore unknown fields (forward-compat)
            }
        }
        if !saw_header || !saw_end {
            return None;
        }
        Some(Request { session, intent: intent?, scope, limit, query })
    }
}

/// The domain names whose sealed ceiling grants `intent`'s verb. Computed from the sealed table so it
/// tracks policy; at slice-1 every domain is RO_SEARCH, so this is every domain — but the code path
/// that would exclude a discover:false or non-search domain exists and is exercised.
fn domains_granting(intent: Intent) -> Vec<&'static str> {
    shrek_policy::swamp::INDEXABLE_DOMAINS
        .iter()
        .filter(|d| match intent {
            Intent::Discover => d.ceiling.discover,
            Intent::Search => d.ceiling.search && d.ceiling.read,
        })
        .map(|d| d.name)
        .collect()
}

/// Apply an optional `scope` selector that may only NARROW the session grants (swamp.md §9). If the
/// scope lies beneath (or equals) some grant, the effective scope becomes exactly that subtree; if it
/// lies under no grant, it can grant nothing — the effective set is EMPTY (a scope can never widen
/// authority). No scope ⇒ the full grant set.
fn narrow_scope(grants: &[PathBuf], scope: Option<&str>) -> Vec<PathBuf> {
    match scope {
        None => grants.to_vec(),
        Some(s) => {
            let sp = PathBuf::from(s);
            if grants.iter().any(|g| path_beneath_or_equal(&sp, g)) {
                vec![sp]
            } else {
                Vec::new()
            }
        }
    }
}

/// Is `inner` equal to, or component-wise beneath, `outer`? Both compared by path components, so a
/// mere shared byte-prefix (`/a/appfoo` vs grant `/a/app`) does NOT count as beneath.
fn path_beneath_or_equal(inner: &Path, outer: &Path) -> bool {
    let mut o = outer.components();
    let mut i = inner.components();
    loop {
        match (o.next(), i.next()) {
            (Some(oc), Some(ic)) if oc == ic => continue,
            (Some(_), _) => return false, // outer has a component inner lacks/differs ⇒ not beneath
            (None, _) => return true,     // exhausted outer ⇒ inner is at or below it
        }
    }
}

/// Collapse control characters (newline/tab/NUL) to spaces so a value can never break the line
/// protocol or inject extra response lines.
fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_narrows_within_a_grant() {
        let grants = vec![PathBuf::from("/home/u/Projects")];
        let eff = narrow_scope(&grants, Some("/home/u/Projects/app-a"));
        assert_eq!(eff, vec![PathBuf::from("/home/u/Projects/app-a")]);
    }

    #[test]
    fn scope_outside_grants_yields_empty_never_widens() {
        let grants = vec![PathBuf::from("/home/u/Projects/app-a")];
        // Trying to "narrow" to a sibling / parent / unrelated tree grants nothing.
        assert!(narrow_scope(&grants, Some("/home/u/Projects/app-b")).is_empty());
        assert!(narrow_scope(&grants, Some("/home/u/Projects")).is_empty());
        assert!(narrow_scope(&grants, Some("/home/u/Vault")).is_empty());
        assert!(narrow_scope(&grants, Some("/etc")).is_empty());
    }

    #[test]
    fn no_scope_keeps_full_grant_set() {
        let grants = vec![PathBuf::from("/a"), PathBuf::from("/b")];
        assert_eq!(narrow_scope(&grants, None), grants);
    }

    #[test]
    fn beneath_is_component_wise_not_substring() {
        assert!(path_beneath_or_equal(Path::new("/a/app"), Path::new("/a/app")));
        assert!(path_beneath_or_equal(Path::new("/a/app/x"), Path::new("/a/app")));
        assert!(!path_beneath_or_equal(Path::new("/a/app-b"), Path::new("/a/app")));
        assert!(!path_beneath_or_equal(Path::new("/a"), Path::new("/a/app")));
    }

    #[test]
    fn empty_grants_stay_empty_under_any_scope() {
        assert!(narrow_scope(&[], Some("/anything")).is_empty());
        assert!(narrow_scope(&[], None).is_empty());
    }

    #[test]
    fn sanitize_strips_control_chars() {
        assert_eq!(sanitize("a\nb\tc\0d"), "a b c d");
    }

    #[test]
    fn search_requires_read_and_search_ceiling() {
        // Slice-1 sanity: every sealed domain grants search (all RO_SEARCH).
        let ds = domains_granting(Intent::Search);
        assert!(ds.contains(&"projects"));
    }
}
