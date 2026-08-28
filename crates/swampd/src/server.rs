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
use crate::crawl;
use crate::embed::SemanticCtx;
use crate::index::{Hit, Index, Intent};
use crate::linux_uapi::{self, PollFd, POLLIN};
use crate::watch::{Freshness, Watcher};
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
    /// The semantic tier context, or `None` when no embedding provider is configured. Its presence adds
    /// similarity ranking on `search`; its absence (or a per-query provider failure) degrades to the FTS
    /// floor with `semantic unavailable`. It NEVER affects authority — the frozen gate runs regardless.
    sem: Option<&'a SemanticCtx<'a>>,
}

impl<'a> Server<'a> {
    pub fn new(
        index: &'a Index,
        authority_dir: PathBuf,
        allowed_uids: Vec<u32>,
        sem: Option<&'a SemanticCtx<'a>>,
    ) -> Self {
        Server { index, authority_dir, allowed_uids, sem }
    }

    /// Bind the query socket and run the single-thread reactor: `poll` the query listener AND the
    /// inotify watcher fd together, so live map-repair (§6) and caller-scoped queries (§9) share one
    /// thread, one `rusqlite::Connection`, and no locks. Queries are fast and swampd is availability-
    /// plane, so a synchronous reactor is right; the watcher never blocks a query and vice-versa.
    ///
    /// `freshness` is the boot state (STALE until the initial reconcile + arm succeeded). It flips to
    /// STALE on an inotify overflow or a watch failure and returns to FRESH only after a successful
    /// full reconcile AND watcher re-arm. Every query response carries the current freshness.
    pub fn serve(
        &self,
        sock_path: &Path,
        watcher: &mut Watcher,
        home: &Path,
        mut freshness: Freshness,
    ) -> std::io::Result<()> {
        let _ = std::fs::remove_file(sock_path);
        let listener = UnixListener::bind(sock_path)?;
        listener.set_nonblocking(true)?;
        let _ = std::fs::set_permissions(sock_path, std::fs::Permissions::from_mode(0o666));
        eprintln!(
            "swampd: query socket {} (allowed uids {:?}) freshness={}",
            sock_path.display(),
            self.allowed_uids,
            freshness.wire()
        );
        let lfd = listener.as_raw_fd();
        loop {
            // A hard-broken inotify fd is dropped from the set (fd < 0 ⇒ `poll` ignores it) so we serve a
            // STALE static index instead of busy-spinning on a permanently-ready error fd.
            let wfd = if watcher.broken() { -1 } else { watcher.raw_fd() };
            if watcher.broken() && freshness == Freshness::Fresh {
                freshness = Freshness::Stale;
            }
            let mut fds = [
                PollFd { fd: lfd, events: POLLIN, revents: 0 },
                PollFd { fd: wfd, events: POLLIN, revents: 0 },
            ];
            match linux_uapi::poll(&mut fds, -1) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }

            // Watcher first: applying pending fs events keeps the map current before we answer queries.
            if fds[1].revents & POLLIN != 0 {
                let overflow = watcher.process(self.index, home, self.sem);
                if overflow {
                    // Dropped events → the incremental map is untrustworthy. Full reconcile + re-arm,
                    // then recompute freshness. FRESH only if BOTH succeed (fork 1).
                    watcher.reset_health();
                    let stats = crawl::reconcile_full(self.index, home, watcher, self.sem);
                    freshness = if watcher.healthy() { Freshness::Fresh } else { Freshness::Stale };
                    eprintln!(
                        "swampd: post-overflow reconcile objects={} deleted={} freshness={}",
                        stats.objects, stats.deleted, freshness.wire()
                    );
                } else if !watcher.healthy() && freshness == Freshness::Fresh {
                    // A watch/read failure during incremental apply — degrade until the next reconcile.
                    freshness = Freshness::Stale;
                    eprintln!("swampd: watcher degraded — serving STALE until reconcile");
                }
            }

            if fds[0].revents & POLLIN != 0 {
                loop {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            if let Err(e) = self.handle(stream, freshness) {
                                eprintln!("swampd: query conn error: {e}");
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(e) => {
                            eprintln!("swampd: accept error: {e}");
                            break;
                        }
                    }
                }
            }
        }
    }

    fn handle(&self, mut stream: UnixStream, freshness: Freshness) -> std::io::Result<()> {
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

        // The FTS/metadata result — the MANDATORY LEXICAL FLOOR, produced by the FROZEN authorization
        // gate, untouched by this slice. It is always computed and always authoritative for correctness.
        let fts_hits = self
            .index
            .query(req.intent, &req.query, &effective, &verb_domains, req.limit)
            .unwrap_or_default();

        // Semantic tier — ADDITIVE, and only on `search`. It is `available` iff a provider is configured
        // AND this query's terms embed successfully; on absence or ANY provider failure we serve the FTS
        // floor and report `unavailable` (§1.2 T7 — provider loss degrades, never disables). The semantic
        // candidate set is built by the SAME scope+ceiling as the FTS gate, so merging two authorized
        // result sets can never widen authority (§1.1).
        let (hits, semantic_avail) = match (req.intent, self.sem) {
            (Intent::Search, Some(ctx)) => match ctx.embed_query(&req.query) {
                Some(qvec) => {
                    let sem_hits = self
                        .index
                        .semantic_query(&qvec, &ctx.semantic_version, &effective, &verb_domains, req.limit)
                        .unwrap_or_default();
                    (merge_semantic_led(sem_hits, fts_hits, req.limit), true)
                }
                None => (fts_hits, false), // provider unreachable/erroring → FTS floor
            },
            _ => (fts_hits, false), // discover intent, or no provider → lexical/metadata only
        };

        // Freshness (slice-3) and semantic-availability (slice-4) ride as header lines after RESULT and
        // before the hits. Existing consumers ignore unrecognized lines, so this stays backward-
        // compatible; the RESULT/hit/END structural core is unchanged, preserving slice-2's deny-vs-zero-
        // hit indistinguishability. BOTH signals are index-global — they disclose nothing about which
        // sessions or objects exist.
        writeln!(stream, "RESULT {}", hits.len())?;
        writeln!(stream, "freshness {}", freshness.wire())?;
        writeln!(stream, "semantic {}", if semantic_avail { "available" } else { "unavailable" })?;
        for h in &hits {
            writeln!(stream, "hit {}\t{}", sanitize(&h.path), sanitize(&h.snippet))?;
        }
        writeln!(stream, "END")?;
        Ok(())
    }
}

/// Semantic-led fusion (slice-4 Fork F6 — a prior retrieval bake-off proved equal-weight RRF drags a strong
/// semantic ranking DOWN). Lead with the semantic order, then append lexical hits not already present as
/// the exact-match booster/floor. Deduped by path, capped at `limit`. Both inputs already passed the
/// SAME authority gate, so their union is authorized by construction.
fn merge_semantic_led(semantic: Vec<Hit>, lexical: Vec<Hit>, limit: usize) -> Vec<Hit> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(limit.min(semantic.len() + lexical.len()));
    for h in semantic.into_iter().chain(lexical) {
        if out.len() >= limit {
            break;
        }
        if seen.insert(h.path.clone()) {
            out.push(h);
        }
    }
    out
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
