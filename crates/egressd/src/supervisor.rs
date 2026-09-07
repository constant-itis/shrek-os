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
use std::os::fd::AsRawFd;
use std::os::unix::fs::{chown, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use shrek_policy::desktop_egress::{admits_socket_bless, bless_tier, is_broad_profile, parse_raw_triple, BlessTier};

use crate::apply::{self, NftExec, ShellNft};
use crate::dot;
use crate::hosts;
use crate::store::{self, BlessRecord, FaultKind, PinRecord};
use crate::uapi;

/// Max bytes read for one request before giving up (giant-payload guard). One short line — a verb + a
/// ≤64-char profile token + a newline fits comfortably. This is the UID-1000 cap; the socket enforces it
/// for every non-root peer, so the untrusted front door stays tight.
pub const REQ_MAX: usize = 128;
/// Max bytes for a ROOT peer's request (the ceremony-commit path). A raw `host:proto:port` wire form is
/// bounded at 300 by [`parse_raw_triple`]; +verb +newline fits under this. Only a root peer (gatekeeperd)
/// is granted the larger line — a uid-1000 peer is still held to [`REQ_MAX`] by [`serve`].
pub const REQ_MAX_PRIV: usize = 384;
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

    // ---- ADR-008: the owner's bounded provider hook-up (uid-1000, identity-gated) ----
    /// Bind a model-provider TOKEN to an IPv4 literal — the owner's `/etc/hosts` hook-up. The ONLY
    /// uid-1000 verb carrying a SECOND field (the address): a deliberate, bounded relaxation of the
    /// "verb + token only" grammar. It is safe because the token is a CLOSED provider set (mapped to a
    /// sealed name server-side, never a free hostname) and the address only sets which IP one of 4 sealed
    /// model names resolves to — it opens NO firewall aperture and touches no name a root daemon looks up
    /// beyond the sandbox's own model endpoint (ADR-008 §3). Both fields are re-validated by shrek-policy.
    Bind(String, String), // (provider-token, canonical IPv4 literal)
    /// Remove a provider binding (idempotent — an unbound provider is a clean `OK`).
    Unbind(String), // (provider-token)

    /// ADR-009 §2/§6: file a capability REQUEST into the inbox — a CLOSED CATALOG TOKEN, never free text.
    /// uid-1000, identity-gated. Grants NOTHING (only makes a pending card appear); the daemon refuses a
    /// token that names no catalog capability, so a spoofed prompt can at most re-surface a root-vetted
    /// name. This buys B's discoverability with zero new authority and zero attacker-authored text.
    Want(String), // (catalog capability token)

    // ---- PRIVILEGED ceremony-commit verbs (ADR-007 S6 fix #4 redesign) ----
    // These are the CONFIRMED result of a console SAK/VT ceremony. They carry the very things the
    // uid-1000 verbs forbid — a broad profile or a raw `host:proto:port` destination — so they are gated
    // on a ROOT peer ([`authorize`]): gatekeeperd, having run the ceremony, connects as root and relays
    // the confirmed op here. The daemon (the sole nft mutator, already `CAP_NET_ADMIN`) performs the
    // store write + apply in-process, so gatekeeperd never needs `CAP_NET_ADMIN` and there is no longer a
    // transient root process editing the ROOT-netns table under the broker's cap umbrella. A uid-1000
    // peer can NEVER reach these — else the socket would become a second front door for the ceremony
    // tier. The engine still re-validates the tier + re-parses the grammar (defense in depth).
    ConfirmedBless(String),     // profile (must be ceremony-tier / broad, e.g. web-browsing)
    ConfirmedUnbless(String),   // profile
    ConfirmedAddRaw(String),    // raw `host:proto:port` wire form (re-validated by the sealed grammar)
    ConfirmedRemoveRaw(String), // raw `host:proto:port` wire form

    /// ADR-009 §4.2: commit / remove an OWNER capability manifest (root peer only — the ceremony ran in
    /// gatekeeperd, which STAGED the confirmed bytes to the volatile staging dir and relays this verb; the
    /// content never rides the wire, only the capability NAME token). egressd is the SOLE writer of the
    /// live owner dir: install re-parses the staged candidate + enforces the §4.4 install-refuses before
    /// committing; remove drops the manifest + withdraws any grant referencing it.
    ConfirmedManifestInstall(String), // capability name (staged candidate read from the staging dir)
    ConfirmedManifestRemove(String),  // capability name
}

impl Request {
    pub fn is_mutating(&self) -> bool {
        !matches!(self, Request::Status)
    }
    /// A privileged ceremony-commit verb — served ONLY to a root peer (gatekeeperd post-ceremony).
    pub fn is_privileged(&self) -> bool {
        matches!(
            self,
            Request::ConfirmedBless(_)
                | Request::ConfirmedUnbless(_)
                | Request::ConfirmedAddRaw(_)
                | Request::ConfirmedRemoveRaw(_)
                | Request::ConfirmedManifestInstall(_)
                | Request::ConfirmedManifestRemove(_)
        )
    }
    /// An ADR-008 hosts verb (`bind`/`unbind`) — mutates the HOSTS store/projection, not the egress
    /// store, so [`Supervisor::execute`] takes the hosts lock in the handler, not the egress store lock.
    pub fn is_hosts(&self) -> bool {
        matches!(self, Request::Bind(..) | Request::Unbind(_))
    }
    pub fn verb(&self) -> &'static str {
        match self {
            Request::Status => "status",
            Request::Bless(_) => "bless",
            Request::Unbless(_) => "unbless",
            Request::Repin(_) => "repin",
            Request::BrowserUp => "browser-up",
            Request::Bind(..) => "bind",
            Request::Unbind(_) => "unbind",
            Request::Want(_) => "want",
            Request::ConfirmedBless(_) => "confirmed-bless",
            Request::ConfirmedUnbless(_) => "confirmed-unbless",
            Request::ConfirmedAddRaw(_) => "confirmed-add-raw",
            Request::ConfirmedRemoveRaw(_) => "confirmed-remove-raw",
            Request::ConfirmedManifestInstall(_) => "confirmed-manifest-install",
            Request::ConfirmedManifestRemove(_) => "confirmed-manifest-remove",
        }
    }
    pub fn profile(&self) -> &str {
        match self {
            Request::Status | Request::BrowserUp => "-",
            Request::Bless(p) | Request::Unbless(p) | Request::Repin(p) => p,
            // the hosts verbs journal under their provider token.
            Request::Bind(t, _) | Request::Unbind(t) => t,
            // the want verb + the manifest verbs journal under their capability-name token.
            Request::Want(t) => t,
            Request::ConfirmedManifestInstall(n) | Request::ConfirmedManifestRemove(n) => n,
            Request::ConfirmedBless(p) | Request::ConfirmedUnbless(p) => p,
            // raw ops journal under a fixed "raw" label, not the destination (which the audit line + the
            // events projection carry separately); keeps the profile column stable.
            Request::ConfirmedAddRaw(_) | Request::ConfirmedRemoveRaw(_) => "raw",
        }
    }
}

/// Parse one request line, fail-closed. Accepts EXACTLY `status`, `<verb> <profile>` for the uid-1000
/// mutating verbs, `browser-up`, or one of the privileged `confirmed-*` verbs (a profile token, or a raw
/// `host:proto:port` re-validated through the ONE sealed grammar). A third whitespace field, an unknown
/// verb, or a control char is a hard error. Parsing is identity-blind — a uid-1000 peer's `confirmed-*`
/// line parses fine here and is then REFUSED at [`authorize`] (peer_uid != 0), so this is not where the
/// tier gate lives. The `REQ_MAX_PRIV` guard is belt-and-suspenders; the real per-peer byte cap is
/// enforced by [`serve`]/[`read_request`] (uid-1000 → [`REQ_MAX`]).
pub fn parse_request(raw: &str) -> Result<Request, &'static str> {
    if raw.len() > REQ_MAX_PRIV {
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
    let c = tok.next();
    // `bind` is the SOLE verb that takes a 3rd whitespace field (the address); ADR-008 [R1-MF8b]. Every
    // OTHER verb still hard-rejects `b` — the "verb + token only" invariant — and `bind` itself rejects a
    // 4th field, so no verb can smuggle an extra destination/parameter.
    if verb != "bind" && b.is_some() {
        return Err("too many fields");
    }
    if verb == "bind" && c.is_some() {
        return Err("too many fields");
    }
    let want_profile = |p: Option<&str>| -> Result<String, &'static str> {
        let p = p.ok_or("verb needs a profile")?;
        if !store::valid_token(p) {
            return Err("invalid profile token");
        }
        Ok(p.to_string())
    };
    // A raw destination is NOT a `valid_token` (it carries `:` and `.`), so it has its own branch: parse
    // it through the sealed grammar and canonicalize to the wire form. A single whitespace-free token, so
    // the "no third field" rule above still holds.
    let want_raw = |p: Option<&str>| -> Result<String, &'static str> {
        let p = p.ok_or("verb needs a destination")?;
        let t = shrek_policy::desktop_egress::parse_raw_triple(p)?;
        Ok(t.to_wire())
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
        // ADR-008 hosts verbs. `bind` carries verb + provider-token + IPv4; the token must be one of the
        // CLOSED provider set (mapped server-side to a sealed name) and the address a strict IPv4 literal,
        // canonicalized here so the store/projection is unambiguous. `unbind` carries verb + token only.
        "bind" => {
            let token = a.ok_or("bind needs a provider token")?;
            let addr = b.ok_or("bind needs an address")?;
            if !store::valid_token(token) || shrek_policy::provider_bind::provider_host(token).is_none() {
                return Err("unknown provider token");
            }
            let ip = shrek_policy::provider_bind::valid_bind_addr(addr)
                .ok_or("address must be an IPv4 literal")?;
            Ok(Request::Bind(token.to_string(), ip.to_string()))
        }
        "unbind" => {
            let token = a.ok_or("unbind needs a provider token")?;
            if !store::valid_token(token) || shrek_policy::provider_bind::provider_host(token).is_none() {
                return Err("unknown provider token");
            }
            Ok(Request::Unbind(token.to_string()))
        }
        // uid-1000 capability request — a single CLOSED catalog token (validated against the catalog in
        // the handler; the parser only enforces the token grammar, mirroring `bless`).
        "want" => Ok(Request::Want(want_profile(a)?)),
        "confirmed-bless" => Ok(Request::ConfirmedBless(want_profile(a)?)),
        "confirmed-unbless" => Ok(Request::ConfirmedUnbless(want_profile(a)?)),
        "confirmed-add-raw" => Ok(Request::ConfirmedAddRaw(want_raw(a)?)),
        "confirmed-remove-raw" => Ok(Request::ConfirmedRemoveRaw(want_raw(a)?)),
        // root-peer owner-manifest commit — a single capability NAME token; the content is read from the
        // staging dir by the handler (never on the wire).
        "confirmed-manifest-install" => Ok(Request::ConfirmedManifestInstall(want_profile(a)?)),
        "confirmed-manifest-remove" => Ok(Request::ConfirmedManifestRemove(want_profile(a)?)),
        _ => Err("unknown verb"),
    }
}

/// Read one bounded, newline-terminated request from a stream. Fail-closed on EOF-before-newline
/// (disconnect), on exceeding `max` bytes without a newline (giant payload), or on a read error/timeout.
/// `max` is the per-peer byte cap the caller chooses from the peer's uid ([`REQ_MAX`] for uid-1000, the
/// larger [`REQ_MAX_PRIV`] for a root ceremony-commit) — the untrusted front door stays tight while the
/// root path admits a raw-triple line. Generic over `Read` so the abuse cases are unit-tested w/o a socket.
pub fn read_request<R: Read>(mut r: R, max: usize) -> Result<String, &'static str> {
    let mut buf = [0u8; REQ_MAX_PRIV + 1];
    let cap = max.min(REQ_MAX_PRIV);
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
                    if end > cap {
                        return Err("request too large");
                    }
                    return std::str::from_utf8(&buf[..end])
                        .map(|s| s.to_string())
                        .map_err(|_| "non-utf8 request");
                }
                n += k;
                if n > cap {
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

/// The identity-then-authority gate. A PRIVILEGED `confirmed-*` verb is served only to a ROOT peer
/// (gatekeeperd, having run the console SAK/VT ceremony); a uid-1000 peer requesting one is refused so
/// the socket can never become a second front door for the ceremony tier (the exact analog of the old
/// CLI's `geteuid()==0` gate). Every OTHER verb requires the sealed desktop uid (identity), and the
/// mutating ones additionally require the profile to pass the sealed Tier-B rule (authority) — passing
/// the identity gate alone authorizes nothing. The engine still re-validates tier/grammar downstream.
pub fn authorize(peer_uid: u32, req: &Request, desktop_uid: u32) -> Decision {
    if req.is_privileged() {
        // ROOT-only. A uid-1000 (or any non-root) peer can NEVER commit a ceremony op over the socket.
        return if peer_uid == 0 {
            Decision::Allow
        } else {
            Decision::Deny("ceremony-commit is root-only (bypasses the console ceremony)")
        };
    }
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
        // ADR-008 hosts verbs: identity is the ONLY gate (no bless tier). The parser already bounded the
        // token to the closed provider set and the address to an IPv4 literal; a provider bind carries no
        // firewall-plane authority (§3), so there is no Tier-B check to make — the closed token IS the
        // authority. The engine re-validates both in the handler (defense in depth).
        Request::Bind(..) | Request::Unbind(_) => Decision::Allow,
        // The `want` inbox verb: identity is the ONLY gate — it grants nothing (the handler refuses a
        // token that names no catalog capability, and even a valid one only surfaces a pending card).
        Request::Want(_) => Decision::Allow,
        // privileged verbs handled above; unreachable but keep the match exhaustive + fail-closed.
        _ => Decision::Deny("unexpected verb"),
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

        // Rate-limit every uid-1000 mutating attempt BEFORE authorization, so a denied-profile flood
        // still burns the budget (no oracle). Status is read-only and not rate-limited. PRIVILEGED
        // ceremony commits are EXEMPT: they are ROOT-only (a uid-1000 peer can never reach them, so they
        // are no oracle/DoS vector) and are already physically throttled by the console SAK/VT ceremony +
        // gatekeeperd's escalating cooldown. Sharing the 6/30s budget with the uid-1000 verbs would
        // SILENTLY DROP a legitimate post-ceremony revoke inside a busy window (MF-5 — "revoked intent,
        // still-live enforcement"): exactly the S6.1 `revoke-drop` regression. Root is trusted here.
        if req.is_mutating() && !req.is_privileged() && !self.limiter.check(now) {
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
        // ADR-008 hosts verbs mutate the HOSTS store, not the egress store — they take the hosts lock in
        // their own handler (shared with the boot `compose-hosts` oneshot), so they are excluded here.
        let _lock = if req.is_mutating() && !req.is_hosts() {
            store::lock_store(&self.store).ok()
        } else {
            None
        };
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
            // ADR-008 hosts verbs (uid-1000 identity-gated). They touch the hosts store + projection only.
            Request::Bind(token, addr) => self.do_bind(cred, token, addr, at),
            Request::Unbind(token) => self.do_unbind(cred, token, at),
            // ADR-009 uid-1000 capability request inbox.
            Request::Want(token) => self.do_want(cred, token, at),
            // ADR-009 root-peer owner-manifest commit (ceremony relay).
            Request::ConfirmedManifestInstall(name) => self.confirmed_manifest_install(cred, name, at),
            Request::ConfirmedManifestRemove(name) => self.confirmed_manifest_remove(cred, name, exec, at),
            // PRIVILEGED ceremony-commit (root peer only — already gated in `authorize`). The daemon does
            // the store write + nft mutation in-process (it holds CAP_NET_ADMIN), so no transient root
            // process runs nft and gatekeeperd needs no net capability.
            Request::ConfirmedBless(p) => self.confirmed_bless(cred, p, exec, at),
            Request::ConfirmedUnbless(p) => self.confirmed_unbless(cred, p, exec, at),
            Request::ConfirmedAddRaw(w) => self.confirmed_add_raw(cred, w, exec, at),
            Request::ConfirmedRemoveRaw(w) => self.confirmed_remove_raw(cred, w, exec, at),
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
                let _ = store::project_state(&self.store, &self.run, &crate::catalog::load_catalog());
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

    // ---- ADR-008 hosts handlers (uid-1000 identity-gated) -------------------------------------------

    /// Bind a provider token → IPv4 in the hosts store and recompose `/run/shrek/hosts`. The parser
    /// already validated the token (closed set) + the address (IPv4 literal); [`hosts::write_binding`]
    /// re-validates (defense in depth). The journal + the downstream event carry the ADDRESS `[R1-MF8c]`.
    /// Takes the HOSTS lock (shared with the base `compose-hosts` oneshot) — NOT the egress store lock.
    fn do_bind(&mut self, cred: &uapi::Ucred, token: &str, addr: &str, at: u64) -> String {
        let home = hosts::hosts_home_dir();
        let run = hosts::hosts_run_dir();
        let _lock = hosts::lock_hosts(&home).ok();
        match hosts::write_binding(&home, token, addr) {
            Ok(ip) => {
                let _ = hosts::compose_hosts(&home, &run, &crate::catalog::load_catalog());
                journal(cred, "bind", token, "accept", "-", &ip.to_string());
                let _ = append_event(&self.run, at, "bind", token, &ip.to_string());
                format!("OK bind {token} {ip}")
            }
            Err(e) => {
                journal(cred, "bind", token, "store-fail", "-", &e.to_string());
                "ERR store".to_string()
            }
        }
    }

    /// Remove a provider binding and recompose. Idempotent — unbinding an unbound provider is a clean OK.
    fn do_unbind(&mut self, cred: &uapi::Ucred, token: &str, at: u64) -> String {
        let home = hosts::hosts_home_dir();
        let run = hosts::hosts_run_dir();
        let _lock = hosts::lock_hosts(&home).ok();
        match hosts::remove_binding(&home, token) {
            Ok(()) => {
                let _ = hosts::compose_hosts(&home, &run, &crate::catalog::load_catalog());
                journal(cred, "unbind", token, "accept", "-", "unbound");
                let _ = append_event(&self.run, at, "unbind", token, "unbound");
                format!("OK unbind {token}")
            }
            Err(e) => {
                journal(cred, "unbind", token, "store-fail", "-", &e.to_string());
                "ERR store".to_string()
            }
        }
    }

    // ---- ADR-009 capability request inbox + owner-manifest ceremony verbs (S2f) ---------------------

    /// File a uid-1000 capability REQUEST (ADR-009 §2/§6). The token must name an EXISTING catalog
    /// capability — a token that names no capability is refused, so uid 1000 can only ever surface a
    /// root-vetted name, never author a destination or free text. Grants NOTHING; it only makes a pending
    /// card appear (discoverability with zero authority). Recorded into the bounded `/run` inbox.
    fn do_want(&mut self, cred: &uapi::Ucred, token: &str, at: u64) -> String {
        let catalog = crate::catalog::load_catalog();
        if catalog.get(token).is_none() {
            journal(cred, "want", token, "deny", "-", "not a catalog capability");
            return "ERR unknown-capability".to_string();
        }
        match store::record_want(&self.run, token, at) {
            Ok(_) => {
                journal(cred, "want", token, "accept", "-", "requested");
                let _ = append_event(&self.run, at, "want", token, "requested");
                format!("OK want {token}")
            }
            Err(e) => {
                journal(cred, "want", token, "store-fail", "-", &e.to_string());
                "ERR store".to_string()
            }
        }
    }

    /// Commit a CONFIRMED owner-manifest install (root peer only — the ceremony ran in gatekeeperd, which
    /// STAGED the confirmed bytes to the volatile staging dir and relayed this verb; only the NAME rides
    /// the wire). egressd is the SOLE writer of the live owner dir: it reads the staged candidate,
    /// RE-PARSES it (the ceremony proved human intent, not well-formedness — the confirmed-* doctrine),
    /// checks the staged name matches, enforces the §4.4 install-refuses (sealed-name collision /
    /// system-reserved host / owner `deliver hosts`), then commits + clears staging + reprojects. No nft
    /// change — an installed owner capability is not one-click-blessable over the socket in S2.
    fn confirmed_manifest_install(&mut self, cred: &uapi::Ucred, name: &str, at: u64) -> String {
        let verb = "confirmed-manifest-install";
        let staging = crate::catalog::staging_cap_dir();
        let owner = crate::catalog::owner_cap_dir();
        let Some(text) = crate::catalog::read_staged(&staging, name) else {
            journal(cred, verb, name, "deny", "-", "no staged candidate");
            return "ERR no-staged-candidate".to_string();
        };
        let m = match shrek_policy::egress_capability::parse_manifest(&text) {
            Ok(m) => m,
            Err(e) => {
                let _ = crate::catalog::clear_staged(&staging, name);
                journal(cred, verb, name, "deny", "-", &e.reason());
                return format!("ERR invalid-manifest: {}", e.reason());
            }
        };
        if m.name != name {
            let _ = crate::catalog::clear_staged(&staging, name);
            journal(cred, verb, name, "deny", "-", "staged name mismatch");
            return "ERR name-mismatch".to_string();
        }
        let sealed = crate::catalog::load_sealed_catalog();
        if let Err(reason) = crate::catalog::validate_owner_install(&m, &sealed) {
            let _ = crate::catalog::clear_staged(&staging, name);
            journal(cred, verb, name, "refuse", "-", &reason);
            return format!("ERR refused: {reason}");
        }
        if let Err(e) = crate::catalog::write_owner_manifest(&owner, name, &text) {
            journal(cred, verb, name, "store-fail", "-", &e.to_string());
            return "ERR store".to_string();
        }
        let _ = crate::catalog::clear_staged(&staging, name);
        self.reproject(); // the state view now carries the new capability card (source=owner)
        journal(cred, verb, name, "accept", "-", "installed");
        let _ = append_event(&self.run, at, "manifest-install", name, "installed");
        format!("OK confirmed-manifest-install {name}")
    }

    /// Commit a CONFIRMED owner-manifest removal (root peer only). Removes the live owner manifest and —
    /// defensively — withdraws any grant that referenced it, then reconciles the `@cap_pinned` union so
    /// its tuples (if any) are gone and the hosts bridge drops its lines at the next compose (ADR-009
    /// §4.6). An owner capability is not one-click-blessable over the socket in S2 so there is normally no
    /// grant, but a future granted capability MUST tear down here. Idempotent.
    fn confirmed_manifest_remove(&mut self, cred: &uapi::Ucred, name: &str, exec: &mut dyn NftExec, at: u64) -> String {
        let verb = "confirmed-manifest-remove";
        let owner = crate::catalog::owner_cap_dir();
        if let Err(e) = crate::catalog::remove_owner_manifest(&owner, name) {
            journal(cred, verb, name, "store-fail", "-", &e.to_string());
            return "ERR store".to_string();
        }
        let _ = store::remove_bless(&self.store, name);
        let _ = store::remove_pin(&self.store, name);
        let _ = store::clear_fault(&self.store, name);
        if let Err(e) = crate::confirmed::reconcile_cap(&self.store, exec) {
            let msg = format!("{e:?}");
            self.reproject();
            journal(cred, verb, name, "apply-fail", "-", &msg);
            return "ERR apply-failed".to_string();
        }
        self.reproject();
        journal(cred, verb, name, "accept", "-", "removed");
        let _ = append_event(&self.run, at, "manifest-remove", name, "removed");
        format!("OK confirmed-manifest-remove {name}")
    }

    // ---- PRIVILEGED ceremony-commit handlers (root peer only; ADR-007 S6 fix #4 redesign) -----------
    // These replace the removed `egressd confirmed-*` CLI: the SAME store+reconcile engine (confirmed.rs),
    // now driven IN the running daemon rather than a transient root process gatekeeperd forked. `exec` is
    // the daemon's live `ShellNft`, so the daemon (which holds CAP_NET_ADMIN) is the sole nft mutator —
    // gatekeeperd relays the confirmed op over the socket and touches no network capability. Each verb
    // re-validates tier/grammar (defense in depth — the request already parsed, but the trust boundary is
    // "the string originated from a uid-1000 request; the ceremony proved intent, not well-formedness").

    /// Persist + apply a CONFIRMED bless of a broad ceremony profile (`web-browsing`). Intent-first: the
    /// durable record (tier `ceremony`) is written before the browser rule install, which lands IFF the
    /// slice exists now (usually it won't — the browser isn't running yet — so it stays pending and
    /// `browser-up` installs it at launch).
    fn confirmed_bless(&mut self, cred: &uapi::Ucred, profile: &str, exec: &mut dyn NftExec, at: u64) -> String {
        if bless_tier(profile) != Some(BlessTier::Ceremony) || !is_broad_profile(profile) {
            journal(cred, "confirmed-bless", profile, "deny", "-", "not a ceremony-tier profile");
            return "ERR not-ceremony-tier".to_string();
        }
        if let Err(e) = store::write_bless(
            &self.store,
            &BlessRecord { profile: profile.to_string(), tier: "ceremony".into(), blessed: at },
        ) {
            journal(cred, "confirmed-bless", profile, "store-fail", "-", &e.to_string());
            return "ERR store".to_string();
        }
        let _ = store::clear_fault(&self.store, profile);
        let installed = crate::confirmed::reconcile_web_browsing(&self.store, exec, self.desktop_uid).unwrap_or(false);
        self.reproject();
        journal(cred, "confirmed-bless", profile, "accept", "-", if installed { "live" } else { "pending (browser not up)" });
        let _ = append_event(&self.run, at, "bless", profile, if installed { "enabled" } else { "enabled (pending browser)" });
        format!("OK confirmed-bless {profile} {}", if installed { "live" } else { "pending" })
    }

    /// Revoke a CONFIRMED ceremony bless: tear down live enforcement FIRST (MF-5 — the browser rule must
    /// go, or the panel reads "Disabled" while broad egress persists), and only then drop the record, so
    /// a teardown failure never leaves "revoked in the store, still allowed in the kernel".
    fn confirmed_unbless(&mut self, cred: &uapi::Ucred, profile: &str, exec: &mut dyn NftExec, at: u64) -> String {
        if bless_tier(profile) != Some(BlessTier::Ceremony) {
            journal(cred, "confirmed-unbless", profile, "deny", "-", "not a ceremony-tier profile");
            return "ERR not-ceremony-tier".to_string();
        }
        if let Err(e) = apply::uninstall_browser_rules(exec) {
            let msg = format!("{e:?}");
            let _ = store::write_fault(&self.store, profile, FaultKind::ApplyFail, &msg, at);
            self.reproject();
            journal(cred, "confirmed-unbless", profile, "apply-fail", "-", &msg);
            return "ERR apply-failed".to_string();
        }
        let _ = store::remove_bless(&self.store, profile);
        let _ = store::clear_fault(&self.store, profile);
        self.reproject();
        journal(cred, "confirmed-unbless", profile, "accept", "-", "revoked");
        let _ = append_event(&self.run, at, "unbless", profile, "revoked");
        format!("OK confirmed-unbless {profile}")
    }

    /// Add a CONFIRMED raw destination. Intent-first (MF-3): the triple is stored before the DoT resolve,
    /// so a ceremony approved before the network is up persists as "blessed, waiting" and heals on
    /// reconcile. `@raw_pinned` is reconciled to the UNION of all raw entries (never a per-entry element).
    fn confirmed_add_raw(&mut self, cred: &uapi::Ucred, wire: &str, exec: &mut dyn NftExec, at: u64) -> String {
        let t = match parse_raw_triple(wire) {
            Ok(t) => t,
            Err(e) => {
                journal(cred, "confirmed-add-raw", "raw", "deny", "-", e);
                return format!("ERR bad-destination: {e}");
            }
        };
        if let Err(e) = store::add_raw(&self.store, &t) {
            journal(cred, "confirmed-add-raw", "raw", "store-fail", "-", &e.to_string());
            return "ERR store".to_string();
        }
        let mut resolver = crate::confirmed::DotRawResolver;
        let (_, pending) = crate::confirmed::resolve_raw(&self.store, &mut resolver, at);
        match crate::confirmed::reconcile_cap(&self.store, exec) {
            Ok(_) => {
                self.reproject();
                let live = store::list_raw_pins(&self.store).iter().any(|r| r.triple == t);
                journal(cred, "confirmed-add-raw", "raw", "accept", "-", if live { "pinned" } else { "pending" });
                let _ = append_event(&self.run, at, "add-raw", "raw", if live { "pinned" } else { "pending" });
                if live {
                    format!("OK confirmed-add-raw {} live", t.to_wire())
                } else {
                    format!("OK confirmed-add-raw {} pending ({pending} pending total)", t.to_wire())
                }
            }
            Err(e) => {
                // Record persisted (intent-first); a later reconcile heals it. Report the apply miss so
                // the caller (and the ceremony log) knows the element did not land yet.
                let msg = format!("{e:?}");
                self.reproject();
                journal(cred, "confirmed-add-raw", "raw", "apply-fail", "-", &msg);
                let _ = append_event(&self.run, at, "add-raw", "raw", "apply-failed");
                "ERR apply-failed".to_string()
            }
        }
    }

    /// Remove a CONFIRMED raw destination: drop the intent, then reconcile `@raw_pinned` to the UNION of
    /// the REMAINING entries (MF-5 — never a per-entry element delete, which would kill a shared tuple).
    fn confirmed_remove_raw(&mut self, cred: &uapi::Ucred, wire: &str, exec: &mut dyn NftExec, at: u64) -> String {
        let t = match parse_raw_triple(wire) {
            Ok(t) => t,
            Err(e) => {
                journal(cred, "confirmed-remove-raw", "raw", "deny", "-", e);
                return format!("ERR bad-destination: {e}");
            }
        };
        if let Err(e) = store::remove_raw(&self.store, &t) {
            journal(cred, "confirmed-remove-raw", "raw", "store-fail", "-", &e.to_string());
            return "ERR store".to_string();
        }
        let mut resolver = crate::confirmed::DotRawResolver;
        let _ = crate::confirmed::resolve_raw(&self.store, &mut resolver, at);
        match crate::confirmed::reconcile_cap(&self.store, exec) {
            Ok(_) => {
                self.reproject();
                journal(cred, "confirmed-remove-raw", "raw", "accept", "-", "revoked");
                let _ = append_event(&self.run, at, "remove-raw", "raw", "revoked");
                format!("OK confirmed-remove-raw {}", t.to_wire())
            }
            Err(e) => {
                let msg = format!("{e:?}");
                self.reproject();
                journal(cred, "confirmed-remove-raw", "raw", "apply-fail", "-", &msg);
                "ERR apply-failed".to_string()
            }
        }
    }

    /// Refresh BOTH `/run` projections after a store change (curated pinned map + legible state view),
    /// so the uid-1000 UI never reads a stale panel and the pinned map stays in step. Then recompose the
    /// ADR-009 `/etc/hosts` bridge so the just-changed blessed pins are resolvable by the uid-1000 DMS Go
    /// backend via NSS `files` (the delivery half of ADR-007 S7). Best-effort: a projection or compose
    /// failure never fails the bless it reports.
    fn reproject(&self) {
        // Load the catalog once for BOTH consumers: the state view (source/purpose/feature card tokens)
        // and the hosts bridge (sealed-source delivery filter). A cheap two-dir read; no shared mutable
        // state to keep in sync.
        let catalog = crate::catalog::load_catalog();
        let _ = store::project_pinned(&self.store, &self.run);
        let _ = store::project_state(&self.store, &self.run, &catalog);
        // ADR-009 delivery bridge: recompose /run/shrek/hosts (← /etc/hosts) so it carries the blessed
        // SEALED-source egress pins we just projected (owner pins are structurally excluded, §4.4). Under
        // the hosts lock so it never interleaves with a provider bind/unbind; a distinct lock from the
        // store lock held above, always taken store→hosts, so no cycle. compose reads the pinned map from
        // the egress `/run` view we just refreshed.
        let home = hosts::hosts_home_dir();
        let hosts_run = hosts::hosts_run_dir();
        if let Ok(_lock) = hosts::lock_hosts(&home) {
            let _ = hosts::compose_hosts(&home, &hosts_run, &catalog);
        }
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
        // 3. apply element-only: reconcile the WHOLE @cap_pinned union (this profile's fresh pins folded
        //    in with every other grant's — weather's tuples land as `<ip> . tcp . 443` alongside any raw
        //    tuples; ADR-009 §4.5). The pin record is written above, so the union already sees it.
        match crate::confirmed::reconcile_cap(&self.store, exec) {
            Ok(_) => {
                let _ = store::clear_fault(&self.store, profile);
                self.reproject();
                let detail = format!("{} ip(s)", pins.len());
                journal(cred, verb, profile, "accept", &rip, &detail);
                let _ = append_event(&self.run, at, verb, profile, &detail);
                format!("OK {verb} {profile} {}", pins.len())
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
        // Reconcile @cap_pinned to the union EXCLUDING this profile FIRST (tears down its tuples while
        // leaving every other grant's in place, ADR-009 §4.5); only clear the durable records if that
        // succeeded, so a delete failure leaves the profile consistently "still blessed, retry-able",
        // never a drift where records say revoked but elements linger.
        let desired = crate::confirmed::desired_cap_union(&self.store, Some(profile));
        match apply::apply_cap(exec, &desired) {
            Ok(_) => {
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
    let mut healed = 0usize;
    // 1. SELF-HEAL capability pins (no nft): a blessed one-click profile with NO stored pin — the
    //    intent-first residue of a first-run bless before the clock/network converged — gets a fresh DoT
    //    re-resolve now. NO per-profile apply anymore: the single `@cap_pinned` union apply (step 3)
    //    folds every capability + raw grant together (ADR-009 §4.5).
    for b in store::list_bless(store) {
        if !admits_socket_bless(&b.profile) {
            continue; // only pinnable one-click profiles carry runtime elements
        }
        let has_pin = store::load_pin(store, &b.profile).map(|r| !r.pins.is_empty()).unwrap_or(false);
        if !has_pin {
            match resolver.resolve(&b.profile) {
                Ok((pins, _)) if !pins.is_empty() => {
                    let rec = PinRecord { profile: b.profile.clone(), pins, resolved: at };
                    if store::write_pin(store, &rec).is_ok() {
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
    }

    // 2. Re-resolve raw destinations into the resolved cache (S4; MF-3/Q4 survive-reboot). No nft here —
    //    this only refreshes the cache the union apply below reads.
    let mut raw_resolver = crate::confirmed::DotRawResolver;
    let (raw_ok, raw_pending) = crate::confirmed::resolve_raw(store, &mut raw_resolver, at);

    // 3. ONE element-only union apply: `@cap_pinned` = every blessed capability's stored pins + every raw
    //    cache pin (ADR-009 §4.5). Restores runtime allows across a restart/reboot without touching the
    //    S1 deny floor. A blessed profile whose baked set is absent (the oneshot didn't run) fails here
    //    and leaves the deny floor — fail-closed.
    let cap = match crate::confirmed::reconcile_cap(store, exec) {
        Ok(present) => present.len(),
        Err(e) => {
            eprintln!("egressd[boot]: cap reconcile failed (deny floor stands): {e:?}");
            0
        }
    };

    // 4. web-browsing cgroup rules (separate from @cap_pinned): a blessed `web-browsing` re-installs its
    //    cgroup accept-pair IFF the slice exists yet (else it stays legibly pending and heals at browser
    //    launch via `browser-up`). Inert with no web-browsing bless.
    let browser = match crate::confirmed::reconcile_web_browsing(store, exec, desktop_uid()) {
        Ok(installed) => installed,
        Err(e) => {
            eprintln!("egressd[boot]: web-browsing reconcile failed: {e:?}");
            false
        }
    };

    let _ = store::project_pinned(store, run);
    let _ = store::project_state(store, run, &crate::catalog::load_catalog());
    let summary = format!(
        "reconcile: {cap} cap element(s), {healed} re-resolved; raw {raw_ok} pinned/{raw_pending} pending; browser {}",
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
        // A root peer (gatekeeperd's ceremony-commit relay) may send a raw-triple line up to
        // REQ_MAX_PRIV; every other peer is held to the tight uid-1000 cap.
        let cap = if cred.uid == 0 { REQ_MAX_PRIV } else { REQ_MAX };
        let resp = match read_request(&mut stream, cap) {
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
    use std::net::Ipv4Addr;

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
    fn parse_accepts_and_bounds_the_hosts_verbs() {
        // ADR-008: `bind` is the SOLE 3-token verb; the address is canonicalized.
        assert_eq!(
            parse_request("bind local 192.168.1.152"),
            Ok(Request::Bind("local".into(), "192.168.1.152".into()))
        );
        assert_eq!(parse_request("unbind anthropic\n"), Ok(Request::Unbind("anthropic".into())));
        // a non-canonical-but-parseable octet form is re-rendered canonically into the Request.
        // (`Ipv4Addr::from_str` is strict, so this is really just the round-trip of a normal quad.)
        assert_eq!(
            parse_request("bind codex 10.0.0.4"),
            Ok(Request::Bind("codex".into(), "10.0.0.4".into()))
        );
        // token must be one of the CLOSED provider set — not a sealed egress name, not a public host.
        assert!(parse_request("bind swamp 1.2.3.4").is_err());
        assert!(parse_request("bind shrek-model 1.2.3.4").is_err());
        assert!(parse_request("unbind github 1.2.3.4").is_err()); // (also: 3rd field on unbind)
        // address must be a strict IPv4 literal (hostnames never resolve via NSS files; hex/short forms).
        assert!(parse_request("bind local myhost.lan").is_err());
        assert!(parse_request("bind local 0x7f000001").is_err());
        assert!(parse_request("bind local 127.1").is_err());
        assert!(parse_request("bind local ::1").is_err());
        // arity: bind needs BOTH fields and rejects a 4th; unbind rejects a 2nd.
        assert!(parse_request("bind local").is_err()); // missing address
        assert!(parse_request("bind").is_err()); // missing both
        assert!(parse_request("bind local 1.2.3.4 extra").is_err()); // smuggled 4th field
        assert!(parse_request("unbind local extra").is_err()); // unbind takes one token
    }

    #[test]
    fn authorize_hosts_verbs_are_identity_gated_only() {
        let owner = 1000;
        let bind = Request::Bind("local".into(), "1.2.3.4".into());
        let unbind = Request::Unbind("local".into());
        // the desktop owner may bind/unbind (no bless tier — the closed token is the authority).
        assert_eq!(authorize(owner, &bind, owner), Decision::Allow);
        assert_eq!(authorize(owner, &unbind, owner), Decision::Allow);
        // any OTHER uid — including root — is refused: bind/unbind are the OWNER's verbs, and the root
        // ceremony path is a disjoint set (they are not `is_privileged`).
        assert!(matches!(authorize(1001, &bind, owner), Decision::Deny(_)));
        assert!(matches!(authorize(0, &bind, owner), Decision::Deny(_)));
        assert!(!bind.is_privileged() && !unbind.is_privileged());
        assert!(bind.is_hosts() && unbind.is_hosts());
    }

    #[test]
    fn read_request_bounds_and_disconnect() {
        // normal line
        assert_eq!(read_request(Cursor::new(b"bless weather\n".to_vec()), REQ_MAX).unwrap(), "bless weather");
        // EOF before newline (mid-request disconnect)
        assert_eq!(read_request(Cursor::new(b"bless weat".to_vec()), REQ_MAX), Err("disconnected before newline"));
        // giant payload with no newline
        let giant = vec![b'a'; REQ_MAX + 10];
        assert_eq!(read_request(Cursor::new(giant), REQ_MAX), Err("request too large"));
        // empty stream
        assert_eq!(read_request(Cursor::new(Vec::new()), REQ_MAX), Err("disconnected before newline"));
    }

    #[test]
    fn read_request_per_uid_cap_gates_the_privileged_line_length() {
        // A raw-triple ceremony line exceeds the uid-1000 REQ_MAX but fits REQ_MAX_PRIV. The cap is the
        // caller's (serve() derives it from the peer uid), so the SAME bytes pass at the root cap and are
        // rejected at the uid-1000 cap — this is what keeps the untrusted front door tight.
        let host = "x".repeat(120);
        let line = format!("confirmed-add-raw {host}.example.com:tcp:443\n");
        assert!(line.len() > REQ_MAX && line.len() <= REQ_MAX_PRIV, "fixture must straddle the two caps");
        assert!(read_request(Cursor::new(line.clone().into_bytes()), REQ_MAX_PRIV).is_ok());
        assert_eq!(read_request(Cursor::new(line.into_bytes()), REQ_MAX), Err("request too large"));
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

    #[test]
    fn parse_accepts_the_privileged_verbs() {
        assert_eq!(parse_request("confirmed-bless web-browsing"), Ok(Request::ConfirmedBless("web-browsing".into())));
        assert_eq!(parse_request("confirmed-unbless web-browsing"), Ok(Request::ConfirmedUnbless("web-browsing".into())));
        // a raw triple is canonicalized through the sealed grammar (never a `valid_token`).
        assert_eq!(parse_request("confirmed-add-raw example.com:tcp:8443"), Ok(Request::ConfirmedAddRaw("example.com:tcp:8443".into())));
        assert_eq!(parse_request("confirmed-remove-raw 203.0.113.7:udp:8883"), Ok(Request::ConfirmedRemoveRaw("203.0.113.7:udp:8883".into())));
        // a malformed destination is refused at parse (defense in depth; the daemon re-checks too).
        assert!(parse_request("confirmed-add-raw singlelabel:tcp:443").is_err());
        assert!(parse_request("confirmed-add-raw e.com:icmp:0").is_err());
        // still no smuggled third field.
        assert!(parse_request("confirmed-add-raw a.com:tcp:443 extra").is_err());
    }

    #[test]
    fn authorize_ceremony_commit_is_root_only() {
        let root = 0;
        let desktop = 1000;
        // ROOT peer (gatekeeperd post-ceremony) ⇒ the privileged verbs are allowed.
        assert_eq!(authorize(root, &Request::ConfirmedBless("web-browsing".into()), desktop), Decision::Allow);
        assert_eq!(authorize(root, &Request::ConfirmedAddRaw("a.com:tcp:443".into()), desktop), Decision::Allow);
        // the DESKTOP uid (or any non-root) can NEVER reach them — the socket is not a second front door.
        assert!(matches!(authorize(desktop, &Request::ConfirmedBless("web-browsing".into()), desktop), Decision::Deny(_)));
        assert!(matches!(authorize(desktop, &Request::ConfirmedAddRaw("a.com:tcp:443".into()), desktop), Decision::Deny(_)));
        assert!(matches!(authorize(1234, &Request::ConfirmedRemoveRaw("a.com:tcp:443".into()), desktop), Decision::Deny(_)));
        // and root does NOT get the uid-1000 verbs (root is not the desktop user).
        assert!(matches!(authorize(root, &Request::Bless("weather".into()), desktop), Decision::Deny(_)));
    }

    #[test]
    fn confirmed_bless_over_ipc_persists_ceremony_tier_and_gates_non_root() {
        let (store, run) = sup_dirs("cbless");
        let mut sup = Supervisor::new(store.clone(), run.clone(), 1000);
        let root = uapi::Ucred { pid: 10, uid: 0, gid: 0 };
        let desktop = uapi::Ucred { pid: 11, uid: 1000, gid: 1000 };
        let t = Instant::now();

        // A NON-root peer is refused at the boundary — the socket is not a ceremony bypass; nft untouched.
        let denied = sup.handle(&desktop, "confirmed-bless web-browsing", &mut NoopExec, &mut NoopResolver, t, 7);
        assert!(denied.starts_with("ERR denied"), "{denied}");
        assert!(store::load_bless(&store, "web-browsing").is_none(), "a denied commit must not persist");

        // A ROOT peer (gatekeeperd post-ceremony) persists the bless at tier=ceremony (pending: no slice up).
        let ok = sup.handle(&root, "confirmed-bless web-browsing", &mut OkExec, &mut NoopResolver, t, 9);
        assert!(ok.starts_with("OK confirmed-bless web-browsing"), "{ok}");
        let rec = store::load_bless(&store, "web-browsing").unwrap();
        assert_eq!(rec.tier, "ceremony");
        let state = std::fs::read_to_string(store::state_map(&run)).unwrap();
        assert!(state.contains("profile web-browsing tier=ceremony"), "{state}");

        // Even from root, a ceremony-commit for a NON-ceremony profile is refused by the engine re-check
        // (defense in depth: the socket authorized the ROOT identity, the engine still guards the tier).
        let bad = sup.handle(&root, "confirmed-bless weather", &mut NoopExec, &mut NoopResolver, t, 11);
        assert!(bad.starts_with("ERR"), "{bad}");
        assert!(store::load_bless(&store, "weather").is_none());
    }

    #[test]
    fn parse_accepts_want_and_manifest_verbs() {
        assert_eq!(parse_request("want weather"), Ok(Request::Want("weather".into())));
        assert_eq!(
            parse_request("confirmed-manifest-install radar"),
            Ok(Request::ConfirmedManifestInstall("radar".into()))
        );
        assert_eq!(
            parse_request("confirmed-manifest-remove radar\n"),
            Ok(Request::ConfirmedManifestRemove("radar".into()))
        );
        assert!(parse_request("want").is_err()); // needs a token
        assert!(parse_request("want weather extra").is_err()); // no third field
        assert!(parse_request("confirmed-manifest-install ../escape").is_err()); // traversal token
    }

    #[test]
    fn authorize_want_is_identity_gated_manifest_is_root_only() {
        let desktop = 1000;
        let root = 0;
        // `want`: the desktop uid may file; any other uid (INCLUDING root) may not — it is the owner's verb.
        assert_eq!(authorize(desktop, &Request::Want("weather".into()), desktop), Decision::Allow);
        assert!(matches!(authorize(1001, &Request::Want("weather".into()), desktop), Decision::Deny(_)));
        assert!(matches!(authorize(root, &Request::Want("weather".into()), desktop), Decision::Deny(_)));
        // manifest verbs: PRIVILEGED (root-only); a uid-1000 peer can never reach them.
        assert!(Request::ConfirmedManifestInstall("r".into()).is_privileged());
        assert!(Request::ConfirmedManifestRemove("r".into()).is_privileged());
        assert_eq!(authorize(root, &Request::ConfirmedManifestInstall("r".into()), desktop), Decision::Allow);
        assert!(matches!(authorize(desktop, &Request::ConfirmedManifestInstall("r".into()), desktop), Decision::Deny(_)));
        assert!(matches!(authorize(desktop, &Request::ConfirmedManifestRemove("r".into()), desktop), Decision::Deny(_)));
    }

    #[test]
    fn want_refuses_a_non_catalog_token() {
        // With no catalog present (unit env: the sealed `/usr` dir is absent ⇒ empty catalog), a `want`
        // for any token is refused as unknown-capability — uid 1000 can only ever surface a root-vetted
        // name. The accept path is exercised end-to-end in the oracle (with a real catalog).
        let (store, run) = sup_dirs("want");
        let mut sup = Supervisor::new(store, run, 1000);
        let desktop = uapi::Ucred { pid: 1, uid: 1000, gid: 1000 };
        let r = sup.handle(&desktop, "want weather", &mut OkExec, &mut NoopResolver, Instant::now(), 5);
        assert_eq!(r, "ERR unknown-capability");
    }

    #[test]
    fn manifest_install_is_root_only_and_errs_without_a_staged_candidate() {
        let (store, run) = sup_dirs("manifest");
        let mut sup = Supervisor::new(store, run, 1000);
        let root = uapi::Ucred { pid: 2, uid: 0, gid: 0 };
        let desktop = uapi::Ucred { pid: 3, uid: 1000, gid: 1000 };
        // uid-1000 is refused at the boundary (the socket is not a ceremony bypass).
        let denied = sup.handle(&desktop, "confirmed-manifest-install radar", &mut OkExec, &mut NoopResolver, Instant::now(), 7);
        assert!(denied.starts_with("ERR denied"), "{denied}");
        // root, but nothing staged ⇒ a clean ERR (no candidate to commit; no fs write).
        let r = sup.handle(&root, "confirmed-manifest-install radar-absent-xyzzy", &mut OkExec, &mut NoopResolver, Instant::now(), 9);
        assert_eq!(r, "ERR no-staged-candidate");
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

    #[test]
    fn privileged_ceremony_commit_bypasses_the_rate_limiter() {
        // The S6.1 regression guard: a ROOT ceremony commit must NEVER be rate-limited, even when the
        // shared uid-1000 budget is exhausted — else a legitimate post-ceremony revoke is silently
        // dropped (MF-5: "revoked intent, still-live enforcement"). The old CLI path took the store lock
        // directly and never touched the limiter; routing the commit through the socket must preserve that.
        let (store, run) = sup_dirs("cbypass");
        let mut sup = Supervisor::new(store.clone(), run.clone(), 1000);
        let desktop = uapi::Ucred { pid: 2, uid: 1000, gid: 1000 };
        let root = uapi::Ucred { pid: 3, uid: 0, gid: 0 };
        let t0 = Instant::now();
        // Saturate the limiter with uid-1000 attempts (rate-limited before touching nft/resolver).
        for _ in 0..(RATE_MAX + 2) {
            let _ = sup.handle(&desktop, "bless web-browsing", &mut NoopExec, &mut NoopResolver, t0, 0);
        }
        // A uid-1000 mutating attempt is now firmly rate-limited...
        assert_eq!(
            sup.handle(&desktop, "bless weather", &mut NoopExec, &mut NoopResolver, t0, 0),
            "ERR rate-limited"
        );
        // ...but the ROOT ceremony commit sails through and executes (no bless present ⇒ a clean OK).
        let r = sup.handle(&root, "confirmed-unbless web-browsing", &mut OkExec, &mut NoopResolver, t0, 5);
        assert!(r.starts_with("OK confirmed-unbless"), "privileged commit must bypass the limiter: {r}");
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
