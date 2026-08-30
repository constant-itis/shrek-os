//! bench_record — the persistent state model for the Bench plane (ADR-003 Part 2 step 4).
//!
//! A **Bench** is the user-authority mutable-compute plane (ADR-002): a persistent, quota-capped
//! rootless-container home that a user installs tools into without touching the sealed `/usr`. Unlike a
//! T2 sandbox — whose grants live in a per-request private mount ns and die with the process — a Bench
//! outlives any single container and must survive reboot and `gatekeeperd` restarts. So its identity +
//! quota + (later, step 5) grant set live in a DURABLE record on the persistent `/home` plane, NOT the
//! volatile `/run` where `authority_record`/`net_binding` keep their ephemeral session state.
//!
//! Same dep-free line-text discipline as [`crate::net_binding`] / [`crate::authority_record`]: a fixed
//! header, one field per line, atomic temp+rename, and a fail-closed parser. The record is Bench
//! METADATA (name, id, project id, quota, state, grants) — not a secret — so it is `root`-owned but
//! world-readable (0644 in a 0755 dir), letting an unprivileged `shrek bench list` read it directly
//! without a broker hop, while only the privileged supervisor writes it.
//!
//! Boot re-issuance (step 4 skeleton, step 5 fills grants): `/home` is durable but `/run` is not, so at
//! boot the supervisor re-reads every record here and re-applies the volatile state (project quotas; and
//! later the FS/egress grants) — the records are the single source of truth a fresh boot rebuilds from.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{chown, PermissionsExt};
use std::path::{Path, PathBuf};

/// Default location of the durable Bench records — on the PERSISTENT `/home` plane (not volatile `/run`),
/// beside the container-storage pool. Overridable for the host/container oracle (no `/home`) via
/// `SHREK_BENCH_DIR`, mirroring the record env overrides elsewhere.
pub fn records_dir() -> PathBuf {
    std::env::var("SHREK_BENCH_DIR")
        .unwrap_or_else(|_| "/home/.shrek/benches/records".to_string())
        .into()
}

/// The base for allocated ext4 project ids. Kept clear of the low/reserved range (0 = "no project",
/// system ids) so a Bench quota can never collide with an unrelated project id on the shrek-data fs.
pub const PROJECT_ID_BASE: u32 = 100_000;

/// A Bench name must be a safe single-path-component token (it is the record filename AND the pool
/// sub-directory name), so it can never traverse out of the records/pool dirs. Alnum plus `._-`,
/// non-empty, ≤ 64, not `.`/`..`, and not a leading `.` (records are not hidden; the leaf is the name).
pub fn valid_bench_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.len() <= 64
        && !name.starts_with('.')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// The durable state of one Bench. `id` is a stable token used later (step 5) to derive the per-Bench
/// netns/nft identity ([`crate::net_plane::SandboxNet::for_id`]); in step 4 it equals `name`. `grants`
/// is empty until step 5 routes FS/egress grants through the gatekeeper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BenchRecord {
    pub name: String,
    pub id: String,
    /// ext4 project id backing this Bench's disk quota (0 = unset).
    pub project: u32,
    /// block hard limit in KiB (0 = no cap).
    pub quota_kib: u64,
    /// creation time (unix seconds). Passed by the caller — the sealed daemons avoid wall-clock reads.
    pub created: u64,
    /// `created` | `running` | `stopped`.
    pub state: String,
    /// granted host paths (step 5). Empty in step 4.
    pub grants: Vec<String>,
}

impl BenchRecord {
    /// Serialize to the dep-free line-text wire form. A path/field containing a newline is impossible in
    /// a valid record (names are validated; paths are canonical) — the caller guards grant paths.
    fn to_wire(&self) -> String {
        let mut s = String::from("SHREK-BENCH 1\n");
        s.push_str(&format!("name {}\n", self.name));
        s.push_str(&format!("id {}\n", self.id));
        s.push_str(&format!("project {}\n", self.project));
        s.push_str(&format!("quota_kib {}\n", self.quota_kib));
        s.push_str(&format!("created {}\n", self.created));
        s.push_str(&format!("state {}\n", self.state));
        for g in &self.grants {
            s.push_str(&format!("grant {g}\n"));
        }
        s.push_str("END\n");
        s
    }

    /// Parse from the wire form. Fail-closed: any missing/malformed required field or a bad header
    /// returns `None`, exactly as the other record parsers (a corrupt record is treated as absent).
    fn from_wire(body: &str) -> Option<BenchRecord> {
        let mut lines = body.lines();
        if lines.next()? != "SHREK-BENCH 1" {
            return None;
        }
        let mut name = None;
        let mut id = None;
        let mut project = None;
        let mut quota_kib = None;
        let mut created = None;
        let mut state = None;
        let mut grants = Vec::new();
        let mut saw_end = false;
        for line in lines {
            if line == "END" {
                saw_end = true;
                break;
            }
            let (k, v) = line.split_once(' ')?;
            match k {
                "name" => name = Some(v.to_string()),
                "id" => id = Some(v.to_string()),
                "project" => project = Some(v.parse::<u32>().ok()?),
                "quota_kib" => quota_kib = Some(v.parse::<u64>().ok()?),
                "created" => created = Some(v.parse::<u64>().ok()?),
                "state" => state = Some(v.to_string()),
                "grant" => grants.push(v.to_string()),
                _ => return None, // unknown field ⇒ fail closed (never silently ignore)
            }
        }
        let rec = BenchRecord {
            name: name?,
            id: id?,
            project: project?,
            quota_kib: quota_kib?,
            created: created?,
            state: state?,
            grants,
        };
        // The record's own name must be a safe token (defence in depth: the filename already is one).
        if !saw_end || !valid_bench_name(&rec.name) {
            return None;
        }
        Some(rec)
    }
}

/// Resolve `dev` uid/gid from /etc/passwd so records are traversable/readable by the desktop user's
/// `shrek bench list`. Returns `None` if absent (record then stays root-owned; still 0644-readable).
fn dev_ids() -> Option<(u32, u32)> {
    let passwd = fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() >= 4 && f[0] == "dev")
            .then(|| Some((f[2].parse().ok()?, f[3].parse().ok()?)))
            .flatten()
    })
}

/// Write (create or replace) a Bench's record. Atomic temp+rename so a reader never sees a partial
/// record. Mode 0644 in a 0755 dir — Bench metadata is not sensitive, so an unprivileged reader may list.
pub fn write_record(dir: &Path, rec: &BenchRecord) -> io::Result<PathBuf> {
    if !valid_bench_name(&rec.name) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid bench name"));
    }
    for g in &rec.grants {
        if g.contains('\n') {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "grant path contains newline"));
        }
    }
    fs::create_dir_all(dir)?;
    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o755));
    let path = dir.join(&rec.name);
    let tmp = dir.join(format!(".{}.tmp", rec.name));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(rec.to_wire().as_bytes())?;
        f.sync_all()?;
    }
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o644))?;
    if let Some((_uid, gid)) = dev_ids() {
        // Best-effort: root-owned but `dev`-group so an unprivileged `shrek bench list` can read.
        let _ = chown(dir, Some(0), Some(gid));
    }
    fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Load a Bench record by name. Fail-closed: a missing/malformed record returns `None`.
pub fn load_record(dir: &Path, name: &str) -> Option<BenchRecord> {
    if !valid_bench_name(name) {
        return None;
    }
    let body = fs::read_to_string(dir.join(name)).ok()?;
    BenchRecord::from_wire(&body)
}

/// Remove a Bench record (teardown). Idempotent — a missing record is not an error.
pub fn remove_record(dir: &Path, name: &str) -> io::Result<()> {
    if !valid_bench_name(name) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid bench name"));
    }
    match fs::remove_file(dir.join(name)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// List all valid Bench records (for `shrek bench list`, boot re-issuance, and project-id allocation).
/// Skips the temp files and any record that fails to parse — a corrupt record is invisible, not fatal.
pub fn list_records(dir: &Path) -> Vec<BenchRecord> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else { return out };
    for ent in rd.flatten() {
        let Some(fname) = ent.file_name().to_str().map(str::to_owned) else { continue };
        if fname.starts_with('.') {
            continue; // skip `.name.tmp` and any dotfiles
        }
        if let Some(rec) = load_record(dir, &fname) {
            out.push(rec);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Allocate the smallest unused ext4 project id at or above [`PROJECT_ID_BASE`], given the existing
/// records. Pure over the record set (unit-tested). A Bench reuses no live project id, so two Benches
/// never share a quota; a destroyed Bench frees its id (its record is gone) for reuse.
pub fn next_project_id(records: &[BenchRecord]) -> u32 {
    let used: std::collections::BTreeSet<u32> =
        records.iter().map(|r| r.project).filter(|&p| p >= PROJECT_ID_BASE).collect();
    let mut id = PROJECT_ID_BASE;
    while used.contains(&id) {
        id += 1;
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> PathBuf {
        // Unique per call — tests run in parallel threads (same pid), so a per-pid path would let one
        // test's setup nuke another's dir mid-run.
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let base = std::env::var("CARGO_TARGET_TMPDIR").unwrap_or_else(|_| "/tmp".into());
        let d = PathBuf::from(base)
            .join(format!("bench-rec-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = fs::remove_dir_all(&d);
        let _ = fs::create_dir_all(&d);
        d
    }

    fn rec(name: &str, project: u32) -> BenchRecord {
        BenchRecord {
            name: name.into(),
            id: name.into(),
            project,
            quota_kib: 1024,
            created: 42,
            state: "created".into(),
            grants: vec![],
        }
    }

    #[test]
    fn name_validation_blocks_traversal_and_dotfiles() {
        assert!(valid_bench_name("dev-scratch_1"));
        assert!(!valid_bench_name(""));
        assert!(!valid_bench_name("."));
        assert!(!valid_bench_name(".."));
        assert!(!valid_bench_name("a/b"));
        assert!(!valid_bench_name("../etc"));
        assert!(!valid_bench_name(".hidden")); // leading dot reserved for temp/dotfiles
        assert!(!valid_bench_name(&"x".repeat(65)));
    }

    #[test]
    fn write_then_load_roundtrips_all_fields() {
        let d = tmpdir();
        let mut r = rec("media", 100_000);
        r.grants = vec!["/home/dev/in".into(), "/home/dev/out".into()];
        r.state = "running".into();
        let p = write_record(&d, &r).unwrap();
        // mode is 0644
        assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o644);
        let got = load_record(&d, "media").unwrap();
        assert_eq!(got, r);
    }

    #[test]
    fn write_replaces_and_remove_is_idempotent() {
        let d = tmpdir();
        write_record(&d, &rec("b", 100_000)).unwrap();
        let mut r2 = rec("b", 100_000);
        r2.state = "stopped".into();
        write_record(&d, &r2).unwrap();
        assert_eq!(load_record(&d, "b").unwrap().state, "stopped");
        remove_record(&d, "b").unwrap();
        assert!(load_record(&d, "b").is_none());
        remove_record(&d, "b").unwrap(); // idempotent
    }

    #[test]
    fn malformed_records_fail_closed() {
        let d = tmpdir();
        let p = d.join("bad");
        for bad in [
            "garbage",
            "SHREK-BENCH 2\nname bad\nid bad\nproject 1\nquota_kib 0\ncreated 0\nstate created\nEND\n",
            "SHREK-BENCH 1\nname bad\nid bad\nproject notnum\nquota_kib 0\ncreated 0\nstate created\nEND\n",
            "SHREK-BENCH 1\nname bad\nid bad\nproject 1\nquota_kib 0\ncreated 0\nstate created\n", // no END
            "SHREK-BENCH 1\nid bad\nproject 1\nquota_kib 0\ncreated 0\nstate created\nEND\n",      // no name
            "SHREK-BENCH 1\nname bad\nid bad\nproject 1\nquota_kib 0\ncreated 0\nstate created\nbogus x\nEND\n",
        ] {
            fs::write(&p, bad).unwrap();
            assert!(load_record(&d, "bad").is_none(), "should reject: {bad:?}");
        }
    }

    #[test]
    fn list_skips_temp_and_corrupt_records() {
        let d = tmpdir();
        write_record(&d, &rec("a", 100_000)).unwrap();
        write_record(&d, &rec("c", 100_001)).unwrap();
        fs::write(d.join(".a.tmp"), "half").unwrap(); // temp: skipped
        fs::write(d.join("z"), "corrupt").unwrap(); // corrupt: skipped
        let names: Vec<String> = list_records(&d).into_iter().map(|r| r.name).collect();
        assert_eq!(names, vec!["a".to_string(), "c".to_string()]);
    }

    #[test]
    fn next_project_id_starts_at_base_and_skips_used() {
        assert_eq!(next_project_id(&[]), PROJECT_ID_BASE);
        let rs = vec![rec("a", PROJECT_ID_BASE), rec("b", PROJECT_ID_BASE + 1), rec("c", PROJECT_ID_BASE + 3)];
        assert_eq!(next_project_id(&rs), PROJECT_ID_BASE + 2);
        // an unset (0) project doesn't consume the base slot.
        assert_eq!(next_project_id(&[rec("x", 0)]), PROJECT_ID_BASE);
    }

    #[test]
    fn write_rejects_bad_name_and_newline_grant() {
        let d = tmpdir();
        assert!(write_record(&d, &rec("../escape", 1)).is_err());
        let mut r = rec("ok", 1);
        r.grants = vec!["/home/dev/a\nb".into()];
        assert!(write_record(&d, &r).is_err());
    }
}
