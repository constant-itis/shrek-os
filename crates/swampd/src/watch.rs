//! watch — the live event pipeline (swamp.md §6), the inotify half of "the map repairs itself".
//!
//! Fork-1 (owner-adjudicated slice-3): the watcher is **inotify**, not fanotify. fanotify's
//! create/delete/rename-with-names needs `CAP_SYS_ADMIN`, which would break swampd's whole identity as
//! an unprivileged, Landlocked, availability-plane daemon (§5). inotify is unprivileged and works
//! entirely inside the sealed allow-set: swampd can only arm watches on directories the kernel already
//! lets it read, so an event under a denied domain never arrives — the wall bounds the watcher exactly
//! as it bounds the crawl. Raw syscalls live in [`crate::linux_uapi`] (no `notify` crate — minimal-deps).
//!
//! Every applied event re-runs the SAME policy gate as the crawl (via [`crate::crawl::index_object`] /
//! subtree reconcile), so a live create/rename can no more introduce an out-of-domain or never-indexable
//! row than the initial map could. Deletes remove metadata AND FTS. New directories are watched THEN
//! subtree-reconciled, closing the recursive-watch race (a child created before the watch is armed is
//! still caught by the immediate reconcile).

use crate::index::Index;
use crate::linux_uapi as k;
use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::os::fd::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};

/// The event classes swampd arms on each watched directory. `IN_ONLYDIR` makes an accidental add_watch
/// on a non-directory fail rather than silently mis-watch. `IN_CLOSE_WRITE` is the coalescing point —
/// one enrichment per file-close, not one per write syscall (§6).
const WATCH_MASK: u32 = k::IN_CREATE
    | k::IN_CLOSE_WRITE
    | k::IN_DELETE
    | k::IN_MOVED_FROM
    | k::IN_MOVED_TO
    | k::IN_DELETE_SELF
    | k::IN_MOVE_SELF
    | k::IN_ONLYDIR;

/// Whether the served index is known-current. Index-global (no per-session/per-object signal). STALE is
/// the conservative state: results may still be useful, but completeness / non-existence claims are
/// unsafe until a reconcile + watcher re-arm restores FRESH (slice-3 §3). Returning to FRESH requires
/// BOTH, never one alone.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Freshness {
    Fresh,
    Stale,
}

impl Freshness {
    /// The token emitted on the query wire (`freshness <fresh|stale>`).
    pub fn wire(&self) -> &'static str {
        match self {
            Freshness::Fresh => "fresh",
            Freshness::Stale => "stale",
        }
    }
}

pub struct Watcher {
    /// Owns the inotify fd (opened `IN_NONBLOCK` so the reactor drains it and stops on `EAGAIN`).
    file: File,
    /// wd → the absolute directory path it watches (to resolve `event.name` to a full path).
    wds: HashMap<i32, PathBuf>,
    /// Set when an `inotify_add_watch` fails (e.g. `ENOSPC` = watch limit) during the current arm pass.
    /// Sticky within a pass; the reactor clears it before a re-arm so freshness reflects the latest arm.
    degraded: bool,
    /// Set when the inotify fd itself hard-errors on read (should never happen for inotify). The reactor
    /// then drops the fd from its poll set — serving a STALE static index — rather than busy-spinning.
    broken: bool,
}

impl Watcher {
    /// Create the inotify instance. Failure here (no inotify support) is the caller's cue to serve STALE
    /// with no live updates — never to run unconfined or to crash the availability-plane daemon.
    pub fn new() -> std::io::Result<Watcher> {
        let fd = k::inotify_init1(k::IN_CLOEXEC | k::IN_NONBLOCK)?;
        Ok(Watcher { file: File::from(fd), wds: HashMap::new(), degraded: false, broken: false })
    }

    pub fn raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }

    /// Whether every watch in the current arm pass was armed successfully (and the fd still works).
    pub fn healthy(&self) -> bool {
        !self.degraded && !self.broken
    }

    /// The inotify fd hard-errored — the reactor must stop polling it (and serve STALE) to avoid spinning.
    pub fn broken(&self) -> bool {
        self.broken
    }

    /// Clear the degraded flag before a fresh full arm pass (startup / post-overflow re-arm).
    pub fn reset_health(&mut self) {
        self.degraded = false;
    }

    /// Arm (or idempotently re-arm) a watch on one directory. Called by the crawl/reconcile walk for
    /// every mapped directory, so watching and mapping share one traversal. A failure degrades freshness
    /// but never aborts — partial watching is a STALE index, not a dead one.
    pub fn arm_dir(&mut self, dir: &Path) {
        let Ok(cpath) = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()) else {
            self.degraded = true;
            return;
        };
        match k::inotify_add_watch(self.raw_fd(), &cpath, WATCH_MASK) {
            Ok(wd) => {
                self.wds.insert(wd, dir.to_path_buf());
            }
            Err(e) => {
                eprintln!("swampd: watch arm failed for {} ({e}) — index will serve STALE", dir.display());
                self.degraded = true;
            }
        }
    }

    /// Drain and apply all pending inotify events. Returns `true` if the kernel queue overflowed
    /// (`IN_Q_OVERFLOW`) — the reactor then does a full reconcile + re-arm and recomputes freshness,
    /// because an overflow means events were dropped and the incremental map can no longer be trusted.
    pub fn process(&mut self, index: &Index, home: &Path) -> bool {
        let mut overflow = false;
        let mut buf = [0u8; 16 * 1024];
        loop {
            let n = match self.file.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    eprintln!("swampd: inotify read error ({e}) — dropping watcher, serving STALE");
                    self.degraded = true;
                    self.broken = true;
                    break;
                }
            };
            let mut off = 0usize;
            while off + 16 <= n {
                let wd = i32::from_ne_bytes(buf[off..off + 4].try_into().unwrap());
                let mask = u32::from_ne_bytes(buf[off + 4..off + 8].try_into().unwrap());
                let len = u32::from_ne_bytes(buf[off + 12..off + 16].try_into().unwrap()) as usize;
                let name_start = off + 16;
                let name_end = name_start + len;
                if name_end > n {
                    break; // truncated (should not happen with whole-event reads); stop safely
                }
                let name = parse_name(&buf[name_start..name_end]);
                if self.apply_event(index, home, wd, mask, name.as_deref()) {
                    overflow = true;
                }
                off = name_end;
            }
        }
        overflow
    }

    /// Apply one event. Returns `true` only for `IN_Q_OVERFLOW`.
    fn apply_event(
        &mut self,
        index: &Index,
        home: &Path,
        wd: i32,
        mask: u32,
        name: Option<&str>,
    ) -> bool {
        if mask & k::IN_Q_OVERFLOW != 0 {
            eprintln!("swampd: inotify queue overflow — events dropped, forcing reconcile");
            return true;
        }
        if mask & k::IN_IGNORED != 0 {
            // The kernel removed this watch (dir gone / explicit rm). Reap the mapping.
            self.wds.remove(&wd);
            return false;
        }
        let Some(dir) = self.wds.get(&wd).cloned() else {
            return false; // event for a watch we already forgot
        };
        let is_dir = mask & k::IN_ISDIR != 0;

        // The watched directory itself was deleted or moved: retract its whole subtree and forget its
        // watches (IN_IGNORED will also arrive to reap the wd).
        if mask & (k::IN_DELETE_SELF | k::IN_MOVE_SELF) != 0 {
            let _ = index.delete_subtree(&dir.to_string_lossy());
            self.forget_subtree_watches(&dir);
            return false;
        }

        let Some(name) = name else { return false };
        let path = dir.join(name);
        let ps = path.to_string_lossy().to_string();

        if mask & (k::IN_DELETE | k::IN_MOVED_FROM) != 0 {
            if is_dir {
                let _ = index.delete_subtree(&ps);
                self.forget_subtree_watches(&path);
            } else {
                let _ = index.delete_path(&ps);
            }
            return false;
        }

        if mask & (k::IN_CREATE | k::IN_MOVED_TO) != 0 {
            if is_dir {
                // Arm the new dir THEN reconcile its subtree — the race-closing order (fork 1).
                crate::crawl::reconcile_subtree(index, home, self, &path);
            } else {
                let _ = crate::crawl::index_object(index, home, &path);
            }
            return false;
        }

        if mask & k::IN_CLOSE_WRITE != 0 {
            // Content settled → (re)map + (re)extract this one file.
            let _ = crate::crawl::index_object(index, home, &path);
        }
        false
    }

    /// Drop the wd mappings for `prefix` and everything component-wise beneath it (a directory that was
    /// deleted/moved). The kernel auto-removes the underlying watches; this keeps our map in step so a
    /// recycled wd is never mis-resolved to a stale path.
    fn forget_subtree_watches(&mut self, prefix: &Path) {
        self.wds.retain(|_, p| !path_at_or_beneath(p, prefix));
    }
}

/// The NUL-terminated (and NUL-padded) `name` field of an inotify_event → a `str`, or None if empty.
fn parse_name(raw: &[u8]) -> Option<String> {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    if end == 0 {
        return None;
    }
    Some(String::from_utf8_lossy(&raw[..end]).into_owned())
}

/// Is `p` equal to, or component-wise beneath, `prefix`? Component-wise so `/a/app` never matches
/// sibling `/a/app-b` — the same boundary the query scope and `delete_subtree` use.
fn path_at_or_beneath(p: &Path, prefix: &Path) -> bool {
    let mut a = prefix.components();
    let mut b = p.components();
    loop {
        match (a.next(), b.next()) {
            (Some(x), Some(y)) if x == y => continue,
            (Some(_), _) => return false,
            (None, _) => return true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshness_wire_tokens() {
        assert_eq!(Freshness::Fresh.wire(), "fresh");
        assert_eq!(Freshness::Stale.wire(), "stale");
    }

    #[test]
    fn parse_name_stops_at_nul_and_rejects_empty() {
        assert_eq!(parse_name(b"main.rs\0\0\0").as_deref(), Some("main.rs"));
        assert_eq!(parse_name(b"\0\0\0"), None);
        assert_eq!(parse_name(b""), None);
    }

    #[test]
    fn subtree_membership_is_component_wise() {
        assert!(path_at_or_beneath(Path::new("/a/app/x"), Path::new("/a/app")));
        assert!(path_at_or_beneath(Path::new("/a/app"), Path::new("/a/app")));
        assert!(!path_at_or_beneath(Path::new("/a/app-b"), Path::new("/a/app")));
        assert!(!path_at_or_beneath(Path::new("/a"), Path::new("/a/app")));
    }
}
