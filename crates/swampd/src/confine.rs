//! confine — swampd Landlocks ITSELF to the sealed indexable allow-set before it reads a single byte
//! of user data (swamp.md §5, security-model.md §5). This is THE spine of the confused-deputy defence:
//! after [`Confinement::enforce`] returns, an `open()` of any path outside the allow-set fails at the
//! KERNEL boundary — the bytes of `~/Vault`, `~/.ssh`, another user's home never enter swampd's
//! address space, so a swampd compromise leaks nothing from a protected domain.
//!
//! The ruleset is default-DENY by construction: it HANDLES the full filesystem access set (so every
//! unallowed access is denied) and then ALLOWs a small, explicit set of roots:
//!   * SYSTEM roots (read+exec) — what the daemon itself needs to run: `/usr`, `/etc`, `/proc`, … .
//!   * INDEX roots (read only, NO exec) — the sealed member trees swampd may map (the allow-set). A
//!     path not under one of these is simply never allowed, so default-deny covers it — including any
//!     `~/NewSecrets` created AFTER the ruleset was built (security-model.md §5).
//!   * The AUTHORITY dir (read) — where gatekeeperd drops session grant records (§9).
//!   * The STATE dir (read/write) — swampd's own scratch: the index db + the query socket.
//!
//! Reuses the proven Landlock ABI (see [`crate::linux_uapi`], mirrored from gatekeeperd). It is NOT a
//! general sandbox: it builds exactly one ruleset for this one daemon. Fails CLOSED — if Landlock is
//! absent/disabled, `enforce` errors and swampd must refuse to serve (never run unconfined, §5).

use crate::linux_uapi as k;
use std::ffi::CString;
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

/// The filesystem access rights the ruleset HANDLES, masked to the probed ABI. Everything handled but
/// not explicitly allowed on some root is DENIED. Mirrors gatekeeperd's `handled_fs_for_abi`: v1 =
/// bits 0..=12, +REFER at v2, +TRUNCATE at v3, +IOCTL_DEV at v5 (handling a right the kernel predates
/// makes `landlock_create_ruleset` return EINVAL, so we clamp).
fn handled_fs_for_abi(abi: i64) -> u64 {
    let mut m: u64 = (1 << 13) - 1;
    if abi >= 2 {
        m |= k::LANDLOCK_ACCESS_FS_REFER;
    }
    if abi >= 3 {
        m |= k::LANDLOCK_ACCESS_FS_TRUNCATE;
    }
    if abi >= 5 {
        m |= k::LANDLOCK_ACCESS_FS_IOCTL_DEV;
    }
    m
}

/// A root the ruleset allows, and the access it grants there (pre-mask).
struct Rule {
    path: PathBuf,
    access: u64,
    /// If true, the root MUST exist — a missing one is a fail-closed error (e.g. the state dir).
    /// If false, a missing root is silently skipped (an absent member tree is just not indexable —
    /// default-deny already covers it, so its absence must not weaken or abort confinement).
    required: bool,
}

/// The confinement plan: the closed set of roots swampd may touch. Built in `main` from the sealed
/// [`shrek_policy::swamp`] allow-set (expanded against `$HOME`) plus the daemon's operational dirs.
pub struct Confinement {
    rules: Vec<Rule>,
}

impl Confinement {
    pub fn new() -> Self {
        Confinement { rules: Vec::new() }
    }

    /// System dirs the daemon needs to run: read + execute (loader, NSS, `/proc/self`, `/dev/urandom`
    /// for SQLite). These are not protected user domains. Missing ones are skipped.
    pub fn system_read(&mut self, path: impl Into<PathBuf>) -> &mut Self {
        self.rules.push(Rule {
            path: path.into(),
            access: k::LANDLOCK_ACCESS_FS_EXECUTE
                | k::LANDLOCK_ACCESS_FS_READ_FILE
                | k::LANDLOCK_ACCESS_FS_READ_DIR,
            required: false,
        });
        self
    }

    /// A sealed index member tree: READ ONLY, no execute. swampd reads user files to map/extract them;
    /// it must never be able to EXECUTE one (defence in depth — a compromise can read its allow-set,
    /// never run its contents). Missing member trees are skipped (default-deny covers them).
    pub fn index_read(&mut self, path: impl Into<PathBuf>) -> &mut Self {
        self.rules.push(Rule {
            path: path.into(),
            access: k::LANDLOCK_ACCESS_FS_READ_FILE | k::LANDLOCK_ACCESS_FS_READ_DIR,
            required: false,
        });
        self
    }

    /// The session-authority record dir (gatekeeperd writes, swampd reads): READ ONLY. Required —
    /// without it swampd could authorize no caller, so its absence is a fail-closed error.
    pub fn authority_read(&mut self, path: impl Into<PathBuf>) -> &mut Self {
        self.rules.push(Rule {
            path: path.into(),
            access: k::LANDLOCK_ACCESS_FS_READ_FILE | k::LANDLOCK_ACCESS_FS_READ_DIR,
            required: true,
        });
        self
    }

    /// swampd's own scratch (index db + query socket): full read/write/create/remove. Required.
    pub fn state_rw(&mut self, path: impl Into<PathBuf>) -> &mut Self {
        self.rules.push(Rule {
            path: path.into(),
            access: k::LANDLOCK_ACCESS_FS_READ_FILE
                | k::LANDLOCK_ACCESS_FS_READ_DIR
                | k::LANDLOCK_ACCESS_FS_WRITE_FILE
                | k::LANDLOCK_ACCESS_FS_MAKE_REG
                | k::LANDLOCK_ACCESS_FS_MAKE_DIR
                | k::LANDLOCK_ACCESS_FS_MAKE_SOCK
                | k::LANDLOCK_ACCESS_FS_MAKE_FIFO
                | k::LANDLOCK_ACCESS_FS_REMOVE_FILE
                | k::LANDLOCK_ACCESS_FS_REMOVE_DIR
                | k::LANDLOCK_ACCESS_FS_TRUNCATE,
            required: true,
        });
        self
    }

    /// Build and irreversibly enforce the ruleset on the calling (single) thread. After this returns
    /// Ok, swampd is confined for the rest of its life. Any failure is fail-closed: the caller must
    /// exit rather than proceed unconfined.
    pub fn enforce(&self) -> io::Result<()> {
        let abi = k::landlock_abi_version().map_err(|e| {
            io::Error::new(e.kind(), format!("landlock unavailable ({e}) — refusing to run unconfined"))
        })?;
        if abi < 1 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "landlock abi < 1 — refusing to run unconfined",
            ));
        }
        let handled = handled_fs_for_abi(abi);
        let attr = k::LandlockRulesetAttr {
            handled_access_fs: handled,
            handled_access_net: 0,
            scoped: 0,
        };
        let ruleset = k::landlock_create_ruleset(&attr)?;

        let mut allowed_any = false;
        for rule in &self.rules {
            let cpath = match CString::new(rule.path.as_os_str().as_encoded_bytes()) {
                Ok(c) => c,
                Err(_) => {
                    // A NUL in a path is nonsense; a required root failing this is fatal.
                    if rule.required {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!("invalid root path {:?}", rule.path),
                        ));
                    }
                    continue;
                }
            };
            let dir = match k::open_path_dir(&cpath) {
                Ok(fd) => fd,
                Err(e) => {
                    if rule.required {
                        return Err(io::Error::new(
                            e.kind(),
                            format!("required confinement root {:?} unavailable: {e}", rule.path),
                        ));
                    }
                    // Optional root absent — default-deny covers it; skip without weakening the wall.
                    continue;
                }
            };
            let pb = k::LandlockPathBeneathAttr {
                allowed_access: rule.access & handled,
                parent_fd: dir.as_raw_fd(),
            };
            k::landlock_add_path_beneath(ruleset.as_raw_fd(), &pb)?;
            allowed_any = true;
        }
        if !allowed_any {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "confinement has no allowable roots — would deny swampd all filesystem access",
            ));
        }

        // no_new_privs is the precondition for unprivileged restrict_self; then the wall goes up.
        k::set_no_new_privs()?;
        k::landlock_restrict_self(ruleset.as_raw_fd())?;
        Ok(())
    }
}

impl Default for Confinement {
    fn default() -> Self {
        Self::new()
    }
}

/// Expand the sealed allow-set into concrete `$HOME`-relative member roots. Pure joining — no I/O,
/// no canonicalization (nonexistent members are handled by `enforce` skipping them). The union of
/// these is exactly what the crawler is permitted to map, kept in lockstep with the crawler's own
/// per-object [`shrek_policy::swamp::is_indexable`] check so the kernel wall and the userspace skip
/// agree by construction.
pub fn index_member_roots(home: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for d in shrek_policy::swamp::INDEXABLE_DOMAINS {
        for m in d.members {
            roots.push(home.join(m));
        }
    }
    roots
}
