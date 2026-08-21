//! crawl / reconcile — the map builder (swamp.md §7). Walks the sealed allow-set member trees,
//! cheapest first: Phase-1 metadata/structural (path, dev/ino, size, mtime, owning domain) for every
//! object, plus Phase-2 FTS text extraction for readable text files.
//!
//! Slice-3 promotes the one-shot crawl into a **reconcile** and folds watcher-arming into the SAME
//! walk: every directory is handed to [`crate::watch::Watcher`] the moment it is mapped, so a create
//! that races the walk is caught by a queued inotify event instead of being lost. The single per-object
//! apply — [`index_object`] — is also what the live watcher calls for an individual file event, so the
//! crawl path and the event path enrich objects through identical policy checks.
//!
//! The walk runs AFTER swampd has Landlocked itself ([`crate::confine`]): the kernel is the outer wall
//! (a read outside the allow-set simply fails), and this module is the userspace mirror — it consults
//! the SAME sealed [`shrek_policy::swamp`] policy per object, so an object the kernel would deny is also
//! one the walker declines to map. Never-indexable markers and heavy build/cache dirs (§7) are pruned.

use crate::index::{Index, ObjectRecord};
use crate::watch::Watcher;
use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// Directories pruned entirely from the walk (swamp.md §7 exclusions): build output, caches, VCS
/// internals, dependency trees. Skipped for mapping, enrichment AND watching. Never widens what is
/// readable; only narrows what is walked, on top of the allow-set.
const PRUNE_DIRS: &[&str] =
    &["node_modules", "target", ".git", ".cache", ".cargo", "Steam", ".venv", "__pycache__"];

/// Max bytes read for FTS extraction. Larger files are mapped (metadata) but not text-extracted.
const MAX_FTS_BYTES: u64 = 512 * 1024;

#[derive(Default, Debug)]
pub struct Stats {
    pub objects: u64,
    pub texts: u64,
    pub pruned: u64,
    pub skipped_never: u64,
    pub deleted: u64,
}

/// Full reconcile (startup, and after an inotify overflow): walk every sealed member tree, upsert every
/// live object, arm a watch on every live directory, then PRUNE every DB row whose path is no longer
/// live (vanished, or left the allow-set, while swampd was down). This is what makes persistence honest
/// across a restart/reboot — the on-disk index is reconciled to reality before serving. Returns coverage.
pub fn reconcile_full(index: &Index, home: &Path, watcher: &mut Watcher) -> Stats {
    let mut stats = Stats::default();
    let mut live: HashSet<String> = HashSet::new();
    for d in shrek_policy::swamp::INDEXABLE_DOMAINS {
        for m in d.members {
            let root = home.join(m);
            if root.exists() {
                walk(index, home, &root, watcher, Some(&mut live), &mut stats);
            }
        }
    }
    stats.deleted = index.prune_absent(&live).unwrap_or(0);
    stats
}

/// Reconcile a single subtree (a directory that was just created or moved in): arm its watch as we
/// descend and upsert everything under it. NO prune — a brand-new subtree can have nothing stale, and
/// pruning would need the global live set. Closes the recursive-watch race: children that appeared
/// between the mkdir and the watch being armed are picked up here.
pub fn reconcile_subtree(index: &Index, home: &Path, watcher: &mut Watcher, root: &Path) {
    let mut stats = Stats::default();
    walk(index, home, root, watcher, None, &mut stats);
}

/// Map (or refresh) exactly one object at `path` — the unit shared by the crawl walk and the live
/// watcher. Applies the full policy gate, then upserts metadata and (re)extracts or clears FTS. Returns
/// the coverage delta. If the object is a symlink, unreadable, never-indexable, in a pruned dir, or
/// outside the allow-set, any stale record for that exact path is removed and nothing is mapped — so an
/// event that turns a mapped file into e.g. a symlink or a `.env` correctly retracts it from the index.
pub fn index_object(index: &Index, home: &Path, path: &Path) -> Stats {
    let mut stats = Stats::default();
    let key = path.to_string_lossy().to_string();

    let Ok(md) = std::fs::symlink_metadata(path) else {
        // Gone or unreadable → ensure no stale record survives (belt-and-suspenders with delete events).
        stats.deleted += index.delete_path(&key).map(u64::from).unwrap_or(0);
        return stats;
    };
    if md.file_type().is_symlink() {
        stats.deleted += index.delete_path(&key).map(u64::from).unwrap_or(0);
        return stats;
    }

    let rel = match path.strip_prefix(home) {
        Ok(r) => r.to_string_lossy().to_string(),
        Err(_) => return stats,
    };
    if shrek_policy::swamp::is_never_indexable(&rel) {
        stats.skipped_never += 1;
        stats.deleted += index.delete_path(&key).map(u64::from).unwrap_or(0);
        return stats;
    }
    if md.is_dir() {
        if let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) {
            if PRUNE_DIRS.iter().any(|p| *p == name) {
                stats.pruned += 1;
                return stats;
            }
        }
    }
    let Some(domain) = shrek_policy::swamp::domain_for(&rel) else {
        // Left the allow-set (e.g. moved out of a granted tree) → retract any stale record.
        stats.deleted += index.delete_path(&key).map(u64::from).unwrap_or(0);
        return stats;
    };

    // Skip re-extracting FTS when a file's content-affecting metadata is unchanged (cheap live updates).
    let unchanged = index.meta_for(&key) == Some((md.mtime(), md.size() as i64));

    let rec = ObjectRecord {
        path: key.clone(),
        dev: md.dev() as i64,
        ino: md.ino() as i64,
        size: md.size() as i64,
        mtime: md.mtime(),
        domain: domain.name.to_string(),
        is_dir: md.is_dir(),
    };
    let Ok(id) = index.upsert_object(&rec) else { return stats };
    stats.objects += 1;

    if md.is_file() {
        if md.size() <= MAX_FTS_BYTES {
            if unchanged {
                stats.texts += 1; // already enriched at the same (mtime,size); leave FTS in place
            } else if let Some(text) = extract_text(path) {
                if index.set_fts(id, &text).is_ok() {
                    stats.texts += 1;
                }
            } else {
                // Content is now binary/unreadable → its old text must stop being searchable.
                let _ = index.clear_fts(id);
            }
        } else {
            // Grew past the cap → mapped but no longer text-searchable.
            let _ = index.clear_fts(id);
        }
    }
    stats
}

fn walk(
    index: &Index,
    home: &Path,
    dir: &Path,
    watcher: &mut Watcher,
    mut live: Option<&mut HashSet<String>>,
    stats: &mut Stats,
) {
    // Map + arm THIS directory first, so it is being watched before we enumerate its children.
    let out = index_object(index, home, dir);
    accumulate(stats, &out);
    if out.objects == 0 {
        // Pruned / never-indexable / outside allow-set — do not watch or descend.
        return;
    }
    if let Some(l) = live.as_deref_mut() {
        l.insert(dir.to_string_lossy().to_string());
    }
    watcher.arm_dir(dir);

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(md) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if md.file_type().is_symlink() {
            // Never follow a symlink: it could escape the allow-set (kernel would deny anyway) or cycle.
            continue;
        }
        if md.is_dir() {
            walk(index, home, &path, watcher, live.as_deref_mut(), stats);
        } else {
            let out = index_object(index, home, &path);
            if out.objects > 0 {
                if let Some(l) = live.as_deref_mut() {
                    l.insert(path.to_string_lossy().to_string());
                }
            }
            accumulate(stats, &out);
        }
    }
}

fn accumulate(dst: &mut Stats, src: &Stats) {
    dst.objects += src.objects;
    dst.texts += src.texts;
    dst.pruned += src.pruned;
    dst.skipped_never += src.skipped_never;
    dst.deleted += src.deleted;
}

/// Read a file as searchable UTF-8 text, or None if it is binary/unreadable. A cheap heuristic: read
/// the bytes, reject if a NUL appears (binary) or it is not valid UTF-8.
fn extract_text(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}
