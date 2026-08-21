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
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Bumped whenever the on-disk schema/semantics change. A persistent DB whose stored version differs
/// (older layout, or a corrupt/foreign file) is wiped and rebuilt by the startup reconcile — invalid
/// durable state fails toward a clean rebuild, never a corrupt serve (slice-3 §6).
const SCHEMA_VERSION: &str = "3";

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
             -- WAL + FULL sync = atomic, crash-safe commits on the durable /var index (slice-3 §6).
             PRAGMA synchronous=FULL;
             -- Keep SQLite temp/spill files in memory: under swampd's Landlock confinement /tmp is
             -- not on the allow-set, so an on-disk temp would EACCES.
             PRAGMA temp_store=MEMORY;
             CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);
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
             -- FTS5 content-storing index keyed by rowid = objects.id. Its existence here is also the
             -- compile-time proof that the bundled SQLite has FTS5 enabled. Rows are DELETE/re-INSERTed
             -- on live updates (slice-3), which a content-storing fts5 table supports directly.
             CREATE VIRTUAL TABLE IF NOT EXISTS fts USING fts5(content, tokenize='unicode61');",
        )?;
        let idx = Index { conn };
        idx.enforce_schema_version()?;
        Ok(idx)
    }

    /// Reconcile the persistent DB against the current [`SCHEMA_VERSION`]. A DB with no version (fresh),
    /// the current version (reuse in place), or a differing version (older layout / corrupt / foreign
    /// file → wipe objects+fts and rebuild) all resolve to a usable, current-schema store. The startup
    /// reconcile then repopulates. Never serves rows from an unknown layout.
    fn enforce_schema_version(&self) -> rusqlite::Result<()> {
        let stored: Option<String> = self
            .conn
            .query_row("SELECT v FROM meta WHERE k='schema_version'", [], |r| r.get(0))
            .ok();
        if stored.as_deref() != Some(SCHEMA_VERSION) {
            if stored.is_some() {
                // Present but mismatched → drop all derived rows; reconcile rebuilds from the filesystem.
                self.conn.execute_batch("DELETE FROM fts; DELETE FROM objects;")?;
            }
            self.conn.execute(
                "INSERT INTO meta (k, v) VALUES ('schema_version', ?1)
                 ON CONFLICT(k) DO UPDATE SET v=excluded.v",
                params![SCHEMA_VERSION],
            )?;
        }
        Ok(())
    }

    /// Insert (or update) an object; returns its STABLE row id. Uses `RETURNING id` — critical for live
    /// updates (slice-3): on the `ON CONFLICT DO UPDATE` path SQLite does NOT refresh
    /// `last_insert_rowid()`, so returning it would hand back a stale id and the caller's `set_fts` would
    /// write text under the wrong rowid, silently breaking the `fts.rowid = objects.id` join. `RETURNING`
    /// yields the affected row's id in both the insert and update cases.
    pub fn upsert_object(&self, o: &ObjectRecord) -> rusqlite::Result<i64> {
        self.conn.query_row(
            "INSERT INTO objects (path, dev, ino, size, mtime, domain, is_dir)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(path) DO UPDATE SET
                 dev=excluded.dev, ino=excluded.ino, size=excluded.size,
                 mtime=excluded.mtime, domain=excluded.domain, is_dir=excluded.is_dir
             RETURNING id",
            params![o.path, o.dev, o.ino, o.size, o.mtime, o.domain, o.is_dir as i64],
            |r| r.get(0),
        )
    }

    /// Attach (or REPLACE) extracted full-text for an object (FTS tier). `id` is the object row id.
    /// Idempotent: a prior FTS row for this id is deleted first, so a re-enrichment after a live edit
    /// (slice-3) replaces the text rather than duplicating it. Keeps FTS rowid aligned with objects.id
    /// so the JOIN in `query` is exact.
    pub fn set_fts(&self, id: i64, text: &str) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM fts WHERE rowid=?1", params![id])?;
        self.conn
            .execute("INSERT INTO fts (rowid, content) VALUES (?1, ?2)", params![id, text])?;
        Ok(())
    }

    /// Drop any FTS text for an object without touching its metadata row — used when a live edit turns a
    /// once-textual file binary/oversized (its content must stop being searchable). Safe if none exists.
    pub fn clear_fts(&self, id: i64) -> rusqlite::Result<()> {
        self.conn.execute("DELETE FROM fts WHERE rowid=?1", params![id])?;
        Ok(())
    }

    /// The `(mtime, size)` of the object at `path`, or None if unmapped. Lets the reconcile/watcher skip
    /// re-extracting FTS for a file whose content-affecting metadata is unchanged.
    pub fn meta_for(&self, path: &str) -> Option<(i64, i64)> {
        self.conn
            .query_row(
                "SELECT mtime, size FROM objects WHERE path=?1",
                params![path],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok()
    }

    /// Remove the object at exactly `path` (and its FTS text). Returns whether a row existed. Used for a
    /// single-file delete/moved-from event.
    pub fn delete_path(&self, path: &str) -> rusqlite::Result<bool> {
        let id: Option<i64> = self
            .conn
            .query_row("SELECT id FROM objects WHERE path=?1", params![path], |r| r.get(0))
            .ok();
        let Some(id) = id else { return Ok(false) };
        self.conn.execute("DELETE FROM fts WHERE rowid=?1", params![id])?;
        self.conn.execute("DELETE FROM objects WHERE id=?1", params![id])?;
        Ok(true)
    }

    /// Remove `prefix` AND everything component-wise beneath it (metadata AND FTS) — a directory delete
    /// or moved-from. Uses the SAME `path = p OR substr(path,1,len(p)+1)=p||'/'` boundary as the query
    /// scope, so `/a/app` never sweeps sibling `/a/app-b`. Returns the number of objects removed.
    pub fn delete_subtree(&self, prefix: &str) -> rusqlite::Result<u64> {
        let scope = "(path = ?1 OR substr(path,1,length(?1)+1) = ?1 || '/')";
        self.conn.execute(
            &format!("DELETE FROM fts WHERE rowid IN (SELECT id FROM objects WHERE {scope})"),
            params![prefix],
        )?;
        let n = self
            .conn
            .execute(&format!("DELETE FROM objects WHERE {scope}"), params![prefix])?;
        Ok(n as u64)
    }

    /// Delete every mapped object whose path is NOT in `live` (metadata AND FTS). This is the startup-
    /// reconcile prune: it removes rows for objects that vanished, or left the allow-set, while swampd
    /// was down — the mechanism behind "deletion really removes content" across a restart. Returns the
    /// number pruned.
    pub fn prune_absent(&self, live: &HashSet<String>) -> rusqlite::Result<u64> {
        let mut stale: Vec<i64> = Vec::new();
        {
            let mut stmt = self.conn.prepare("SELECT id, path FROM objects")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                let (id, path) = row?;
                if !live.contains(&path) {
                    stale.push(id);
                }
            }
        }
        for id in &stale {
            self.conn.execute("DELETE FROM fts WHERE rowid=?1", params![id])?;
            self.conn.execute("DELETE FROM objects WHERE id=?1", params![id])?;
        }
        Ok(stale.len() as u64)
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
    fn delete_path_removes_metadata_and_fts() {
        let idx = seed();
        let grants = vec![PathBuf::from("/home/u/Projects")];
        // Present before delete.
        assert!(!idx.query(Intent::Search, "isolation", &grants, &["projects"], 50).unwrap().is_empty());
        assert!(idx.delete_path("/home/u/Projects/app-a/src/main.rs").unwrap());
        // Its FTS token "tiers" is gone; and a re-delete reports it no longer exists.
        let hits = idx.query(Intent::Search, "tiers", &grants, &["projects"], 50).unwrap();
        assert!(hits.iter().all(|h| !h.path.contains("main.rs")));
        assert!(!idx.delete_path("/home/u/Projects/app-a/src/main.rs").unwrap());
    }

    #[test]
    fn delete_subtree_is_component_wise_and_clears_fts() {
        let idx = Index::open_in_memory().unwrap();
        for (i, p) in ["/home/u/Projects/app/x.rs", "/home/u/Projects/app/sub/y.rs", "/home/u/Projects/app-b/z.rs"]
            .iter()
            .enumerate()
        {
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
        // Deleting /app removes app + app/sub, but NOT sibling app-b.
        let removed = idx.delete_subtree("/home/u/Projects/app").unwrap();
        assert_eq!(removed, 2);
        let grants = vec![PathBuf::from("/home/u/Projects")];
        let hits = idx.query(Intent::Search, "isolation", &grants, &["projects"], 50).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].path.contains("/app-b/"));
    }

    #[test]
    fn prune_absent_removes_rows_not_in_live_set() {
        let idx = seed();
        // Only two of the four seeded objects are "live"; the rest must be pruned (metadata AND fts).
        let mut live = HashSet::new();
        live.insert("/home/u/Projects/app-a/src/main.rs".to_string());
        live.insert("/home/u/Projects/app-a/README.md".to_string());
        let pruned = idx.prune_absent(&live).unwrap();
        assert_eq!(pruned, 2);
        let grants = vec![PathBuf::from("/home/u/Projects")];
        // "secret sauce" lived only in app-b/lib.rs, now pruned → unsearchable.
        assert!(idx.query(Intent::Search, "sauce", &grants, &["projects"], 50).unwrap().is_empty());
        assert!(!idx.query(Intent::Search, "isolation", &grants, &["projects"], 50).unwrap().is_empty());
    }

    #[test]
    fn set_fts_is_idempotent_replace_not_append() {
        let idx = Index::open_in_memory().unwrap();
        let id = idx
            .upsert_object(&ObjectRecord {
                path: "/home/u/Projects/app/x.rs".into(),
                dev: 1,
                ino: 1,
                size: 1,
                mtime: 0,
                domain: "projects".into(),
                is_dir: false,
            })
            .unwrap();
        idx.set_fts(id, "alpha original").unwrap();
        idx.set_fts(id, "beta replacement").unwrap(); // re-enrich after an edit
        let grants = vec![PathBuf::from("/home/u/Projects")];
        // Old content is gone; new content is present; exactly one row (no duplicate rowid).
        assert!(idx.query(Intent::Search, "alpha", &grants, &["projects"], 50).unwrap().is_empty());
        assert_eq!(idx.query(Intent::Search, "beta", &grants, &["projects"], 50).unwrap().len(), 1);
    }

    #[test]
    fn clear_fts_removes_searchable_text_only() {
        let idx = Index::open_in_memory().unwrap();
        let id = idx
            .upsert_object(&ObjectRecord {
                path: "/home/u/Projects/app/x.rs".into(),
                dev: 1,
                ino: 1,
                size: 1,
                mtime: 0,
                domain: "projects".into(),
                is_dir: false,
            })
            .unwrap();
        idx.set_fts(id, "wastext nowbinary").unwrap();
        idx.clear_fts(id).unwrap();
        let grants = vec![PathBuf::from("/home/u/Projects")];
        // Content no longer full-text searchable, but the object is still discoverable by path.
        assert!(idx.query(Intent::Search, "wastext", &grants, &["projects"], 50).unwrap().is_empty());
        assert!(!idx.query(Intent::Discover, "x.rs", &grants, &["projects"], 50).unwrap().is_empty());
    }

    #[test]
    fn meta_for_reports_change() {
        let idx = Index::open_in_memory().unwrap();
        idx.upsert_object(&ObjectRecord {
            path: "/home/u/Projects/app/x.rs".into(),
            dev: 1,
            ino: 1,
            size: 10,
            mtime: 100,
            domain: "projects".into(),
            is_dir: false,
        })
        .unwrap();
        assert_eq!(idx.meta_for("/home/u/Projects/app/x.rs"), Some((100, 10)));
        assert_eq!(idx.meta_for("/nope"), None);
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
