//! supervisor — the root desktop-egress daemon (ADR-007 S2d).
//!
//! A long-running root service that listens on a unix socket for the uid-1000 desktop session's
//! bless/unbless/re-pin requests and drives the S2a-c machinery (store -> DoT resolve -> element-only
//! apply). The boundary is deliberately narrow and is the whole point of this module:
//!
//!   * SO_PEERCRED is IDENTITY, NOT AUTHORIZATION ([`authorize`]). A request is served only if the peer
//!     is the sealed desktop uid, AND the profile passes the sealed Tier-B rule
//!     (`admits_socket_bless`). uid 1000 proving it is uid 1000 grants nothing on its own.
//!   * The wire carries a VERB + a PROFILE TOKEN, nothing else ([`parse_request`]). Never a hostname,
//!     IP, resolver, nft set name, path, or free parameter from uid 1000 — a third field is a hard
//!     parse error. `repin` means "re-resolve this already-known sealed profile", the name looked up
//!     internally, never supplied.
//!   * Every mutating attempt — including ones that will be DENIED — consumes a rate-limit token
//!     ([`RateLimiter`]), so hammering the socket is neither a useful oracle nor a resource attack.
//!   * Requests are journaled (pid/uid/verb/profile/decision/resolver/result) and mirrored to a
//!     DOWNSTREAM-ONLY notification projection ([`append_event`]): the root daemon RECORDS the event; a
//!     uid-1000 desktop consumer (S3) reads it. The daemon never invokes a desktop/session command.
//!   * Death or a failed request leaves the S1-baked nft floor UNTOUCHED — the applier is element-only
//!     and never flushes. On restart [`reconcile`] re-adds the blessed pins as elements into the
//!     existing named sets; it never recreates or flushes the table.
//!
//! Correctness is independent of `resolved`/NM/NSS (S2c proves it); the shipped hardened `resolved.conf`
//! is defense-in-depth, not a dependency.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::Ipv4Addr;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{chown, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use shrek_policy::desktop_egress::admits_socket_bless;

use crate::apply::{self, NftExec, ShellNft};
use crate::dot;
use crate::store::{self, BlessRecord, FaultKind, PinRecord};
use crate::uapi;

/// Max bytes read for one request before giving up (giant-payload guard). One short line — a verb + a
/// ≤64-char profile token + a newline fits comfortably.
pub const REQ_MAX: usize = 128;
/// Sealed desktop session uid the socket serves. Overridable ONLY in the oracle build (the host oracle
/// runs as the invoking user, not 1000) via `SHREK_EGRESS_DESKTOP_UID`.
pub const DESKTOP_UID: u32 = 1000;
/// Rate-limit: at most this many MUTATING attempts (bless/unbless/repin — including rejected) per
/// window. A desktop blesses a profile once; this is generous for real use, hostile to a hammering loop.
pub const RATE_MAX: usize = 6;
pub const RATE_WINDOW: Duration = Duration::from_secs(30);
/// How long a connected client may dawdle before sending a full request line (holds the single-threaded
/// accept loop otherwise).
pub const READ_TIMEOUT: Duration = Duration::from_secs(5);

pub fn desktop_uid() -> u32 {
    #[cfg(feature = "oracle-env")]
    if let Ok(v) = std::env::var("SHREK_EGRESS_DESKTOP_UID") {
        if let Ok(u) = v.parse() {
            return u;
        }
    }
    DESKTOP_UID
}

pub fn socket_path(run: &Path) -> PathBuf {
    #[cfg(feature = "oracle-env")]
    if let Ok(p) = std::env::var("SHREK_EGRESS_SOCK") {
        if !p.is_empty() {
            return p.into();
        }
    }
    run.join("sock")
}

// ---- protocol -----------------------------------------------------------------------------------

/// A parsed request. The ONLY things uid 1000 can express: a verb and (for the mutating verbs) a
/// profile token. No destination, no parameter — by construction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Status,
    Bless(String),
    Unbless(String),
    Repin(String),
    /// Actuate an ALREADY-ceremony-blessed `web-browsing` record at browser launch (MF-7): install the
    /// cgroup rule pair now that `shrekbrowser.slice` exists. Grants NOTHING not already root-blessed
    /// via the console ceremony — it only makes a persisted broad bless live — so it is tier-safe on the
    /// uid-1000 socket (identity-gated, but authority already rests on the prior ceremony). Wire = verb
    /// only (no cgroup path: the supervisor computes the deterministic slice path from the desktop uid).
    BrowserUp,
}

impl Request {
    pub fn is_mutating(&self) -> bool {
        !matches!(self, Request::Status)
    }
    pub fn verb(&self) -> &'static str {
        match self {
            Request::Status => "status",
            Request::Bless(_) => "bless",
            Request::Unbless(_) => "unbless",
            Request::Repin(_) => "repin",
            Request::BrowserUp => "browser-up",
        }
    }
    pub fn profile(&self) -> &str {
        match self {
            Request::Status | Request::BrowserUp => "-",
            Request::Bless(p) | Request::Unbless(p) | Request::Repin(p) => p,
        }
    }
}

/// Parse one request line, fail-closed. Accepts EXACTLY `status`, or `<verb> <profile>` for the three
/// mutating verbs — a third token, an unknown verb, a control char, or a non-`valid_token` profile is a
/// hard error. This is where "verb + profile only, never a supplied destination" is enforced.
pub fn parse_request(raw: &str) -> Result<Request, &'static str> {
    if raw.len() > REQ_MAX {
        return Err("request too large");
    }
    let line = raw.trim_end_matches(['\r', '\n']);
    if line.bytes().any(|b| b.is_ascii_control()) {
        return Err("control character in request");
    }
    let mut tok = line.split_whitespace();
    let verb = tok.next().ok_or("empty request")?;
    let a = tok.next();
    let b = tok.next();
    if b.is_some() {
        return Err("too many fields"); // never accept a 3rd field (a smuggled destination/param)
    }
    let want_profile = |p: Option<&str>| -> Result<String, &'static str> {
        let p = p.ok_or("verb needs a profile")?;
        if !store::valid_token(p) {
            return Err("invalid profile token");
        }
        Ok(p.to_string())
    };
    match verb {
        "status" => {
            if a.is_some() {
                return Err("status takes no argument");
            }
            Ok(Request::Status)
        }
        "bless" => Ok(Request::Bless(want_profile(a)?)),
        "unbless" => Ok(Request::Unbless(want_profile(a)?)),
        "repin" => Ok(Request::Repin(want_profile(a)?)),
        "browser-up" => {
            if a.is_some() {
                return Err("browser-up takes no argument");
            }
            Ok(Request::BrowserUp)
        }
        _ => Err("unknown verb"),
    }
}

/// Read one bounded, newline-terminated request from a stream. Fail-closed on EOF-before-newline
/// (disconnect), on exceeding [`REQ_MAX`] without a newline (giant payload), or on a read error/timeout.
/// Generic over `Read` so the abuse cases are unit-tested without a socket.
pub fn read_request<R: Read>(mut r: R) -> Result<String, &'static str> {
    let mut buf = [0u8; REQ_MAX + 1];
    let mut n = 0usize;
    loop {
        if n >= buf.len() {
            return Err("request too large");
        }
        match r.read(&mut buf[n..]) {
            Ok(0) => return Err("disconnected before newline"),
            Ok(k) => {
                if let Some(rel) = buf[n..n + k].iter().position(|&b| b == b'\n') {
                    let end = n + rel;
                    return std::str::from_utf8(&buf[..end])
                        .map(|s| s.to_string())
                        .map_err(|_| "non-utf8 request");
                }
                n += k;
                if n > REQ_MAX {
                    return Err("request too large");
                }
            }
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err("read error"),
        }
    }
}

// ---- authorization ------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny(&'static str),
}

/// The two-gate authorization. Gate 1: the peer must be the sealed desktop uid (identity). Gate 2: the
/// verb's profile must pass the sealed Tier-B rule (authority). Passing gate 1 alone authorizes nothing.
pub fn authorize(peer_uid: u32, req: &Request, desktop_uid: u32) -> Decision {
    if peer_uid != desktop_uid {
        return Decision::Deny("peer is not the desktop user");
    }
    match req {
        Request::Status => Decision::Allow,
        // BrowserUp actuates an existing root-blessed record; it cannot manufacture authority (the
        // handler no-ops unless web-browsing is already ceremony-blessed), so identity is the only gate.
        Request::BrowserUp => Decision::Allow,
        Request::Bless(p) | Request::Repin(p) | Request::Unbless(p) => {
            if admits_socket_bless(p) {
                Decision::Allow
            } else {
                Decision::Deny("profile not one-click blessable (baseline/broad/unknown)")
            }
        }
    }
}

// ---- rate limiting ------------------------------------------------------------------------------

/// A monotonic sliding-window limiter. Every mutating ATTEMPT calls [`check`], including ones that go on
/// to be denied — so a flood of denied requests still exhausts the window and cannot become an oracle.
pub struct RateLimiter {
    window: Duration,
    max: usize,
    hits: VecDeque<Instant>,
}

impl RateLimiter {
    pub fn new(window: Duration, max: usize) -> Self {
        RateLimiter { window, max, hits: VecDeque::new() }
    }
    /// Record an attempt at `now`; return whether it is within the limit. Prunes expired hits first.
    pub fn check(&mut self, now: Instant) -> bool {
        while let Some(&front) = self.hits.front() {
            if now.duration_since(front) > self.window {
                self.hits.pop_front();
            } else {
                break;
            }
        }
        if self.hits.len() >= self.max {
            return false;
        }
        self.hits.push_back(now);
        true
    }
}

// ---- journal + notification ---------------------------------------------------------------------

/// One structured audit line to stderr (systemd routes it to the journal). Everything the owner asked
/// to see: requester pid/uid, verb, profile, decision, resolver used, result/failure reason.
fn journal(cred: &uapi::Ucred, verb: &str, profile: &str, decision: &str, resolver: &str, result: &str) {
    eprintln!(
        "egressd[req]: pid={} uid={} verb={} profile={} decision={} resolver={} result={}",
        cred.pid, cred.uid, verb, profile, decision, resolver, result
    );
}

/// Append an event to the DOWNSTREAM-ONLY notification projection (`<run>/events`, root:root 0644 in a
/// 0755 dir). A uid-1000 desktop consumer (S3) reads this and raises the notification — the root daemon
/// never invokes a desktop command. Kept bounded (last `EVENT_KEEP` lines) so /run cannot grow without
/// limit under repeated activity.
const EVENT_KEEP: usize = 50;
pub fn append_event(run: &Path, at: u64, verb: &str, profile: &str, result: &str) -> io::Result<()> {
    std::fs::create_dir_all(run)?;
    let _ = std::fs::set_permissions(run, std::fs::Permissions::from_mode(0o755));
    let _ = chown(run, Some(0), Some(0));
    let path = run.join("events");
    let mut lines: Vec<String> = std::fs::read_to_string(&path)
        .ok()
        .map(|s| s.lines().map(|l| l.to_string()).collect())
        .unwrap_or_default();
    // result is a single controlled token set; still guard the line against newline injection.
    let safe: String = result.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    lines.push(format!("{at} {verb} {profile} {safe}"));
    let start = lines.len().saturating_sub(EVENT_KEEP);
    let body = lines[start..].join("\n") + "\n";
    let tmp = run.join(".events.tmp");
    std::fs::write(&tmp, body.as_bytes())?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644))?;
    let _ = chown(&tmp, Some(0), Some(0));
    std::fs::rename(&tmp, &path)
}

// ---- resolution seam ----------------------------------------------------------------------------

/// The DoT-resolution seam, mirroring [`NftExec`]: the real supervisor resolves a profile's SEALED
/// name(s) over the baked sealed-DoT client, but bless/reconcile take this trait so the intent-first
/// ordering and the boot re-resolve retry are unit-testable WITHOUT a live network (a test double
/// returns canned pins or a failure). Correctness of the boundary does not depend on the transport;
/// only the real impl does.
pub trait PinResolver {
    /// Resolve `profile`'s own sealed hosts. Returns `(pins, resolver-label)` — the label is the
    /// resolver IP used, for the journal (`"-"` if unknown). `Err` is a resolution failure (fail-closed:
    /// the caller parks a `resolve-fail` fault and keeps the deny floor).
    fn resolve(&mut self, profile: &str) -> Result<(Vec<store::Pin>, String), String>;
}

/// The production resolver: the sealed DoT client (S2c). Never consults `resolved`/NM/`resolv.conf`/
/// `getaddrinfo` — the `[R1-MF1]`+`[R2-MF-C]` bypass of uid-1000's name-resolution authority.
pub struct DotResolver;
impl PinResolver for DotResolver {
    fn resolve(&mut self, profile: &str) -> Result<(Vec<store::Pin>, String), String> {
        // Fixed query id (see dot:: docs — over authenticated single-query TLS the transport is the
        // security, not the id). Short timeout so a hung resolver fails closed.
        match dot::resolve_profile_pins_logged(profile, 0x7e57, Duration::from_secs(5)) {
            Ok((pins, resolver)) => {
                Ok((pins, resolver.map(|r| r.to_string()).unwrap_or_else(|| "-".into())))
            }
            Err(e) => Err(e.to_string()),
        }
    }
}

// ---- the supervisor -----------------------------------------------------------------------------

/// Handler state: the store + run dirs, the sealed desktop uid, and the rate limiter (mutating-attempt
/// budget). One instance drives the whole accept loop.
pub struct Supervisor {
    pub store: PathBuf,
    pub run: PathBuf,
    pub desktop_uid: u32,
    pub limiter: RateLimiter,
}

impl Supervisor {
    pub fn new(store: PathBuf, run: PathBuf, desktop_uid: u32) -> Self {
        Supervisor {
            store,
            run,
            desktop_uid,
            limiter: RateLimiter::new(RATE_WINDOW, RATE_MAX),
        }
    }

    /// Handle one raw request line and return the response line. `now`/`at` are injected (monotonic +
    /// wall) so the accept loop supplies the clock and tests stay deterministic. `exec` is the nft
    /// executor (real `nft` in the daemon; a double in the oracle isn't used — end-to-end runs live).
    pub fn handle(
        &mut self,
        cred: &uapi::Ucred,
        raw: &str,
        exec: &mut dyn NftExec,
        resolver: &mut dyn PinResolver,
        now: Instant,
        at: u64,
    ) -> String {
        let req = match parse_request(raw) {
            Ok(r) => r,
            Err(e) => {
                journal(cred, "?", "-", "parse-error", "-", e);
                return format!("ERR bad-request: {e}");
            }
        };

        // Rate-limit every mutating attempt BEFORE authorization, so a denied-profile flood still burns
        // the budget (no oracle). Status is read-only and not rate-limited here.
        if req.is_mutating() && !self.limiter.check(now) {
            journal(cred, req.verb(), req.profile(), "rate-limited", "-", "over limit");
            return "ERR rate-limited".to_string();
        }

        match authorize(cred.uid, &req, self.desktop_uid) {
            Decision::Deny(reason) => {
                journal(cred, req.verb(), req.profile(), "deny", "-", reason);
                format!("ERR denied: {reason}")
            }
            Decision::Allow => self.execute(cred, &req, exec, resolver, at),
        }
    }

    fn execute(
        &mut self,
        cred: &uapi::Ucred,
        req: &Request,
        exec: &mut dyn NftExec,
        resolver: &mut dyn PinResolver,
        at: u64,
    ) -> String {
        // MF-4: serialize with the transient root `confirmed-*` process for every mutating op — their
        // store writes / nft reconciles / `/run` projections must never interleave. Best-effort (a lock
        // failure on a single-user appliance still proceeds), but both writers take it, so they queue.
        let _lock = if req.is_mutating() { store::lock_store(&self.store).ok() } else { None };
        match req {
            Request::Status => {
                let b = store::list_bless(&self.store).len();
                let p = store::list_pins(&self.store).len();
                journal(cred, "status", "-", "accept", "-", &format!("blessed={b} pinned={p}"));
                format!("OK status blessed={b} pinned={p}")
            }
            Request::Bless(p) => self.bless(cred, p, exec, resolver, at, false),
            Request::Repin(p) => {
                if store::load_bless(&self.store, p).is_none() {
                    journal(cred, "repin", p, "deny", "-", "not blessed");
                    return "ERR not-blessed".to_string();
                }
                self.bless(cred, p, exec, resolver, at, true)
            }
            Request::Unbless(p) => self.unbless(cred, p, exec, at),
            Request::BrowserUp => self.browser_up(cred, exec, at),
        }
    }

    /// MF-7: actuate an ALREADY-ceremony-blessed `web-browsing` record at browser launch. Installs the
    /// cgroup rule pair IFF web-browsing is blessed AND the slice now exists AND the rules aren't already
    /// present — otherwise a legible no-op (never manufactures authority; never a fault for "not blessed"
    /// or "slice not up"). The browser launch wrapper calls this after `systemd-run --slice`.
    fn browser_up(&mut self, cred: &uapi::Ucred, exec: &mut dyn NftExec, at: u64) -> String {
        if store::load_bless(&self.store, "web-browsing").is_none() {
            journal(cred, "browser-up", "web-browsing", "noop", "-", "not blessed");
            return "OK browser-up not-blessed".to_string();
        }
        match crate::confirmed::reconcile_web_browsing(&self.store, exec, self.desktop_uid) {
            Ok(true) => {
                let _ = store::project_state(&self.store, &self.run);
                journal(cred, "browser-up", "web-browsing", "accept", "-", "rules installed");
                let _ = append_event(&self.run, at, "browser-up", "web-browsing", "live");
                "OK browser-up live".to_string()
            }
            Ok(false) => {
                journal(cred, "browser-up", "web-browsing", "noop", "-", "slice absent or already live");
                "OK browser-up pending".to_string()
            }
            Err(e) => {
                let msg = format!("{e:?}");
                journal(cred, "browser-up", "web-browsing", "apply-fail", "-", &msg);
                "ERR apply-failed".to_string()
            }
        }
    }

    /// Refresh BOTH `/run` projections after a store change (curated pinned map + legible state view),
    /// so the uid-1000 UI never reads a stale panel and the weather widget's `--resolve` map stays in
    /// step. Best-effort: a projection failure never fails the bless it reports.
    fn reproject(&self) {
        let _ = store::project_pinned(&self.store, &self.run);
        let _ = store::project_state(&self.store, &self.run);
    }

    /// Shared bless / re-pin body. INTENT-FIRST `[Fable S3 fix #4]`: record the durable bless BEFORE
    /// resolving, so a resolve/apply failure (typically a first-run bless before the clock/network is up)
    /// leaves the profile legibly "blessed, pin deferred" — boot [`reconcile`] re-resolves it — instead
    /// of a silently-unblessed profile that never retries. Then DoT-resolve the profile's SEALED name(s)
    /// (never a supplied name), store the pins, apply element-only. Fail-closed at each step with a
    /// parked fault + a refreshed state projection (so the panel shows the pending/fault state at once).
    fn bless(
        &mut self,
        cred: &uapi::Ucred,
        profile: &str,
        exec: &mut dyn NftExec,
        resolver: &mut dyn PinResolver,
        at: u64,
        repin: bool,
    ) -> String {
        let verb = if repin { "repin" } else { "bless" };
        // 0. durable intent FIRST. `repin` only reaches here for an already-blessed profile, so the
        //    rewrite is a no-op-shaped refresh; a fresh bless persists even if the resolve below fails.
        let _ = store::write_bless(
            &self.store,
            &BlessRecord { profile: profile.to_string(), tier: "one-click".into(), blessed: at },
        );
        // 1. resolve over the sealed-DoT seam (the profile's own sealed hosts — looked up internally).
        let (pins, rip) = match resolver.resolve(profile) {
            Ok(r) => r,
            Err(e) => {
                let _ = store::write_fault(&self.store, profile, FaultKind::ResolveFail, &e, at);
                self.reproject(); // panel now shows blessed=1 pins=- fault=resolve-fail (pending)
                journal(cred, verb, profile, "resolve-fail", "-", &e);
                let _ = append_event(&self.run, at, verb, profile, "resolve-failed");
                return "ERR resolve-failed".to_string();
            }
        };
        // 2. persist the pin record (re-validated against the sealed profile by the store).
        let rec = PinRecord { profile: profile.to_string(), pins: pins.clone(), resolved: at };
        if let Err(e) = store::write_pin(&self.store, &rec) {
            let _ = store::write_fault(&self.store, profile, FaultKind::ResolveFail, &format!("store: {e}"), at);
            self.reproject();
            journal(cred, verb, profile, "store-fail", &rip, &e.to_string());
            return "ERR store".to_string();
        }
        // 3. apply element-only (reconcile @set to the resolved addrs).
        let desired: Vec<Ipv4Addr> = pins.iter().map(|p| p.addr).collect();
        match apply::apply_pins(&self.store, exec, profile, &desired) {
            Ok(a) => {
                let _ = store::clear_fault(&self.store, profile);
                self.reproject();
                let detail = format!("{} ip(s)", a.len());
                journal(cred, verb, profile, "accept", &rip, &detail);
                let _ = append_event(&self.run, at, verb, profile, &detail);
                format!("OK {verb} {profile} {}", a.len())
            }
            Err(e) => {
                let msg = format!("{e:?}");
                let _ = store::write_fault(&self.store, profile, FaultKind::ApplyFail, &msg, at);
                self.reproject();
                journal(cred, verb, profile, "apply-fail", &rip, &msg);
                let _ = append_event(&self.run, at, verb, profile, "apply-failed");
                "ERR apply-failed".to_string()
            }
        }
    }

    fn unbless(&mut self, cred: &uapi::Ucred, profile: &str, exec: &mut dyn NftExec, at: u64) -> String {
        // Reconcile the set to empty FIRST; only clear the durable records if that succeeded, so a
        // delete failure leaves the profile consistently "still blessed, retry-able", never a drift
        // where records say revoked but elements linger.
        match apply::unapply(&self.store, exec, profile) {
            Ok(()) => {
                let _ = store::remove_bless(&self.store, profile);
                let _ = store::remove_pin(&self.store, profile);
                let _ = store::clear_fault(&self.store, profile);
                self.reproject();
                journal(cred, "unbless", profile, "accept", "-", "revoked");
                let _ = append_event(&self.run, at, "unbless", profile, "revoked");
                format!("OK unbless {profile}")
            }
            Err(e) => {
                let msg = format!("{e:?}");
                let _ = store::write_fault(&self.store, profile, FaultKind::ApplyFail, &msg, at);
                self.reproject();
                journal(cred, "unbless", profile, "apply-fail", "-", &msg);
                "ERR apply-failed".to_string()
            }
        }
    }
}

/// Startup reconcile: re-add every blessed profile's stored pins as ELEMENTS into the existing baked
/// sets. Element-only (never flush/recreate), so a daemon restart or reboot restores runtime allows
/// without touching the S1 deny floor. A profile whose baked set is absent (the oneshot didn't run)
/// fails its apply and parks a fault — fail-closed.
///
/// SELF-HEAL `[Fable S3 fix #4]`: a blessed one-click profile with NO stored pin — the intent-first
/// residue of a first-run bless made before the clock/network converged — gets a fresh DoT re-resolve
/// here (the ONLY place a root-side retry belongs; a UI/socket retry would starve the owner's own
/// clicks against the rate limiter). So a weather bless from the sealed onboarding eventually becomes a
/// live allow on a later boot without the user re-discovering the Settings toggle, and stays a legible
/// pending until then. Returns a one-line summary for the journal.
pub fn reconcile(store: &Path, run: &Path, exec: &mut dyn NftExec, resolver: &mut dyn PinResolver, at: u64) -> String {
    let mut ok = 0usize;
    let mut failed = 0usize;
    let mut healed = 0usize;
    for b in store::list_bless(store) {
        if !admits_socket_bless(&b.profile) {
            continue; // only pinnable one-click profiles carry runtime elements
        }
        let mut desired: Vec<Ipv4Addr> = store::load_pin(store, &b.profile)
            .map(|r| r.pins.iter().map(|p| p.addr).collect())
            .unwrap_or_default();
        if desired.is_empty() {
            // Blessed but pin-deferred: try to complete it now that (maybe) the network/clock are up.
            match resolver.resolve(&b.profile) {
                Ok((pins, _)) if !pins.is_empty() => {
                    let rec = PinRecord { profile: b.profile.clone(), pins: pins.clone(), resolved: at };
                    if store::write_pin(store, &rec).is_ok() {
                        desired = pins.iter().map(|p| p.addr).collect();
                        let _ = store::clear_fault(store, &b.profile);
                        healed += 1;
                    }
                }
                _ => {
                    // still unreachable — keep the profile legibly pending, retry next boot.
                    let _ = store::write_fault(store, &b.profile, FaultKind::ResolveFail, "reconcile: deferred", at);
                }
            }
        }
        match apply::apply_pins(store, exec, &b.profile, &desired) {
            Ok(_) => ok += 1,
            Err(e) => {
                let _ = store::write_fault(store, &b.profile, FaultKind::ApplyFail, &format!("reconcile: {e:?}"), at);
                failed += 1;
            }
        }
    }

    // S4: survive the CEREMONY tier across reboot (MF-3/Q4). Raw destinations re-resolve into the
    // `@raw_pinned` union; a blessed `web-browsing` re-installs its cgroup rule IFF the slice exists yet
    // (else it stays legibly pending and heals at browser launch via `browser-up`). Uses the production
    // raw resolver — this is a boot path, not a unit seam (confirmed::reconcile_raw is unit-tested
    // directly with a fake resolver). With no raw entries + no web-browsing bless this is inert (no
    // network, no nft mutation), so it does not perturb the profile-tier reconcile above.
    let mut raw_resolver = crate::confirmed::DotRawResolver;
    let (raw_ok, raw_pending) = match crate::confirmed::reconcile_raw(store, exec, &mut raw_resolver, at) {
        Ok((p, pend)) => (p, pend),
        Err(e) => {
            eprintln!("egressd[boot]: raw reconcile failed: {e:?}");
            (0, 0)
        }
    };
    let browser = match crate::confirmed::reconcile_web_browsing(store, exec, desktop_uid()) {
        Ok(installed) => installed,
        Err(e) => {
            eprintln!("egressd[boot]: web-browsing reconcile failed: {e:?}");
            false
        }
    };

    let _ = store::project_pinned(store, run);
    let _ = store::project_state(store, run);
    let summary = format!(
        "reconcile: {ok} restored, {healed} re-resolved, {failed} faulted; raw {raw_ok} pinned/{raw_pending} pending; browser {}",
        if browser { "installed" } else { "pending/na" }
    );
    eprintln!("egressd[boot]: {summary}");
    summary
}

/// Bind the socket, reconcile, and serve requests forever. The socket lives under the 0755 run dir so
/// uid 1000 can reach it; mode 0660 (root + the desktop group) narrows who can even connect, but
/// SO_PEERCRED is the authoritative identity gate. One connection at a time with a read timeout — a
/// single-user appliance, and a stalled client cannot wedge the loop.
pub fn serve(store: PathBuf, run: PathBuf) -> io::Result<()> {
    store::ensure_store(&store)?;
    std::fs::create_dir_all(&run)?;
    let _ = std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o755));
    let _ = chown(&run, Some(0), Some(0));

    let sock = socket_path(&run);
    let _ = std::fs::remove_file(&sock); // clear a stale socket from a prior run
    let listener = UnixListener::bind(&sock)?;
    std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o660))?;
    let duid = desktop_uid();
    let _ = chown(&sock, Some(0), Some(duid)); // group = desktop user; peercred still authoritative

    // Reconcile stored blesses into the (already baked + loaded) named sets. Never flushes the table.
    let mut boot_exec = ShellNft;
    let mut boot_resolver = DotResolver;
    reconcile(&store, &run, &mut boot_exec, &mut boot_resolver, now_unix());

    let mut sup = Supervisor::new(store, run, duid);
    for conn in listener.incoming() {
        let mut stream = match conn {
            Ok(s) => s,
            Err(_) => continue,
        };
        let cred = match uapi::peer_cred(stream.as_raw_fd()) {
            Ok(c) => c,
            Err(_) => continue, // cannot identify peer ⇒ drop
        };
        let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
        let _ = stream.set_write_timeout(Some(READ_TIMEOUT));
        let resp = match read_request(&mut stream) {
            Ok(line) => {
                let mut exec = ShellNft;
                let mut resolver = DotResolver;
                sup.handle(&cred, &line, &mut exec, &mut resolver, Instant::now(), now_unix())
            }
            Err(e) => {
                journal(&cred, "?", "-", "read-error", "-", e);
                format!("ERR {e}")
            }
        };
        let _ = writeln!(stream, "{resp}");
    }
    Ok(())
}

/// Wall-clock unix seconds for record/notification timestamps. Only the long-running daemon reads the
/// clock (after NTP has set it); the store/policy libraries stay wall-clock-free (caller-provided).
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // ---- parser abuse (first-class, per the owner) ----
    #[test]
    fn parse_accepts_only_verb_and_profile() {
        assert_eq!(parse_request("status"), Ok(Request::Status));
        assert_eq!(parse_request("bless weather"), Ok(Request::Bless("weather".into())));
        assert_eq!(parse_request("unbless weather\n"), Ok(Request::Unbless("weather".into())));
        assert_eq!(parse_request("repin weather"), Ok(Request::Repin("weather".into())));
        // collapse extra whitespace, still two tokens
        assert_eq!(parse_request("bless   weather"), Ok(Request::Bless("weather".into())));
    }

    #[test]
    fn parse_rejects_abuse() {
        // a smuggled third field (destination/param) — the crux of "verb + profile only"
        assert!(parse_request("bless weather evil.example.com").is_err());
        assert!(parse_request("bless weather 6.6.6.6").is_err());
        assert!(parse_request("unknownverb weather").is_err());
        assert!(parse_request("bless").is_err()); // missing profile
        assert!(parse_request("status weather").is_err()); // status takes none
        assert!(parse_request("").is_err());
        assert!(parse_request("bless ../escape").is_err()); // traversal token
        assert!(parse_request("bless we\x00ather").is_err()); // control char
        assert!(parse_request(&format!("bless {}", "x".repeat(200))).is_err()); // oversized + bad token
    }

    #[test]
    fn read_request_bounds_and_disconnect() {
        // normal line
        assert_eq!(read_request(Cursor::new(b"bless weather\n".to_vec())).unwrap(), "bless weather");
        // EOF before newline (mid-request disconnect)
        assert_eq!(read_request(Cursor::new(b"bless weat".to_vec())), Err("disconnected before newline"));
        // giant payload with no newline
        let giant = vec![b'a'; REQ_MAX + 10];
        assert_eq!(read_request(Cursor::new(giant)), Err("request too large"));
        // empty stream
        assert_eq!(read_request(Cursor::new(Vec::new())), Err("disconnected before newline"));
    }

    // ---- authorization: identity gate + sealed tier gate ----
    #[test]
    fn authorize_uid_gate_is_not_authority() {
        let wrong = 1001;
        let good = 1000;
        // wrong uid ⇒ denied even for an otherwise-admissible profile
        assert_eq!(authorize(wrong, &Request::Bless("weather".into()), good), Decision::Deny("peer is not the desktop user"));
        // right uid + Tier-B profile ⇒ allow
        assert_eq!(authorize(good, &Request::Bless("weather".into()), good), Decision::Allow);
        // right uid but a non-Tier-B profile ⇒ still denied (identity != authority)
        assert!(matches!(authorize(good, &Request::Bless("web-browsing".into()), good), Decision::Deny(_)));
        assert!(matches!(authorize(good, &Request::Bless("desktop-ntp".into()), good), Decision::Deny(_)));
        assert!(matches!(authorize(good, &Request::Repin("web-browsing".into()), good), Decision::Deny(_)));
        // status is allowed for the desktop uid, denied for others
        assert_eq!(authorize(good, &Request::Status, good), Decision::Allow);
        assert!(matches!(authorize(wrong, &Request::Status, good), Decision::Deny(_)));
    }

    // ---- rate limiter: rejected attempts count ----
    #[test]
    fn rate_limiter_windows_and_counts_every_attempt() {
        let mut rl = RateLimiter::new(Duration::from_secs(30), 3);
        let t0 = Instant::now();
        assert!(rl.check(t0));
        assert!(rl.check(t0));
        assert!(rl.check(t0));
        assert!(!rl.check(t0)); // 4th within window ⇒ blocked
        // after the window slides, budget frees
        let later = t0 + Duration::from_secs(31);
        assert!(rl.check(later));
    }

    #[test]
    fn handle_rate_limits_even_denied_profiles() {
        // A flood of blesses for a NON-admissible profile must still exhaust the budget (no oracle):
        // the rate check runs before authorization, so denied attempts consume tokens.
        let dir = std::env::temp_dir().join(format!("egressd-sup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        store::ensure_store(&dir).unwrap();
        let mut sup = Supervisor::new(dir.clone(), dir.clone(), 1000);
        let cred = uapi::Ucred { pid: 1, uid: 1000, gid: 1000 };
        let mut exec = NoopExec;
        let t0 = Instant::now();
        let mut denied = 0;
        let mut limited = 0;
        for _ in 0..(RATE_MAX + 3) {
            // web-browsing is denied at the tier gate BEFORE resolve/nft, so both doubles must go untouched.
            let r = sup.handle(&cred, "bless web-browsing", &mut exec, &mut NoopResolver, t0, 0);
            if r.starts_with("ERR denied") {
                denied += 1;
            } else if r == "ERR rate-limited" {
                limited += 1;
            }
        }
        assert_eq!(denied, RATE_MAX, "first RATE_MAX attempts hit the tier deny");
        assert_eq!(limited, 3, "the rest are rate-limited despite never being admitted");
    }

    /// An executor that must never be called (used where the path should reject before touching nft).
    struct NoopExec;
    impl NftExec for NoopExec {
        fn run(&mut self, _cmd: &apply::NftCmd) -> Result<String, String> {
            panic!("nft must not be invoked on a rejected/denied request");
        }
    }

    /// A resolver that must never be called (rejected/denied paths short-circuit before resolution).
    struct NoopResolver;
    impl PinResolver for NoopResolver {
        fn resolve(&mut self, _profile: &str) -> Result<(Vec<store::Pin>, String), String> {
            panic!("resolver must not be invoked on a rejected/denied request");
        }
    }

    /// A recording nft double that always succeeds — lets the bless success/apply path run in tests.
    struct OkExec;
    impl NftExec for OkExec {
        fn run(&mut self, _cmd: &apply::NftCmd) -> Result<String, String> {
            Ok(String::new())
        }
    }

    /// A canned resolver: `Ok(pins)` or `Err(reason)`, deterministic and network-free.
    struct FakeResolver(Result<Vec<store::Pin>, String>);
    impl PinResolver for FakeResolver {
        fn resolve(&mut self, _profile: &str) -> Result<(Vec<store::Pin>, String), String> {
            self.0.clone().map(|pins| (pins, "203.0.113.53".to_string()))
        }
    }

    /// Distinct store + run dirs (the store has its own `pinned/` sub-dir, so the run projection's
    /// `pinned` FILE must live in a separate dir — mirrors the real `/home` store vs `/run` split).
    fn sup_dirs(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let base = std::env::var("CARGO_TARGET_TMPDIR").unwrap_or_else(|_| "/tmp".into());
        let root = std::path::PathBuf::from(base).join(format!("egressd-s3-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&root);
        let store = root.join("store");
        let run = root.join("run");
        store::ensure_store(&store).unwrap();
        (store, run)
    }

    #[test]
    fn bless_is_intent_first_when_resolve_fails() {
        // A resolve failure (first-run before clock/network) must leave the profile BLESSED-but-pending,
        // not silently unblessed: the bless record persists, a resolve-fail fault is parked, and the
        // /run/state view renders blessed=1 pins=- fault=resolve-fail.
        let (store_d, run) = sup_dirs("intent-first");
        let mut sup = Supervisor::new(store_d.clone(), run.clone(), 1000);
        let cred = uapi::Ucred { pid: 1, uid: 1000, gid: 1000 };
        let mut exec = OkExec;
        let mut resolver = FakeResolver(Err("resolver unreachable".into()));

        let r = sup.handle(&cred, "bless weather", &mut exec, &mut resolver, Instant::now(), 10);
        assert_eq!(r, "ERR resolve-failed");
        assert!(store::load_bless(&store_d, "weather").is_some(), "bless intent must persist through resolve failure");
        assert_eq!(store::load_fault(&store_d, "weather").unwrap().kind, FaultKind::ResolveFail);
        let state = std::fs::read_to_string(store::state_map(&run)).unwrap();
        assert!(
            state.contains("profile weather tier=one-click blessed=1 pins=- refreshed=- fault=resolve-fail"),
            "pending state not projected:\n{state}"
        );
    }

    #[test]
    fn bless_success_projects_pins_and_clears_fault() {
        let (store_d, run) = sup_dirs("bless-ok");
        let mut sup = Supervisor::new(store_d.clone(), run.clone(), 1000);
        let cred = uapi::Ucred { pid: 1, uid: 1000, gid: 1000 };
        let mut exec = OkExec;
        let pins = vec![store::Pin { name: "api.open-meteo.com".into(), addr: "5.6.7.8".parse().unwrap() }];
        let mut resolver = FakeResolver(Ok(pins));

        let r = sup.handle(&cred, "bless weather", &mut exec, &mut resolver, Instant::now(), 42);
        assert_eq!(r, "OK bless weather 1");
        assert!(store::load_fault(&store_d, "weather").is_none());
        let state = std::fs::read_to_string(store::state_map(&run)).unwrap();
        assert!(state.contains("profile weather tier=one-click blessed=1 pins=5.6.7.8 refreshed=42 fault=-"), "{state}");
        // the curated pinned map (weather widget --resolve source) is refreshed too.
        assert_eq!(std::fs::read_to_string(store::pinned_map(&run)).unwrap(), "api.open-meteo.com 5.6.7.8\n");
    }

    #[test]
    fn reconcile_reresolves_a_blessed_but_pinless_profile() {
        // The self-heal: a blessed weather with no pin (first-run residue) becomes live once the resolver
        // succeeds at a later boot — without any UI/socket action.
        let (store_d, run) = sup_dirs("reconcile-heal");
        store::write_bless(&store_d, &BlessRecord { profile: "weather".into(), tier: "one-click".into(), blessed: 1 }).unwrap();
        store::write_fault(&store_d, "weather", FaultKind::ResolveFail, "was offline", 1).unwrap();
        assert!(store::load_pin(&store_d, "weather").is_none());

        let mut exec = OkExec;
        let pins = vec![store::Pin { name: "api.open-meteo.com".into(), addr: "9.9.9.9".parse().unwrap() }];
        let mut resolver = FakeResolver(Ok(pins));
        let summary = reconcile(&store_d, &run, &mut exec, &mut resolver, 77);

        assert!(summary.contains("1 re-resolved"), "summary={summary}");
        assert_eq!(store::load_pin(&store_d, "weather").unwrap().pins[0].addr, "9.9.9.9".parse::<Ipv4Addr>().unwrap());
        assert!(store::load_fault(&store_d, "weather").is_none(), "fault cleared once the pin lands");
        let state = std::fs::read_to_string(store::state_map(&run)).unwrap();
        assert!(state.contains("profile weather tier=one-click blessed=1 pins=9.9.9.9 refreshed=77 fault=-"), "{state}");
    }

    #[test]
    fn reconcile_keeps_pinless_profile_pending_when_still_offline() {
        let (store_d, run) = sup_dirs("reconcile-offline");
        store::write_bless(&store_d, &BlessRecord { profile: "weather".into(), tier: "one-click".into(), blessed: 1 }).unwrap();
        let mut exec = OkExec;
        let mut resolver = FakeResolver(Err("still offline".into()));
        let summary = reconcile(&store_d, &run, &mut exec, &mut resolver, 5);

        assert!(summary.contains("0 re-resolved"), "summary={summary}");
        assert!(store::load_bless(&store_d, "weather").is_some(), "still blessed (intent preserved)");
        assert!(store::load_pin(&store_d, "weather").is_none(), "no pin yet");
        assert_eq!(store::load_fault(&store_d, "weather").unwrap().kind, FaultKind::ResolveFail);
    }

    #[test]
    fn append_event_is_bounded_and_world_readable() {
        let run = std::env::temp_dir().join(format!("egressd-ev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&run);
        for i in 0..(EVENT_KEEP + 20) {
            append_event(&run, i as u64, "bless", "weather", "1 ip(s)").unwrap();
        }
        let body = std::fs::read_to_string(run.join("events")).unwrap();
        assert_eq!(body.lines().count(), EVENT_KEEP, "events file is capped");
        let mode = std::fs::metadata(run.join("events")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "the notification projection is uid-1000 readable");
    }
}
