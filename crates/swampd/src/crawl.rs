//! crawl — the initial map (swamp.md §7). One-shot walk of the sealed allow-set member trees, cheapest
//! first: Phase-1 metadata/structural (path, dev/ino, size, mtime, owning domain) for every object,
//! plus Phase-2 FTS text extraction for readable text files. The live self-repairing event pipeline
//! (§6, fanotify) is deferred — v1 is a point-in-time snapshot built at swampd start.
//!
//! The crawl runs AFTER swampd has Landlocked itself ([`crate::confine`]): the kernel is the outer
//! wall (a read outside the allow-set simply fails), and this module is the userspace mirror — it
//! consults the SAME sealed [`shrek_policy::swamp`] policy per object, so an object the kernel would
//! deny is also one the crawler declines to map. Never-indexable markers and heavy build/cache dirs
//! (§7) are pruned so the machine is not burned enriching noise.

use crate::index::{Index, ObjectRecord};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// Directories pruned entirely from the crawl (swamp.md §7 exclusions): build output, caches, VCS
/// internals, dependency trees. Skipped for BOTH mapping and enrichment at v1 — the "mapped-but-not-
/// enriched" nuance of §7 is a later refinement. This never widens what is readable; it only narrows
/// what is walked, on top of the allow-set.
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
}

/// Crawl every sealed member tree under `home`, inserting records into `index`. Returns coverage
/// stats. Errors reading an individual entry are skipped (best-effort, availability-plane §10), not
/// fatal — a permission blip on one file must never abort the whole map.
pub fn crawl(index: &Index, home: &Path) -> Stats {
    let mut stats = Stats::default();
    for d in shrek_policy::swamp::INDEXABLE_DOMAINS {
        for m in d.members {
            let root = home.join(m);
            if root.exists() {
                walk(index, home, &root, &mut stats);
            }
        }
    }
    stats
}

fn walk(index: &Index, home: &Path, dir: &Path, stats: &mut Stats) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // Never follow a symlink during the crawl: it could point outside the allow-set (the kernel
        // would deny the read anyway, but not walking it keeps the map free of escaping edges) or
        // create cycles. symlink_metadata does not traverse.
        let Ok(md) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if md.file_type().is_symlink() {
            continue;
        }

        let rel = match path.strip_prefix(home) {
            Ok(r) => r.to_string_lossy().to_string(),
            Err(_) => continue,
        };

        // Human-only markers (§5) are barred even nested under an indexable member — prune, don't map.
        if shrek_policy::swamp::is_never_indexable(&rel) {
            stats.skipped_never += 1;
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if md.is_dir() && PRUNE_DIRS.iter().any(|p| *p == name) {
            stats.pruned += 1;
            continue;
        }

        // Which sealed domain owns this object? If none (outside the allow-set), don't map it. This is
        // the userspace mirror of the Landlock wall — belt-and-suspenders with the kernel deny.
        let Some(domain) = shrek_policy::swamp::domain_for(&rel) else {
            continue;
        };

        let rec = ObjectRecord {
            path: path.to_string_lossy().to_string(),
            dev: md.dev() as i64,
            ino: md.ino() as i64,
            size: md.size() as i64,
            mtime: md.mtime(),
            domain: domain.name.to_string(),
            is_dir: md.is_dir(),
        };
        match index.upsert_object(&rec) {
            Ok(id) => {
                stats.objects += 1;
                // Phase-2: extract text for readable, not-too-large regular files.
                if md.is_file() && md.size() <= MAX_FTS_BYTES {
                    if let Some(text) = extract_text(&path) {
                        if index.set_fts(id, &text).is_ok() {
                            stats.texts += 1;
                        }
                    }
                }
            }
            Err(_) => continue,
        }

        if md.is_dir() {
            walk(index, home, &path, stats);
        }
    }
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
