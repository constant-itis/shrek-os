//! store — the durable state model + `/run` projection for the desktop egress supervisor (ADR-007 S2a).
//!
//! The single source of truth for what the deny-by-default desktop session (uid 1000) has been BLESSED
//! to reach, and the resolved IPs currently PINNED for those blesses. Same dep-free line-text discipline
//! as gatekeeperd's [`net_binding`]/[`bench_record`]: a fixed header, one field per line, atomic
//! temp+rename, a fail-closed parser. Two trust properties this module enforces at the storage boundary
//! (defense in depth — the supervisor re-checks too):
//!
//!   * A bless/pin can only name a SEALED profile ([`resolve_desktop`]). An unknown name is refused on
//!     write AND rejected on read, so a tampered file can never vouch for `evil.example` — mirrors the
//!     load-bearing "the request crosses a profile NAME, never a destination" invariant of the sealed
//!     table (desktop_egress.rs §doc).
//!   * A pin's `name` must be one of that profile's OWN sealed rule hosts. So even a pin record for a
//!     real profile cannot smuggle in an off-profile hostname → IP mapping.
//!
//! The `[R2-MF-A]` split: the store lives at `/home/.shrek-system/egress`, `root:root 0700` — unreadable
//! to uid 1000. Only a CURATED view (the flattened name→IP map) is projected to the world-readable
//! `/run/shrek/egress/pinned` (`root:root 0644` in a `0755` dir), which the weather widget reads to dial
//! `curl --resolve <name>:443:<ip>` with TLS hostname verification intact. uid 1000 learns the pinned IP
//! but never the bless store's internals, and cannot write either.
//!
//! S2a scope: state layout + records + projection. NO nft (S2b), NO DoT resolution (S2c), NO socket
//! (S2d) — timestamps are CALLER-PROVIDED (the sealed daemons avoid wall-clock reads, per bench_record).

use shrek_policy::desktop_egress::{
    bless_tier, is_broad_profile, is_prepinned_profile, parse_raw_triple, resolve_desktop, RawTriple,
    DESKTOP_EGRESS_PROFILES,
};
use shrek_policy::egress_capability::{Catalog, Source};
use std::fs;
use std::io::{self, Write};
use std::net::Ipv4Addr;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{chown, PermissionsExt};
use std::path::{Path, PathBuf};

// ---- locations ----------------------------------------------------------------------------------

/// Read a `SHREK_EGRESS_*` path override. Honored ONLY in the `oracle-env` build; the shipped image
/// compiles this to a const `None`, so the sealed defaults below are used unconditionally and no
/// environment can point the root supervisor's store at a uid-1000-writable dir. Mirrors
/// gatekeeperd's `bench_env`.
#[cfg(feature = "oracle-env")]
fn egress_env(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|s| !s.is_empty())
}
#[cfg(not(feature = "oracle-env"))]
fn egress_env(_var: &str) -> Option<String> {
    None
}

/// The durable bless/pin store — on the PERSISTENT `/home` plane so blesses survive reboot (the
/// supervisor re-reads it at boot and re-applies the nft elements). `root:root 0700`: uid 1000 can
/// neither read a bless record nor forge one. Overridable in the oracle build via `SHREK_EGRESS_STORE`.
pub fn store_dir() -> PathBuf {
    egress_env("SHREK_EGRESS_STORE")
        .unwrap_or_else(|| "/home/.shrek-system/egress".to_string())
        .into()
}

/// The VOLATILE `/run` projection dir. `0755` so uid 1000 can traverse to read the `0644` pinned map,
/// but owns nothing here. Overridable in the oracle build via `SHREK_EGRESS_RUN`.
pub fn run_dir() -> PathBuf {
    egress_env("SHREK_EGRESS_RUN")
        .unwrap_or_else(|| "/run/shrek/egress".to_string())
        .into()
}

pub fn blessed_dir(store: &Path) -> PathBuf {
    store.join("blessed")
}
pub fn pinned_dir(store: &Path) -> PathBuf {
    store.join("pinned")
}
/// The advanced raw-destinations file (ADR-007 §4: a flat `host<TAB>proto<TAB>port` TSV). S2 initially
/// created `raw` as a directory; S4 corrects that to the ADR §4 flat file — the whole file IS the set,
/// which is exactly what the union re-pin (MF-5) reads.
pub fn raw_file(store: &Path) -> PathBuf {
    store.join("raw")
}
pub fn applied_dir(store: &Path) -> PathBuf {
    store.join(".applied")
}
pub fn fault_dir(store: &Path) -> PathBuf {
    store.join("fault")
}

/// The world-readable pinned map the weather widget reads (`--resolve` source). `root:root 0644`.
pub fn pinned_map(run: &Path) -> PathBuf {
    run.join("pinned")
}

/// The world-readable per-profile STATE view the DMS Connectivity panel + onboarding read (ADR-007 S3).
/// `root:root 0644` in the `0755` run dir, beside the pinned map.
pub fn state_map(run: &Path) -> PathBuf {
    run.join("state")
}

/// Create the store skeleton: the `root:root 0700` store dir and its five sub-dirs
/// (`blessed raw pinned .applied fault`). Idempotent. Best-effort `root:root` chown / 0700 mode (a
/// non-root test host keeps its own ownership; the mode still asserts 0700). The parent
/// (`/home/.shrek-system`) is created `0700` too so uid 1000 cannot even list the egress dir's siblings.
pub fn ensure_store(store: &Path) -> io::Result<()> {
    if let Some(parent) = store.parent() {
        fs::create_dir_all(parent)?;
        let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        let _ = chown(parent, Some(0), Some(0));
    }
    fs::create_dir_all(store)?;
    let _ = fs::set_permissions(store, fs::Permissions::from_mode(0o700));
    let _ = chown(store, Some(0), Some(0));
    for sub in [blessed_dir(store), pinned_dir(store), applied_dir(store), fault_dir(store)] {
        fs::create_dir_all(&sub)?;
        let _ = fs::set_permissions(&sub, fs::Permissions::from_mode(0o700));
        let _ = chown(&sub, Some(0), Some(0));
    }
    // `raw` is a flat FILE now (ADR §4), not a dir. Migrate an S2-era empty `raw/` dir so a later
    // `add_raw` rename onto the path can't fail with EISDIR (best-effort; only ever empty pre-S4).
    let raw = raw_file(store);
    if raw.is_dir() {
        let _ = fs::remove_dir_all(&raw);
    }
    Ok(())
}

// ---- name / token validation --------------------------------------------------------------------

/// A profile token is the record FILENAME, so it must be a safe single path component: alnum plus
/// `._-`, non-empty, ≤ 64, not `.`/`..`, not a leading `.` (records are not hidden). This is the ONLY
/// thing checked for a fault filename — an `unknown-profile` fault must be recordable for a name that is
/// by definition NOT sealed, so sealed-membership is not required here (only path safety).
pub fn valid_token(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.len() <= 64
        && !name.starts_with('.')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Is `name` a SEALED desktop profile? Bless/pin writes are refused for anything else (fail-closed at
/// the storage boundary, over and above the supervisor's own check).
pub fn is_sealed_profile(name: &str) -> bool {
    resolve_desktop(name).is_some()
}

/// The sealed rule hosts of a profile (the ONLY names a pin for that profile may carry). Empty for a
/// broad/stub profile.
fn sealed_hosts(profile: &str) -> Vec<&'static str> {
    resolve_desktop(profile)
        .map(|p| p.rules.iter().map(|r| r.host).collect())
        .unwrap_or_default()
}

/// Collapse a reason string to a single safe line (records are one-field-per-line; a newline would
/// corrupt the parser). Control chars → spaces, trimmed, capped.
fn one_line(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    out.truncate(200);
    out.trim().to_string()
}

// ---- atomic write -------------------------------------------------------------------------------

/// Write `body` to `dir/leaf` atomically (temp + fsync + rename), `mode`, best-effort `root:root`. The
/// temp carries the target's mode BEFORE the rename so the file is never briefly world-readable.
fn atomic_write(dir: &Path, leaf: &str, body: &str, mode: u32) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join(leaf);
    let tmp = dir.join(format!(".{leaf}.tmp"));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(body.as_bytes())?;
        f.sync_all()?;
    }
    fs::set_permissions(&tmp, fs::Permissions::from_mode(mode))?;
    let _ = chown(&tmp, Some(0), Some(0));
    fs::rename(&tmp, &path)?;
    Ok(path)
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

// ---- bless records ------------------------------------------------------------------------------

/// A durable bless: profile P is granted at tier T. This is the DECLARATIVE intent the supervisor
/// re-applies on boot. `tier` is a free token here (the tier POLICY — which profile takes which
/// ceremony — is the supervisor's, S2d); the store only guarantees the profile is sealed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlessRecord {
    pub profile: String,
    pub tier: String,
    /// unix seconds, caller-provided.
    pub blessed: u64,
}

/// Persist a bless. Refused (fail-closed) for an unsealed or unsafe profile name.
pub fn write_bless(store: &Path, rec: &BlessRecord) -> io::Result<PathBuf> {
    if !valid_token(&rec.profile) || !is_sealed_profile(&rec.profile) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "unsealed/unsafe profile"));
    }
    if !valid_token(&rec.tier) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid tier token"));
    }
    let body = format!(
        "SHREK-EGRESS-BLESS 1\nprofile {}\ntier {}\nblessed {}\nEND\n",
        rec.profile, rec.tier, rec.blessed
    );
    atomic_write(&blessed_dir(store), &rec.profile, &body, 0o600)
}

/// Load the bless for `profile`. Fail-closed: missing/malformed ⇒ `None`. Verifies the record's own
/// `profile` line matches the queried name AND is still sealed (a file cannot vouch for a different or
/// since-removed profile).
pub fn load_bless(store: &Path, profile: &str) -> Option<BlessRecord> {
    if !valid_token(profile) {
        return None;
    }
    let body = fs::read_to_string(blessed_dir(store).join(profile)).ok()?;
    let mut lines = body.lines();
    if lines.next()? != "SHREK-EGRESS-BLESS 1" {
        return None;
    }
    let p = lines.next()?.strip_prefix("profile ")?.to_string();
    let tier = lines.next()?.strip_prefix("tier ")?.to_string();
    let blessed: u64 = lines.next()?.strip_prefix("blessed ")?.parse().ok()?;
    if lines.next()? != "END" {
        return None;
    }
    if p != profile || !is_sealed_profile(&p) || !valid_token(&tier) {
        return None;
    }
    Some(BlessRecord { profile: p, tier, blessed })
}

/// Every valid bless in the store (sorted by profile; malformed entries silently skipped — fail-closed).
pub fn list_bless(store: &Path) -> Vec<BlessRecord> {
    let mut out: Vec<BlessRecord> = read_names(&blessed_dir(store))
        .into_iter()
        .filter_map(|n| load_bless(store, &n))
        .collect();
    out.sort_by(|a, b| a.profile.cmp(&b.profile));
    out
}

pub fn remove_bless(store: &Path, profile: &str) -> io::Result<()> {
    if !valid_token(profile) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid profile token"));
    }
    remove_if_present(&blessed_dir(store).join(profile))
}

// ---- pin records --------------------------------------------------------------------------------

/// One resolved mapping: a sealed profile host name → an IPv4 the supervisor sealed into the nft set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pin {
    pub name: String,
    pub addr: Ipv4Addr,
}

/// The resolved pin set for one profile — what the supervisor added to `@<profile>_pinned` and what the
/// `/run` map projects. IPv4-only (agent-plane parity; the nft sets are `ipv4_addr`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinRecord {
    pub profile: String,
    pub pins: Vec<Pin>,
    /// unix seconds of the resolution, caller-provided.
    pub resolved: u64,
}

/// Persist a pin set. Refused (fail-closed) unless the profile is sealed, PINNABLE (not broad — a broad
/// profile is cgroup-scoped, never pinned), and EVERY pin `name` is one of that profile's OWN sealed
/// rule hosts. So a pin file can never carry an off-profile hostname, even for a real profile.
pub fn write_pin(store: &Path, rec: &PinRecord) -> io::Result<PathBuf> {
    if !valid_token(&rec.profile) || !is_sealed_profile(&rec.profile) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "unsealed/unsafe profile"));
    }
    if is_broad_profile(&rec.profile) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "broad profile is not pinnable"));
    }
    let hosts = sealed_hosts(&rec.profile);
    for pin in &rec.pins {
        if !hosts.contains(&pin.name.as_str()) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "pin name not in sealed profile"));
        }
    }
    let mut body = format!("SHREK-EGRESS-PIN 1\nprofile {}\n", rec.profile);
    for pin in &rec.pins {
        body.push_str(&format!("pin {} {}\n", pin.name, pin.addr));
    }
    body.push_str(&format!("resolved {}\nEND\n", rec.resolved));
    atomic_write(&pinned_dir(store), &rec.profile, &body, 0o600)
}

/// Load the pin set for `profile`. Fail-closed and RE-VALIDATED: the record's `profile` must match the
/// query and be sealed+pinnable, and every `pin` name must (still) be a sealed host of that profile — a
/// tampered file that lists a foreign name is rejected wholesale, not partially trusted.
pub fn load_pin(store: &Path, profile: &str) -> Option<PinRecord> {
    if !valid_token(profile) {
        return None;
    }
    let body = fs::read_to_string(pinned_dir(store).join(profile)).ok()?;
    let mut lines = body.lines();
    if lines.next()? != "SHREK-EGRESS-PIN 1" {
        return None;
    }
    let p = lines.next()?.strip_prefix("profile ")?.to_string();
    if p != profile || !is_sealed_profile(&p) || is_broad_profile(&p) {
        return None;
    }
    let hosts = sealed_hosts(&p);
    let mut pins = Vec::new();
    let mut resolved: Option<u64> = None;
    for line in lines {
        if line == "END" {
            // END must be the last meaningful line; anything after is ignored by `lines` exhaustion.
            return match resolved {
                Some(r) => Some(PinRecord { profile: p, pins, resolved: r }),
                None => None,
            };
        } else if let Some(rest) = line.strip_prefix("pin ") {
            let (name, addr) = rest.split_once(' ')?;
            let addr: Ipv4Addr = addr.parse().ok()?;
            if !hosts.contains(&name) {
                return None; // off-profile name ⇒ reject the whole record
            }
            pins.push(Pin { name: name.to_string(), addr });
        } else if let Some(rest) = line.strip_prefix("resolved ") {
            resolved = Some(rest.parse().ok()?);
        } else {
            return None; // unknown line ⇒ fail closed
        }
    }
    None // no END ⇒ truncated ⇒ fail closed
}

/// Every valid pin set in the store (sorted by profile; malformed skipped).
pub fn list_pins(store: &Path) -> Vec<PinRecord> {
    let mut out: Vec<PinRecord> = read_names(&pinned_dir(store))
        .into_iter()
        .filter_map(|n| load_pin(store, &n))
        .collect();
    out.sort_by(|a, b| a.profile.cmp(&b.profile));
    out
}

pub fn remove_pin(store: &Path, profile: &str) -> io::Result<()> {
    if !valid_token(profile) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid profile token"));
    }
    remove_if_present(&pinned_dir(store).join(profile))
}

// ---- applied marker -----------------------------------------------------------------------------

/// The last set of IPv4s the applier successfully wrote into `@<profile>_pinned`. An AUDIT / boot record
/// — the applier reconciles against the LIVE nft set (the real source of truth), not this marker, so a
/// corrupt/missing marker is harmless (a fresh reconcile still converges). Written only after a clean
/// apply. `blessed`/`pinned` are intent; `.applied/` is "what actually made it into the kernel".
pub fn write_applied(store: &Path, profile: &str, addrs: &[Ipv4Addr]) -> io::Result<PathBuf> {
    if !valid_token(profile) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid profile token"));
    }
    let mut sorted: Vec<Ipv4Addr> = addrs.to_vec();
    sorted.sort();
    sorted.dedup();
    let mut body = format!("SHREK-EGRESS-APPLIED 1\nprofile {profile}\n");
    for a in &sorted {
        body.push_str(&format!("addr {a}\n"));
    }
    body.push_str("END\n");
    atomic_write(&applied_dir(store), profile, &body, 0o600)
}

/// Read the applied marker for `profile`. Missing/malformed ⇒ empty (fail-safe: the applier just
/// re-reconciles). Verifies the inner `profile` matches the filename.
pub fn load_applied(store: &Path, profile: &str) -> Vec<Ipv4Addr> {
    if !valid_token(profile) {
        return Vec::new();
    }
    let Ok(body) = fs::read_to_string(applied_dir(store).join(profile)) else {
        return Vec::new();
    };
    let mut lines = body.lines();
    if lines.next() != Some("SHREK-EGRESS-APPLIED 1") {
        return Vec::new();
    }
    if lines.next().and_then(|l| l.strip_prefix("profile ")) != Some(profile) {
        return Vec::new();
    }
    let mut addrs = Vec::new();
    for line in lines {
        if line == "END" {
            return addrs;
        } else if let Some(rest) = line.strip_prefix("addr ") {
            match rest.parse::<Ipv4Addr>() {
                Ok(a) => addrs.push(a),
                Err(_) => return Vec::new(),
            }
        } else {
            return Vec::new();
        }
    }
    Vec::new() // no END ⇒ fail-safe empty
}

pub fn clear_applied(store: &Path, profile: &str) -> io::Result<()> {
    if !valid_token(profile) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid profile token"));
    }
    remove_if_present(&applied_dir(store).join(profile))
}

// ---- fault records ------------------------------------------------------------------------------

/// Why a bless/pin attempt was parked in `fault/` instead of applied. Kept as an explicit enum so the
/// host-oracle proof can assert the exact fail-closed reason (unknown profile ⇒ no element written).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultKind {
    /// A bless named a profile absent from the sealed table (deny-by-default; no element installed).
    UnknownProfile,
    /// Sealed-DoT resolution of a pinnable name failed (no IP to pin; the baked drop stands).
    ResolveFail,
    /// The element-only nft write failed (the deny-by-default skeleton stays in place, fail-closed).
    ApplyFail,
}

impl FaultKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FaultKind::UnknownProfile => "unknown-profile",
            FaultKind::ResolveFail => "resolve-fail",
            FaultKind::ApplyFail => "apply-fail",
        }
    }
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "unknown-profile" => Some(FaultKind::UnknownProfile),
            "resolve-fail" => Some(FaultKind::ResolveFail),
            "apply-fail" => Some(FaultKind::ApplyFail),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaultRecord {
    pub profile: String,
    pub kind: FaultKind,
    pub reason: String,
    /// unix seconds, caller-provided.
    pub at: u64,
}

/// Record a fault. The profile only needs to be a safe TOKEN (an `unknown-profile` fault is, by
/// definition, for an unsealed name), so sealed-membership is NOT required — but path safety is.
pub fn write_fault(store: &Path, profile: &str, kind: FaultKind, reason: &str, at: u64) -> io::Result<PathBuf> {
    if !valid_token(profile) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid profile token"));
    }
    let body = format!(
        "SHREK-EGRESS-FAULT 1\nprofile {}\nkind {}\nreason {}\nat {}\nEND\n",
        profile,
        kind.as_str(),
        one_line(reason),
        at
    );
    atomic_write(&fault_dir(store), profile, &body, 0o600)
}

pub fn load_fault(store: &Path, profile: &str) -> Option<FaultRecord> {
    if !valid_token(profile) {
        return None;
    }
    let body = fs::read_to_string(fault_dir(store).join(profile)).ok()?;
    let mut lines = body.lines();
    if lines.next()? != "SHREK-EGRESS-FAULT 1" {
        return None;
    }
    let p = lines.next()?.strip_prefix("profile ")?.to_string();
    let kind = FaultKind::from_str(lines.next()?.strip_prefix("kind ")?)?;
    let reason = lines.next()?.strip_prefix("reason ")?.to_string();
    let at: u64 = lines.next()?.strip_prefix("at ")?.parse().ok()?;
    if lines.next()? != "END" || p != profile {
        return None;
    }
    Some(FaultRecord { profile: p, kind, reason, at })
}

pub fn clear_fault(store: &Path, profile: &str) -> io::Result<()> {
    if !valid_token(profile) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid profile token"));
    }
    remove_if_present(&fault_dir(store).join(profile))
}

// ---- raw destinations (S4 advanced ceremony tier; ADR-007 §4 flat TSV) --------------------------

/// Read every raw destination (ADR-007 §4). Fail-closed PER LINE: a line that does not re-parse through
/// the ONE sealed grammar ([`parse_raw_triple`]) is SKIPPED, never widened; the result is de-duplicated
/// and preserves file order. A missing file ⇒ empty. This is the union set the raw re-pin reconciles to
/// (MF-5): removing one entry can never drop another's live element because the whole set is recomputed.
pub fn list_raw(store: &Path) -> Vec<RawTriple> {
    let Ok(body) = fs::read_to_string(raw_file(store)) else {
        return Vec::new();
    };
    let mut out: Vec<RawTriple> = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Stored TSV `host\tproto\tport`; the grammar splits on ':' (host never contains either).
        if let Ok(t) = parse_raw_triple(&line.replace('\t', ":")) {
            if !out.contains(&t) {
                out.push(t);
            }
        }
    }
    out
}

/// Add a raw destination (idempotent). Re-validates through the sealed grammar (NEVER trusts the caller —
/// the triple originated from a uid-1000 socket request even though a ceremony proved intent), then
/// rewrites the whole file atomically. Caller MUST hold [`lock_store`].
pub fn add_raw(store: &Path, t: &RawTriple) -> io::Result<()> {
    if parse_raw_triple(&t.to_wire()).as_ref() != Ok(t) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid raw triple"));
    }
    let mut all = list_raw(store);
    if !all.contains(t) {
        all.push(t.clone());
    }
    write_raw_all(store, &all)
}

/// Remove a raw destination (idempotent). Rewrites the remaining set. Caller MUST hold [`lock_store`].
pub fn remove_raw(store: &Path, t: &RawTriple) -> io::Result<()> {
    let all: Vec<RawTriple> = list_raw(store).into_iter().filter(|x| x != t).collect();
    write_raw_all(store, &all)
}

fn write_raw_all(store: &Path, all: &[RawTriple]) -> io::Result<()> {
    let mut lines: Vec<String> = all.iter().map(|t| t.to_tsv()).collect();
    lines.sort();
    lines.dedup();
    let body = if lines.is_empty() { String::new() } else { format!("{}\n", lines.join("\n")) };
    // atomic_write(dir=store, leaf="raw") ⇒ writes store/raw via store/.raw.tmp (root:root 0600).
    atomic_write(store, "raw", &body, 0o600).map(|_| ())
}

/// A raw destination's resolved cache entry: the IPs currently pinned for one `host:proto:port`. The
/// applier splits the `@raw_pinned` union back per entry into this companion so the state view can show
/// which raw destinations are LIVE (have pins) vs still "blessed, waiting" (intent present, no pins yet).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawPin {
    pub triple: RawTriple,
    pub pins: Vec<Ipv4Addr>,
    /// unix seconds the resolve happened, caller-provided.
    pub resolved: u64,
}

/// The raw resolved-cache companion file (beside `raw`). Lines: `host\tproto\tport\tip,ip,…\tat`.
pub fn raw_pins_file(store: &Path) -> PathBuf {
    store.join("raw-pins")
}

/// Rewrite the raw resolved cache wholesale. Only entries whose triple re-parses AND that carry ≥1 pin
/// are kept (a resolve-failed entry stays OUT, so the state view shows it pending). Caller holds the lock.
pub fn write_raw_pins(store: &Path, entries: &[RawPin]) -> io::Result<()> {
    let mut lines: Vec<String> = Vec::new();
    for e in entries {
        if e.pins.is_empty() || parse_raw_triple(&e.triple.to_wire()).as_ref() != Ok(&e.triple) {
            continue;
        }
        let ips = e.pins.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(",");
        lines.push(format!("{}\t{}\t{}", e.triple.to_tsv(), ips, e.resolved));
    }
    lines.sort();
    lines.dedup();
    let body = if lines.is_empty() { String::new() } else { format!("{}\n", lines.join("\n")) };
    atomic_write(store, "raw-pins", &body, 0o600).map(|_| ())
}

/// Read the raw resolved cache. Fail-closed per line (a malformed line is skipped).
pub fn list_raw_pins(store: &Path) -> Vec<RawPin> {
    let Ok(body) = fs::read_to_string(raw_pins_file(store)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in body.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() != 5 {
            continue;
        }
        let Ok(triple) = parse_raw_triple(&format!("{}:{}:{}", f[0], f[1], f[2])) else {
            continue;
        };
        let pins: Vec<Ipv4Addr> = f[3].split(',').filter_map(|s| s.parse().ok()).collect();
        let Ok(resolved) = f[4].parse::<u64>() else {
            continue;
        };
        if !pins.is_empty() {
            out.push(RawPin { triple, pins, resolved });
        }
    }
    out
}

// ---- store lock (MF-4: serialize the two root writers) ------------------------------------------

/// An exclusive advisory lock over the whole store, released when dropped. The long-running supervisor
/// daemon and each transient root `confirmed-*` process both take it around any mutate+`nft`+project
/// sequence, so their store writes, set reconciles, and `/run` projections can never interleave (torn
/// display truth / lost `@raw_pinned` elements). The lock file is `root:root 0600` inside the `0700`
/// store, so only root can contend for it. `flock` blocks (no `LOCK_NB`) → the second writer waits.
pub struct StoreLock {
    _f: fs::File,
}

pub fn lock_store(store: &Path) -> io::Result<StoreLock> {
    fs::create_dir_all(store)?;
    let path = store.join(".lock");
    let f = fs::OpenOptions::new().create(true).write(true).open(&path)?;
    let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    let _ = chown(&path, Some(0), Some(0));
    crate::uapi::flock(f.as_raw_fd(), crate::uapi::LOCK_EX)?;
    Ok(StoreLock { _f: f })
}

// ---- /run projection ----------------------------------------------------------------------------

/// Project the store's pins into the world-readable `/run/shrek/egress/pinned` map the weather widget
/// reads. Format: one `<name> <ipv4>` per line, deterministically sorted, so a shell widget parses it
/// with `while read name ip`. Only names that survive [`load_pin`]'s sealed-host re-validation appear —
/// the `/run` view can never expose an off-profile mapping. The run dir is `0755` (uid 1000 traverses),
/// the map file `root:root 0644` (uid 1000 reads, never writes). Atomic replace.
pub fn project_pinned(store: &Path, run: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(run)?;
    let _ = fs::set_permissions(run, fs::Permissions::from_mode(0o755));
    let _ = chown(run, Some(0), Some(0));

    let mut rows: Vec<(String, Ipv4Addr)> = Vec::new();
    for rec in list_pins(store) {
        for pin in rec.pins {
            rows.push((pin.name, pin.addr));
        }
    }
    rows.sort();
    rows.dedup();
    let mut body = String::new();
    for (name, addr) in rows {
        body.push_str(&format!("{name} {addr}\n"));
    }
    atomic_write(run, "pinned", &body, 0o644)
}

/// Project a LEGIBLE per-profile state view to `/run/shrek/egress/state` — the read model the DMS
/// Connectivity panel + first-run onboarding render (ADR-007 S3), so the UI never reads the `0700` store
/// and the mutation socket stays write-only (no polling contention). One line per SEALED
/// `DESKTOP_EGRESS_PROFILES` entry, in the policy's declared order (deterministic), each annotated from
/// the store. `root:root 0644` in the `0755` run dir, atomic replace — same `[R2-MF-A]` curated-view
/// discipline as [`project_pinned`]. Call this at EVERY store-mutation site (beside `project_pinned` and
/// after fault writes), so a CLI/timer re-pin never leaves this view stale.
///
/// Line format (schema `shrek-egress-state/1`):
/// ```text
/// profile <name> tier=<baseline|one-click|ceremony> blessed=<0|1> pins=<ip,ip,…|-> refreshed=<unix|-> fault=<kind|-> [source=<sealed|owner> feature=<token>]
/// title <name> <root-authored title>          (rest-of-line; per catalog capability)
/// purpose <name> <root-authored purpose>      (rest-of-line; per catalog capability)
/// capfault <name> source=<sealed|owner> <reason>   (rest-of-line; per faulted catalog entry)
/// ```
/// Deliberately projected: only the CLOSED tier/fault-KIND/source/feature tokens on the `profile` line,
/// never the free-text fault reason (that stays in the journal + `0700` store) — a closed set crossing a
/// parse boundary, fail-closed for the QML reader `[Fable S3 fix #1/#4]`. The ADR-009 additions
/// (`source`/`feature` + the `title`/`purpose`/`capfault` lines) carry the ROOT-AUTHORED capability card
/// text — trustworthy for the panel to render (§4.3) and unreachable to uid 1000 for an OWNER manifest
/// (its `/home` dir is `0700`, so /run is the only delivery). Free text rides its own rest-of-line record
/// so a space in a title/purpose can never desync the space-delimited `profile` parse. A pre-pinned
/// baseline (`desktop-ntp`) surfaces its sealed LITERAL IPs from policy (baked verbatim, never resolved);
/// everything else surfaces its stored pins. `blessed=1` with `pins=-`/`fault=resolve-fail` is the legible
/// "blessed, waiting for network/clock" state that intent-first bless + boot reconcile converge.
///
/// `catalog` is the merged sealed+owner catalog (`crate::catalog::load_catalog`); a compiled profile that
/// is ALSO a catalog capability (`weather`) gets its `source`/`feature` + card text; an OWNER capability
/// not in the compiled table gets a display-only line (blessed=0/pins=- — not one-click-blessable over the
/// socket in S2); a FAULTED owner entry (sealed-name collision) surfaces a legible `capfault` line. Pass
/// an empty [`Catalog`] to project the compiled-only view (no capability annotations).
pub fn project_state(store: &Path, run: &Path, catalog: &Catalog) -> io::Result<PathBuf> {
    fs::create_dir_all(run)?;
    let _ = fs::set_permissions(run, fs::Permissions::from_mode(0o755));
    let _ = chown(run, Some(0), Some(0));

    let src_token = |s: Source| match s {
        Source::Sealed => "sealed",
        Source::Owner => "owner",
    };

    let mut body = String::from("schema shrek-egress-state/1\n");
    // Card text (title/purpose/capfault) rides its own rest-of-line records, appended after the profile
    // block so a free-text value can never desync the space-delimited `profile` parse.
    let mut cardtext: Vec<String> = Vec::new();

    for prof in DESKTOP_EGRESS_PROFILES {
        let name = prof.name;
        // tier is sealed policy (never None here — we iterate the sealed table).
        let tier = bless_tier(name).map(|t| t.as_str()).unwrap_or("unknown");
        let blessed = if load_bless(store, name).is_some() { 1 } else { 0 };

        // Pins + last-refresh: a pre-pinned baseline shows its sealed literal IPs (baked verbatim, no
        // resolve, so nothing in the store); a resolvable profile shows what actually resolved.
        let (pins, refreshed): (Vec<Ipv4Addr>, Option<u64>) = if is_prepinned_profile(name) {
            let ips = prof.rules.iter().filter_map(|r| r.host.parse::<Ipv4Addr>().ok()).collect();
            (ips, None)
        } else {
            match load_pin(store, name) {
                Some(rec) if !rec.pins.is_empty() => {
                    (rec.pins.iter().map(|p| p.addr).collect(), Some(rec.resolved))
                }
                _ => (Vec::new(), None),
            }
        };
        let pins_str = if pins.is_empty() {
            "-".to_string()
        } else {
            pins.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(",")
        };
        let refreshed_str = refreshed.map(|t| t.to_string()).unwrap_or_else(|| "-".to_string());
        let fault_str = load_fault(store, name).map(|f| f.kind.as_str()).unwrap_or("-");

        let mut line = format!(
            "profile {name} tier={tier} blessed={blessed} pins={pins_str} refreshed={refreshed_str} fault={fault_str}"
        );
        // ADR-009: a compiled profile that is ALSO a catalog capability (weather) gains the root-authored
        // source + feature (closed tokens); its title/purpose ride their own lines below.
        if let Some(e) = catalog.get(name) {
            line.push_str(&format!(" source={} feature={}", src_token(e.source), e.manifest.feature));
            cardtext.push(format!("title {name} {}", e.manifest.title));
            cardtext.push(format!("purpose {name} {}", e.manifest.purpose));
        }
        body.push_str(&line);
        body.push('\n');
    }

    // ADR-009: catalog capabilities that are NOT a compiled profile (owner-installed capabilities). In S2
    // these are display-only — not one-click-blessable over the socket yet (that lands with the panel /
    // ceremony slices) — so blessed=0/pins=- always; source/feature are the root-authored card tokens.
    for e in &catalog.entries {
        let name = e.manifest.name.as_str();
        if resolve_desktop(name).is_some() {
            continue; // already emitted by the compiled loop above (weather)
        }
        body.push_str(&format!(
            "profile {name} tier={} blessed=0 pins=- refreshed=- fault=- source={} feature={}\n",
            e.manifest.tier.as_str(),
            src_token(e.source),
            e.manifest.feature
        ));
        cardtext.push(format!("title {name} {}", e.manifest.title));
        cardtext.push(format!("purpose {name} {}", e.manifest.purpose));
    }
    // Faulted catalog entries (an owner name shadowing a sealed one, [R-MF3]) — a legible disabled card,
    // never silently dropped (ADR-009 §4.3). The reason is root-generated (embeds only a valid token), so
    // it is a safe rest-of-line value.
    for e in &catalog.faulted {
        let name = e.manifest.name.as_str();
        let reason = e.fault.as_deref().unwrap_or("disabled");
        cardtext.push(format!("capfault {name} source={} {reason}", src_token(e.source)));
    }
    for l in cardtext {
        body.push_str(&l);
        body.push('\n');
    }

    // Advanced raw-destination tier (S4): one `raw` line per blessed triple, from the intent file joined
    // with the resolved cache. blessed=1 always (a raw entry is intent by construction); pins=- means
    // "blessed, waiting for network" (intent-first before the DoT resolve converged), same legible
    // pending state as a profile. Additive to schema/1 — the S3 reader filters on the `profile ` prefix
    // and ignores these, so no schema bump is needed. Lines are sorted (deterministic view).
    let cache = list_raw_pins(store);
    let mut raw_lines: Vec<String> = Vec::new();
    for t in list_raw(store) {
        let rp = cache.iter().find(|r| r.triple == t);
        let pins_str = match rp {
            Some(r) if !r.pins.is_empty() => {
                r.pins.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(",")
            }
            _ => "-".to_string(),
        };
        let refreshed_str = rp.map(|r| r.resolved.to_string()).unwrap_or_else(|| "-".to_string());
        raw_lines.push(format!(
            "raw host={} proto={} port={} blessed=1 pins={pins_str} refreshed={refreshed_str}",
            t.host,
            t.proto.label(),
            t.port,
        ));
    }
    raw_lines.sort();
    for l in raw_lines {
        body.push_str(&l);
        body.push('\n');
    }
    atomic_write(run, "state", &body, 0o644)
}

// ---- capability request inbox (ADR-009 §6 "Pending needs"; the `want` verb, S2f) ----------------

/// The world-readable capability-request inbox the Network Access panel renders (ADR-009 §2/§6). A
/// uid-1000 process files a request via the `want` socket verb — a CLOSED catalog token, NEVER free text
/// (§2: "a catalog token — never free text") — buying B's discoverability with zero new authority. This
/// bounded, deduped `/run` projection is the daemon's record of it. `root:root 0644`.
pub fn wants_map(run: &Path) -> PathBuf {
    run.join("wants")
}

/// Max distinct pending requests kept (oldest-by-timestamp evicted). Bounds `/run` growth under a
/// hammering uid-1000 process (the socket rate-limiter is the first line; this is the storage backstop).
const WANTS_KEEP: usize = 20;

/// Record a capability request into the bounded inbox (`want <token> <at>` lines: one per token, latest
/// timestamp wins, sorted by token, capped at the [`WANTS_KEEP`] most-recent). `token` is a CLOSED catalog
/// token the caller already validated against the catalog — never free text. Atomic replace, `root:root
/// 0644` in the `0755` run dir. Idempotent per token (re-requesting refreshes its timestamp), so it
/// naturally "ages out" as newer requests displace stale ones (§6).
pub fn record_want(run: &Path, token: &str, at: u64) -> io::Result<PathBuf> {
    if !valid_token(token) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid want token"));
    }
    fs::create_dir_all(run)?;
    let _ = fs::set_permissions(run, fs::Permissions::from_mode(0o755));
    let _ = chown(run, Some(0), Some(0));
    // Read existing (token -> at), drop this token (it will be re-added with the fresh `at`), keep the
    // WANTS_KEEP most-recent by timestamp, then sort by token for a deterministic projection.
    let mut entries: Vec<(String, u64)> = Vec::new();
    if let Ok(body) = fs::read_to_string(wants_map(run)) {
        for line in body.lines() {
            let mut it = line.split_whitespace();
            if let (Some("want"), Some(tok), Some(ts), None) = (it.next(), it.next(), it.next(), it.next()) {
                if let Ok(t) = ts.parse::<u64>() {
                    if valid_token(tok) && tok != token {
                        entries.push((tok.to_string(), t));
                    }
                }
            }
        }
    }
    entries.push((token.to_string(), at));
    entries.sort_by(|a, b| b.1.cmp(&a.1)); // most-recent first
    entries.truncate(WANTS_KEEP);
    entries.sort_by(|a, b| a.0.cmp(&b.0)); // deterministic view
    let body: String = entries.iter().map(|(t, ts)| format!("want {t} {ts}\n")).collect();
    atomic_write(run, "wants", &body, 0o644)
}

// ---- dir listing --------------------------------------------------------------------------------

/// Names of the regular files directly in `dir` (skips the `.<leaf>.tmp` write-temporaries and any
/// sub-dirs). Missing dir ⇒ empty. Used by the `list_*` iterators.
fn read_names(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for ent in rd.flatten() {
            if !ent.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            if let Some(name) = ent.file_name().to_str() {
                if name.starts_with('.') && name.ends_with(".tmp") {
                    continue;
                }
                names.push(name.to_string());
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let base = std::env::var("CARGO_TARGET_TMPDIR").unwrap_or_else(|_| "/tmp".into());
        // Distinct dir per test via the caller's line — process id alone collides across #[test]s that
        // run in the same process. Callers pass a unique tag.
        PathBuf::from(base).join(format!("egress-{}-{}", std::process::id(), _tag()))
    }
    // A cheap per-call unique tag without wall-clock/rng (both banned in the sealed crates): a static
    // atomic counter.
    fn _tag() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        N.fetch_add(1, Ordering::Relaxed)
    }

    fn fresh() -> PathBuf {
        let d = tmp();
        let _ = fs::remove_dir_all(&d);
        ensure_store(&d).unwrap();
        d
    }

    #[test]
    fn ensure_store_lays_out_0700_skeleton() {
        let d = fresh();
        assert_eq!(fs::metadata(&d).unwrap().permissions().mode() & 0o777, 0o700);
        // `raw` is a flat TSV FILE now (ADR §4, S4), not a sub-dir — so the record sub-dirs are these.
        for sub in ["blessed", "pinned", ".applied", "fault"] {
            let p = d.join(sub);
            assert!(p.is_dir(), "missing sub-dir {sub}");
            assert_eq!(fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o700);
        }
        // `raw` must NOT be a directory (an add_raw rename onto it would fail with EISDIR).
        assert!(!raw_file(&d).is_dir(), "raw must be a flat file, not a dir");
    }

    #[test]
    fn ensure_store_migrates_an_s2_era_raw_dir_to_a_file() {
        let d = tmp();
        let _ = fs::remove_dir_all(&d);
        // Simulate the S2-era layout: `raw/` created as a directory.
        fs::create_dir_all(raw_file(&d)).unwrap();
        assert!(raw_file(&d).is_dir());
        ensure_store(&d).unwrap();
        assert!(!raw_file(&d).is_dir(), "S2-era raw/ dir must be migrated away");
        // and add_raw now works (the rename onto the path no longer hits EISDIR).
        let t = shrek_policy::desktop_egress::parse_raw_triple("example.com:tcp:443").unwrap();
        add_raw(&d, &t).unwrap();
        assert_eq!(list_raw(&d), vec![t]);
    }

    #[test]
    fn bless_roundtrips_and_lists() {
        let d = fresh();
        write_bless(&d, &BlessRecord { profile: "weather".into(), tier: "weather".into(), blessed: 100 }).unwrap();
        let got = load_bless(&d, "weather").unwrap();
        assert_eq!(got, BlessRecord { profile: "weather".into(), tier: "weather".into(), blessed: 100 });
        let list = list_bless(&d);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].profile, "weather");
    }

    #[test]
    fn bless_refuses_unsealed_profile() {
        let d = fresh();
        assert!(write_bless(&d, &BlessRecord { profile: "evil".into(), tier: "weather".into(), blessed: 1 }).is_err());
        // ...and a traversal token never becomes a path.
        assert!(write_bless(&d, &BlessRecord { profile: "../escape".into(), tier: "t".into(), blessed: 1 }).is_err());
    }

    // ---- S4 raw destinations (flat TSV) ----------------------------------------------------------

    fn raw(host: &str, proto: &str, port: u16) -> RawTriple {
        shrek_policy::desktop_egress::parse_raw_triple(&format!("{host}:{proto}:{port}")).unwrap()
    }

    #[test]
    fn raw_add_list_remove_roundtrips_and_dedups() {
        let d = fresh();
        let a = raw("example.com", "tcp", 443);
        let b = raw("mqtt.example.org", "udp", 8883);
        add_raw(&d, &a).unwrap();
        add_raw(&d, &b).unwrap();
        add_raw(&d, &a).unwrap(); // idempotent
        let mut got = list_raw(&d);
        got.sort_by_key(|t| t.to_tsv());
        assert_eq!(got, vec![a.clone(), b.clone()]);
        // on-disk shape is the ADR §4 flat TSV, one line per entry.
        let body = std::fs::read_to_string(raw_file(&d)).unwrap();
        assert!(body.contains("example.com\ttcp\t443"), "{body}");
        assert_eq!(body.lines().count(), 2);
        remove_raw(&d, &a).unwrap();
        assert_eq!(list_raw(&d), vec![b]);
    }

    #[test]
    fn list_raw_skips_malformed_lines_fail_closed() {
        let d = fresh();
        // hand-write a mix: one good, one option-injection host, one bad proto, one non-numeric port.
        std::fs::write(
            raw_file(&d),
            "example.com\ttcp\t443\n-evil.com\ttcp\t443\nx.com\tsctp\t443\ny.com\ttcp\tNaN\n",
        )
        .unwrap();
        assert_eq!(list_raw(&d), vec![raw("example.com", "tcp", 443)]);
    }

    #[test]
    fn raw_pins_cache_roundtrips_only_nonempty() {
        let d = fresh();
        let a = raw("example.com", "tcp", 443);
        let entries = vec![
            RawPin { triple: a.clone(), pins: vec![Ipv4Addr::new(203, 0, 113, 7)], resolved: 99 },
            // an empty-pin entry must NOT be persisted (that entry stays "pending" in the view).
            RawPin { triple: raw("pending.com", "tcp", 443), pins: vec![], resolved: 99 },
        ];
        write_raw_pins(&d, &entries).unwrap();
        let got = list_raw_pins(&d);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].triple, a);
        assert_eq!(got[0].pins, vec![Ipv4Addr::new(203, 0, 113, 7)]);
    }

    #[test]
    fn project_state_emits_raw_lines_live_and_pending() {
        let d = fresh();
        let run = d.join("run");
        let live = raw("example.com", "tcp", 443);
        let waiting = raw("later.example.org", "udp", 8883);
        add_raw(&d, &live).unwrap();
        add_raw(&d, &waiting).unwrap();
        // only `live` has a resolved cache entry.
        write_raw_pins(&d, &[RawPin { triple: live.clone(), pins: vec![Ipv4Addr::new(203, 0, 113, 7)], resolved: 77 }]).unwrap();
        project_state(&d, &run, &Catalog::default()).unwrap();
        let state = std::fs::read_to_string(state_map(&run)).unwrap();
        assert!(state.contains("raw host=example.com proto=tcp port=443 blessed=1 pins=203.0.113.7 refreshed=77"), "{state}");
        assert!(state.contains("raw host=later.example.org proto=udp port=8883 blessed=1 pins=- refreshed=-"), "{state}");
        // web-browsing (ceremony tier) still projects via the sealed-table loop, shown-not-toggled.
        assert!(state.contains("profile web-browsing tier=ceremony blessed=0"), "{state}");
    }

    #[test]
    fn bless_record_must_describe_the_queried_profile() {
        let d = fresh();
        // A file whose inner `profile` disagrees with its filename cannot vouch for the lookup name.
        let path = blessed_dir(&d).join("weather");
        fs::write(&path, "SHREK-EGRESS-BLESS 1\nprofile web-browsing\ntier x\nblessed 1\nEND\n").unwrap();
        assert_eq!(load_bless(&d, "weather"), None);
    }

    #[test]
    fn pin_roundtrips_only_with_sealed_host() {
        let d = fresh();
        let rec = PinRecord {
            profile: "weather".into(),
            pins: vec![Pin { name: "api.open-meteo.com".into(), addr: Ipv4Addr::new(1, 2, 3, 4) }],
            resolved: 42,
        };
        write_pin(&d, &rec).unwrap();
        assert_eq!(load_pin(&d, "weather").unwrap(), rec);
    }

    #[test]
    fn pin_refuses_off_profile_name() {
        let d = fresh();
        // `evil.example` is not a sealed host of `weather` — refused on write.
        let bad = PinRecord {
            profile: "weather".into(),
            pins: vec![Pin { name: "evil.example".into(), addr: Ipv4Addr::new(9, 9, 9, 9) }],
            resolved: 1,
        };
        assert!(write_pin(&d, &bad).is_err());
        // ...and a hand-tampered file with a foreign name is rejected on read (whole record).
        let path = pinned_dir(&d).join("weather");
        fs::write(&path, "SHREK-EGRESS-PIN 1\nprofile weather\npin evil.example 9.9.9.9\nresolved 1\nEND\n").unwrap();
        assert_eq!(load_pin(&d, "weather"), None);
    }

    #[test]
    fn pin_refuses_broad_profile() {
        let d = fresh();
        // web-browsing is broad ⇒ cgroup-scoped, never pinned.
        let rec = PinRecord { profile: "web-browsing".into(), pins: vec![], resolved: 1 };
        assert!(write_pin(&d, &rec).is_err());
    }

    #[test]
    fn malformed_pin_fails_closed() {
        let d = fresh();
        let path = pinned_dir(&d).join("weather");
        for bad in [
            "garbage",
            "SHREK-EGRESS-PIN 2\nprofile weather\nresolved 1\nEND\n",              // wrong version
            "SHREK-EGRESS-PIN 1\nprofile weather\npin api.open-meteo.com 1.2.3.4\n", // no END
            "SHREK-EGRESS-PIN 1\nprofile weather\npin api.open-meteo.com notanip\nresolved 1\nEND\n",
            "SHREK-EGRESS-PIN 1\nprofile weather\nbogus line\nresolved 1\nEND\n",
        ] {
            fs::write(&path, bad).unwrap();
            assert_eq!(load_pin(&d, "weather"), None, "should reject: {bad:?}");
        }
    }

    #[test]
    fn projection_reflects_pins_and_is_world_readable() {
        let d = fresh();
        let run = tmp();
        let _ = fs::remove_dir_all(&run);
        write_pin(
            &d,
            &PinRecord {
                profile: "weather".into(),
                pins: vec![Pin { name: "api.open-meteo.com".into(), addr: Ipv4Addr::new(5, 6, 7, 8) }],
                resolved: 1,
            },
        )
        .unwrap();
        let map = project_pinned(&d, &run).unwrap();
        let body = fs::read_to_string(&map).unwrap();
        assert_eq!(body, "api.open-meteo.com 5.6.7.8\n");
        assert_eq!(fs::metadata(&map).unwrap().permissions().mode() & 0o777, 0o644);
        assert_eq!(fs::metadata(&run).unwrap().permissions().mode() & 0o777, 0o755);
        // The store itself stays unreadable to the world (the [R2-MF-A] split).
        assert_eq!(fs::metadata(&d).unwrap().permissions().mode() & 0o777, 0o700);
    }

    #[test]
    fn projection_is_empty_when_no_pins() {
        let d = fresh();
        let run = tmp();
        let _ = fs::remove_dir_all(&run);
        let map = project_pinned(&d, &run).unwrap();
        assert_eq!(fs::read_to_string(&map).unwrap(), "");
    }

    #[test]
    fn state_projection_annotates_every_sealed_profile() {
        let d = fresh();
        let run = tmp();
        let _ = fs::remove_dir_all(&run);
        // weather blessed + pinned (the happy one-click case).
        write_bless(&d, &BlessRecord { profile: "weather".into(), tier: "one-click".into(), blessed: 50 }).unwrap();
        write_pin(&d, &PinRecord {
            profile: "weather".into(),
            pins: vec![Pin { name: "api.open-meteo.com".into(), addr: Ipv4Addr::new(5, 6, 7, 8) }],
            resolved: 99,
        }).unwrap();

        let map = project_state(&d, &run, &Catalog::default()).unwrap();
        let body = fs::read_to_string(&map).unwrap();
        let mut lines = body.lines();
        assert_eq!(lines.next().unwrap(), "schema shrek-egress-state/1");
        // One line per sealed profile, in policy order: ntp, updates, weather, web-browsing.
        assert_eq!(
            lines.next().unwrap(),
            "profile desktop-ntp tier=baseline blessed=0 pins=162.159.200.1,162.159.200.123 refreshed=- fault=-",
            "pre-pinned baseline surfaces its sealed LITERAL IPs from policy"
        );
        assert_eq!(
            lines.next().unwrap(),
            "profile desktop-updates tier=baseline blessed=0 pins=- refreshed=- fault=-",
            "the empty-stub baseline shows no pins"
        );
        assert_eq!(
            lines.next().unwrap(),
            "profile weather tier=one-click blessed=1 pins=5.6.7.8 refreshed=99 fault=-",
            "blessed+pinned weather shows its resolved IP + refresh time"
        );
        assert_eq!(
            lines.next().unwrap(),
            "profile web-browsing tier=ceremony blessed=0 pins=- refreshed=- fault=-",
            "the broad profile is ceremony-tier, unblessed here"
        );
        assert_eq!(lines.next(), None, "exactly one line per sealed profile + the schema header");
        // World-readable view; the store stays 0700.
        assert_eq!(fs::metadata(&map).unwrap().permissions().mode() & 0o777, 0o644);
        assert_eq!(fs::metadata(&d).unwrap().permissions().mode() & 0o777, 0o700);
        // Atomic: no write-temp left behind.
        assert!(!run.join(".state.tmp").exists());
    }

    #[test]
    fn state_projection_shows_blessed_but_pending_when_pin_deferred() {
        // Intent-first bless: a bless record with NO pin (resolve deferred until network/clock) renders
        // "blessed, waiting" — blessed=1, pins=-, fault=resolve-fail — NOT a silently-unblessed profile.
        let d = fresh();
        let run = tmp();
        let _ = fs::remove_dir_all(&run);
        write_bless(&d, &BlessRecord { profile: "weather".into(), tier: "one-click".into(), blessed: 10 }).unwrap();
        write_fault(&d, "weather", FaultKind::ResolveFail, "resolver unreachable", 10).unwrap();

        let body = fs::read_to_string(project_state(&d, &run, &Catalog::default()).unwrap()).unwrap();
        assert!(
            body.contains("profile weather tier=one-click blessed=1 pins=- refreshed=- fault=resolve-fail"),
            "pending weather line missing in:\n{body}"
        );
        // Only the CLOSED fault KIND token crosses the boundary — never the free-text reason.
        assert!(!body.contains("resolver unreachable"), "free-text fault reason must not leak into /run/state");
    }

    #[test]
    fn fault_roundtrips_including_unknown_profile() {
        let d = fresh();
        // An unknown-profile fault is recordable for a name that is NOT sealed (but IS a safe token).
        write_fault(&d, "mysteryprofile", FaultKind::UnknownProfile, "not in sealed table", 7).unwrap();
        let f = load_fault(&d, "mysteryprofile").unwrap();
        assert_eq!(f.kind, FaultKind::UnknownProfile);
        assert_eq!(f.reason, "not in sealed table");
        assert_eq!(f.at, 7);
        clear_fault(&d, "mysteryprofile").unwrap();
        assert_eq!(load_fault(&d, "mysteryprofile"), None);
    }

    #[test]
    fn applied_marker_roundtrips_sorted_and_fails_safe() {
        let d = fresh();
        write_applied(&d, "weather", &[Ipv4Addr::new(9, 9, 9, 9), Ipv4Addr::new(1, 1, 1, 1)]).unwrap();
        assert_eq!(
            load_applied(&d, "weather"),
            vec![Ipv4Addr::new(1, 1, 1, 1), Ipv4Addr::new(9, 9, 9, 9)]
        );
        // Corrupt marker ⇒ empty (the applier just re-reconciles), never a parse panic.
        fs::write(applied_dir(&d).join("weather"), "garbage").unwrap();
        assert_eq!(load_applied(&d, "weather"), Vec::<Ipv4Addr>::new());
        clear_applied(&d, "weather").unwrap();
        assert_eq!(load_applied(&d, "weather"), Vec::<Ipv4Addr>::new());
    }

    #[test]
    fn record_want_dedups_bounds_and_is_world_readable() {
        let run = tmp();
        let _ = fs::remove_dir_all(&run);
        // file three requests; re-request the first with a newer timestamp (dedup, latest wins).
        record_want(&run, "weather", 10).unwrap();
        record_want(&run, "radar", 11).unwrap();
        record_want(&run, "weather", 20).unwrap();
        let body = fs::read_to_string(wants_map(&run)).unwrap();
        // one line per token (deduped), sorted by token, latest timestamp for weather.
        assert_eq!(body, "want radar 11\nwant weather 20\n", "{body}");
        assert_eq!(fs::metadata(wants_map(&run)).unwrap().permissions().mode() & 0o777, 0o644);
        // bounded: file WANTS_KEEP+5 distinct tokens, only the WANTS_KEEP most-recent survive.
        for i in 0..(WANTS_KEEP + 5) {
            record_want(&run, &format!("cap{i}"), 100 + i as u64).unwrap();
        }
        let body = fs::read_to_string(wants_map(&run)).unwrap();
        assert_eq!(body.lines().count(), WANTS_KEEP, "inbox is capped");
        // an invalid token is refused (never a path/free-text).
        assert!(record_want(&run, "../escape", 1).is_err());
        assert!(record_want(&run, "has space", 1).is_err());
    }

    #[test]
    fn fault_reason_is_flattened_to_one_line() {
        let d = fresh();
        write_fault(&d, "weather", FaultKind::ResolveFail, "line one\nEND\nprofile evil", 1).unwrap();
        // The injected newline/END cannot corrupt the record: it is collapsed to spaces on write.
        let f = load_fault(&d, "weather").unwrap();
        assert_eq!(f.kind, FaultKind::ResolveFail);
        assert!(!f.reason.contains('\n'));
    }
}
