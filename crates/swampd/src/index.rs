//! index — the embedded store (swamp.md §4): SQLite metadata/structural tables + an FTS5 full-text
//! index, via bundled rusqlite. The filesystem stays authoritative; this is a cache of derived facts
//! (§4). Its ONE security-load-bearing method is [`Index::query`], which realizes the spine of
//! swamp.md §9: **authorize before retrieve.** The caller's session grants and the per-domain ceiling
//! are compiled INTO the SQL predicate, so an object outside the caller's authority is never a
//! candidate row — it is absent from the projection, never retrieved-then-filtered, and never
//! countable or inferable.
//!
//! Deliberately NOT exposed: raw bm25 scores or global match counts. FTS5's bm25 mixes corpus-wide
//! document-frequency stats that span out-of-scope docs; returning a score or a total would be a thin
//! side channel around the scope wall. `query` returns only in-scope paths + snippets drawn from the
//! in-scope document itself — never an aggregate over the whole corpus.

use rusqlite::{params, params_from_iter, Connection};
use std::path::{Path, PathBuf};

/// One object as the crawler discovers it (swamp.md §3, the fields this slice populates). `id` is
/// `(dev,ino)`-derived stable-ish identity for v1; the opaque rename-surviving id is deferred.
pub struct ObjectRecord {
    pub path: String,   // canonical absolute path — the PHYSICAL MAP key
    pub dev: i64,
    pub ino: i64,
    pub size: i64,
    pub mtime: i64,
    pub domain: String, // owning indexable-domain name — the STRUCTURAL MAP
    pub is_dir: bool,
}

/// A single query hit — the caller's projection of one in-scope object.
#[derive(Debug, PartialEq, Eq)]
pub struct Hit {
    pub path: String,
    /// A short excerpt from the object's OWN extracted text (FTS search only); empty for path search.
    pub snippet: String,
}

/// Which verb the query exercises — the escalation-ladder tier (filesystem-intelligence.md §5) and the
/// domain-ceiling verb it is gated on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Intent {
    /// Tier-1 path/metadata match. Gated on the domain's `discover` ceiling.
    Discover,
    /// Tier-2 full-text match. Gated on the domain's `search` ceiling (and `read` to return a snippet).
    Search,
}

pub struct Index {
    conn: Connection,
}

impl Index {
    /// Open (creating) the index at `path`. Use `:memory:` for tests. Schema is created idempotently.
    pub fn open(path: &Path) -> rusqlite::Result<Index> {
        let conn = Connection::open(path)?;
        Self::from_conn(conn)
    }

    /// Open a private in-memory index (tests).
    #[cfg(test)]
    pub fn open_in_memory() -> rusqlite::Result<Index> {
        Self::from_conn(Connection::open_in_memory()?)
    }

    fn from_conn(conn: Connection) -> rusqlite::Result<Index> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             -- Keep SQLite temp/spill files in memory: under swampd's Landlock confinement /tmp is
             -- not on the allow-set, so an on-disk temp would EACCES.
             PRAGMA temp_store=MEMORY;
             CREATE TABLE IF NOT EXISTS objects (
                 id      INTEGER PRIMARY KEY,
                 path    TEXT NOT NULL UNIQUE,
                 dev     INTEGER NOT NULL,
                 ino     INTEGER NOT NULL,
                 size    INTEGER NOT NULL,
                 mtime   INTEGER NOT NULL,
                 domain  TEXT NOT NULL,
                 is_dir  INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_objects_path ON objects(path);
             -- FTS5 external-content-less index keyed by rowid = objects.id. Its existence here is
             -- also the compile-time proof that the bundled SQLite has FTS5 enabled.
             CREATE VIRTUAL TABLE IF NOT EXISTS fts USING fts5(content, tokenize='unicode61');",
        )?;
        Ok(Index { conn })
    }

    /// Insert (or replace) an object; returns its row id.
    pub fn upsert_object(&self, o: &ObjectRecord) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO objects (path, dev, ino, size, mtime, domain, is_dir)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(path) DO UPDATE SET
                 dev=excluded.dev, ino=excluded.ino, size=excluded.size,
                 mtime=excluded.mtime, domain=excluded.domain, is_dir=excluded.is_dir",
            params![o.path, o.dev, o.ino, o.size, o.mtime, o.domain, o.is_dir as i64],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Attach extracted full-text to an object (FTS tier). `id` is the object row id.
    pub fn set_fts(&self, id: i64, text: &str) -> rusqlite::Result<()> {
        // Keep FTS rowid aligned with objects.id so the JOIN in `query` is exact.
        self.conn
            .execute("INSERT INTO fts (rowid, content) VALUES (?1, ?2)", params![id, text])?;
        Ok(())
    }

    /// The authorize-before-retrieve query. `grants` are the caller's canonical session grant roots
    /// (from the root-owned authority record, resolved independently — never caller-claimed);
    /// `verb_domains` are the domain names whose sealed ceiling grants the requested verb. Both are
    /// ANDed into the candidate predicate, so the result set is `matches ∩ session-grants ∩
    /// domain-ceiling`, constructed — not filtered — before any row is returned.
    ///
    /// An empty `grants` (a caller with no authority) or empty `verb_domains` yields an empty result
    /// by construction: the scope predicate is unsatisfiable, so nothing is discoverable. Fail-closed.
    pub fn query(
        &self,
        intent: Intent,
        terms: &str,
        grants: &[PathBuf],
        verb_domains: &[&str],
        limit: usize,
    ) -> rusqlite::Result<Vec<Hit>> {
        if grants.is_empty() || verb_domains.is_empty() || terms.is_empty() {
            return Ok(Vec::new());
        }

        // Scope predicate: object path is exactly a grant root or lies BENEATH it. `substr`-prefix
        // (not GLOB/LIKE) so paths containing glob metacharacters can neither escape nor over-match.
        let mut scope = String::from("(");
        let mut binds: Vec<String> = Vec::new();
        for (i, g) in grants.iter().enumerate() {
            let gs = g.to_string_lossy().to_string();
            if i > 0 {
                scope.push_str(" OR ");
            }
            // path = :g  OR  substr(path,1,len(:g)+1) = :g || '/'
            let a = binds.len() + 1;
            let b = binds.len() + 2;
            scope.push_str(&format!(
                "(o.path = ?{a} OR substr(o.path,1,length(?{b})+1) = ?{b} || '/')"
            ));
            binds.push(gs.clone()); // ?a
            binds.push(gs); // ?b
        }
        scope.push(')');

        // Domain ceiling: object's domain must be one whose sealed ceiling grants this verb.
        let dom_placeholders: Vec<String> =
            (0..verb_domains.len()).map(|i| format!("?{}", binds.len() + 1 + i)).collect();
        let dom_clause = format!("o.domain IN ({})", dom_placeholders.join(","));
        for d in verb_domains {
            binds.push((*d).to_string());
        }

        // The match term is the LAST bind.
        let term_idx = binds.len() + 1;
        binds.push(terms.to_string());
        let limit_idx = binds.len() + 1;

        let sql = match intent {
            Intent::Search => format!(
                "SELECT o.path, snippet(fts, 0, '[', ']', '…', 8) \
                 FROM fts JOIN objects o ON fts.rowid = o.id \
                 WHERE fts MATCH ?{term_idx} AND {scope} AND {dom_clause} \
                 ORDER BY bm25(fts) LIMIT ?{limit_idx}"
            ),
            Intent::Discover => format!(
                "SELECT o.path, '' \
                 FROM objects o \
                 WHERE {scope} AND {dom_clause} \
                 AND instr(lower(o.path), lower(?{term_idx})) > 0 \
                 ORDER BY o.path LIMIT ?{limit_idx}"
            ),
        };

        debug_assert_eq!(limit_idx, binds.len() + 1);
        let mut stmt = self.conn.prepare(&sql)?;
        // Params in slot order: every scope/domain/term bind is text, then the LIMIT as an integer.
        let mut all: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(binds.len() + 1);
        for s in binds {
            all.push(Box::new(s));
        }
        all.push(Box::new(limit as i64));

        let rows = stmt.query_map(params_from_iter(all.iter().map(|b| b.as_ref())), |r| {
            Ok(Hit { path: r.get(0)?, snippet: r.get::<_, String>(1).unwrap_or_default() })
        })?;
        let mut hits = Vec::new();
        for h in rows {
            hits.push(h?);
        }
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed() -> Index {
        let idx = Index::open_in_memory().expect("fts5 must be compiled into bundled sqlite");
        // Two indexable projects under the same domain + one object that models a Vault file that the
        // CRAWLER would never insert. We insert it here anyway to prove the QUERY GATE never reveals
        // it once it is out of the caller's grant scope.
        let objs = [
            ("/home/u/Projects/app-a/src/main.rs", "projects", "fn main() { isolation tiers }"),
            ("/home/u/Projects/app-a/README.md", "projects", "app-a isolation overview"),
            ("/home/u/Projects/app-b/src/lib.rs", "projects", "isolation secret sauce in app-b"),
            ("/home/u/Vault/passwords.txt", "projects", "isolation master password hunter2"),
        ];
        for (i, (p, dom, text)) in objs.iter().enumerate() {
            let id = idx
                .upsert_object(&ObjectRecord {
                    path: (*p).into(),
                    dev: 1,
                    ino: 100 + i as i64,
                    size: text.len() as i64,
                    mtime: 0,
                    domain: (*dom).into(),
                    is_dir: false,
                })
                .unwrap();
            idx.set_fts(id, text).unwrap();
        }
        idx
    }

    #[test]
    fn fts5_is_available_and_matches() {
        let idx = seed();
        let grants = vec![PathBuf::from("/home/u/Projects")];
        let hits = idx.query(Intent::Search, "isolation", &grants, &["projects"], 50).unwrap();
        assert!(!hits.is_empty(), "FTS5 search returned nothing — is FTS5 compiled in?");
    }

    #[test]
    fn scope_narrows_to_granted_subtree_only() {
        let idx = seed();
        // Session granted ONLY app-a. A search for a term present in app-a, app-b AND Vault must
        // return app-a hits ONLY — app-b and Vault are out of scope, so they are ABSENT, not filtered.
        let grants = vec![PathBuf::from("/home/u/Projects/app-a")];
        let hits = idx.query(Intent::Search, "isolation", &grants, &["projects"], 50).unwrap();
        assert!(!hits.is_empty());
        for h in &hits {
            assert!(h.path.contains("/app-a/"), "leaked out-of-scope object: {}", h.path);
        }
        assert!(!hits.iter().any(|h| h.path.contains("app-b")));
        assert!(!hits.iter().any(|h| h.path.contains("Vault")));
    }

    #[test]
    fn vault_token_is_never_discoverable_even_by_content() {
        let idx = seed();
        // "hunter2" exists ONLY in the Vault object. A session scoped to Projects/app-a must not be
        // able to find it by full-text — the token never enters the candidate set.
        let grants = vec![PathBuf::from("/home/u/Projects/app-a")];
        let hits = idx.query(Intent::Search, "hunter2", &grants, &["projects"], 50).unwrap();
        assert!(hits.is_empty(), "Vault FTS token leaked into an unauthorized session: {hits:?}");
    }

    #[test]
    fn empty_grants_reveal_nothing() {
        let idx = seed();
        // A caller with NO authority record (no grants) discovers nothing — fail-closed.
        let hits = idx.query(Intent::Search, "isolation", &[], &["projects"], 50).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn empty_verb_domains_reveal_nothing() {
        let idx = seed();
        // No domain grants the verb ⇒ ceiling excludes everything ⇒ empty projection.
        let grants = vec![PathBuf::from("/home/u/Projects")];
        let hits = idx.query(Intent::Search, "isolation", &grants, &[], 50).unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn discover_path_search_is_also_scoped() {
        let idx = seed();
        let grants = vec![PathBuf::from("/home/u/Projects/app-a")];
        // Path/name search for "src" — present under app-a and app-b — returns app-a only.
        let hits = idx.query(Intent::Discover, "src", &grants, &["projects"], 50).unwrap();
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|h| h.path.contains("/app-a/")));
        // And a Vault path is never discoverable from this scope.
        let vault = idx.query(Intent::Discover, "passwords", &grants, &["projects"], 50).unwrap();
        assert!(vault.is_empty());
    }

    #[test]
    fn sibling_prefix_does_not_leak_across_substring_boundary() {
        // A grant of /home/u/Projects/app must NOT match /home/u/Projects/app-b (component boundary).
        let idx = Index::open_in_memory().unwrap();
        for (i, p) in ["/home/u/Projects/app/x.rs", "/home/u/Projects/app-b/y.rs"].iter().enumerate() {
            let id = idx
                .upsert_object(&ObjectRecord {
                    path: (*p).into(),
                    dev: 1,
                    ino: i as i64,
                    size: 1,
                    mtime: 0,
                    domain: "projects".into(),
                    is_dir: false,
                })
                .unwrap();
            idx.set_fts(id, "isolation token").unwrap();
        }
        let grants = vec![PathBuf::from("/home/u/Projects/app")];
        let hits = idx.query(Intent::Search, "isolation", &grants, &["projects"], 50).unwrap();
        assert_eq!(hits.len(), 1, "sibling app-b leaked across a substring boundary: {hits:?}");
        assert!(hits[0].path.contains("/app/"));
    }
}
