//! consent.rs — the console consent ceremony (ADR-003 Part 2 step 3, docs/bench-authz-consent-slice.md).
//!
//! The three AUTHORITY-INCREASING bench verbs (`grant`, `network`-to-a-profile, `export`) cannot be
//! applied for a non-root desktop peer on its say-so alone: a hijacked or overreaching agent inside the
//! session must not be able to silently expand its own authority. This module is the gate — a human OK on
//! a surface the session cannot spoof or suppress:
//!
//!   SecureAttentionKey (logind) -> a gatekeeperd-owned kernel VT (raw ioctls, NOT logind TakeControl)
//!   -> a sanitized authority DIFF -> a typed answer -> apply ONLY on an exact bound-tuple match.
//!
//! **Two clean layers, by design (the proof strategy rests on this split):**
//!   * The SECURITY CORE — verb resolution (in `bench_plane`), the output-boundary sanitizer, the
//!     authority-diff render, the tuple binding / apply decision, the trifecta predicate, the anti-flood
//!     cooldown — is pure and I/O-free, and is exhaustively unit-tested here over a MOCK console.
//!   * The CONSOLE TRANSPORT ([`RealConsole`]) — the busctl SAK subscription and the VT ioctls / tty I/O
//!     — is the ONLY part faked in unit tests; it is proven by the sealed-VM dogfood (real seat + logind).
//!
//! **Fail closed everywhere.** No SAK, no VT, a switch that misses its deadline, a render failure, an
//! answer timeout, a malformed/short answer, a peer that exits or disconnects, a start-time that changed,
//! a target whose object identity moved — every one of these DENIES. There is no path to "approve", and
//! no fallback to sudo, anywhere in this file.
//!
//! **VM-proven boundary — confirm on the sealed image before trusting the live ceremony** (these are the
//! pre-implementation verify items; until they hold, [`RealConsole`] simply fails closed):
//!   1. systemd is >= 257 and logind's `SecureAttentionKey` is enabled.
//!   2. logind detects the SAK chord on seat input devices INDEPENDENTLY of the compositor (so a hijacked
//!      session cannot suppress it).
//!   3. the kernel has CONFIG_VT + fbcon so the reserved text VT actually scans out while the compositor's
//!      DRM master is paused.

use crate::bench_plane::{self, AuthorityPlan};
use crate::linux_uapi::{self, Ucred};
use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// ---- ceremony parameters ------------------------------------------------------------------------

/// The dedicated console VT the ceremony owns. Fixed ABOVE logind's autovt range (`NAutoVTs` default 6)
/// so activating it never spawns an `autovt@` getty on top of the consent screen. Reserved in the sealed
/// image; its fd is held from daemon start.
const CONSENT_VT: u32 = 8;

/// Timeouts (load-bearing — the anti-flood math and the fail-closed matrix depend on them). Each is a
/// DENY on expiry.
const SAK_ARM_TIMEOUT_MS: i32 = 60_000;
const VT_SWITCH_DEADLINE_MS: u64 = 5_000;
const ANSWER_TIMEOUT_MS: i32 = 45_000;

/// Anti-flood cooldown after a deny, per (uid, verb-family), with escalation (10s, 20s, 40s, 80s, …
/// capped at 300s). Defeats the SEQUENTIAL attack the serial broker does NOT: an agent spamming
/// authority requests back-to-back to burn SAK-arm timeouts and train the human into fatigue-approving.
const COOLDOWN_BASE_SECS: u64 = 10;
const COOLDOWN_MAX_SECS: u64 = 300;

// ---- deny reasons (wire tag + audit string; never leaks the nonce) ------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DenyReason {
    NoSak,
    NoVt,
    RenderFail,
    ReadFail,
    Timeout,
    Declined,
    StarttimeChanged,
    PeerGone,
    Disconnected,
    NoStarttime,
}

impl DenyReason {
    /// Short, non-sensitive tag for the `RESULT … refused ceremony-<tag>` wire line and the host oracle.
    fn tag(self) -> &'static str {
        match self {
            DenyReason::NoSak => "no-sak",
            DenyReason::NoVt => "no-vt",
            DenyReason::RenderFail => "render-fail",
            DenyReason::ReadFail => "read-fail",
            DenyReason::Timeout => "timeout",
            DenyReason::Declined => "declined",
            DenyReason::StarttimeChanged => "starttime-changed",
            DenyReason::PeerGone => "peer-gone",
            DenyReason::Disconnected => "disconnected",
            DenyReason::NoStarttime => "no-starttime",
        }
    }
}

// ---- the bound request + the apply decision (pure; the swap/replay matrix lives here) ------------

/// The tuple an approval is bound to. In the serial single-connection broker the `Pending` IS the
/// in-flight request (there is never a second one to confuse it with — the structural anti-concurrency
/// property), so `uid`/`pid`/`verb`/`argv`/target are fixed by construction; the RUNTIME re-checks that
/// still matter at apply are `starttime` (PID-reuse / TOCTOU), peer-liveness, and — inside
/// `commit_authority` — the target's object identity. The `nonce` never leaves the daemon: its only
/// observable role is the confirmation CODE a blind agent cannot guess.
pub(crate) struct Pending {
    pub nonce: u128,
    pub uid: u32,
    pub pid: u32,
    pub starttime: u64,
}

/// Facts re-read at apply time (after the human answers) — kept separate from `Pending` so the decision
/// is a pure function of (what we bound) vs (what is true now).
struct LiveFacts {
    starttime: Option<u64>,
    peer_alive: bool,
}

/// What the human typed on the VT (first line only; raw bytes are sanitized only for RENDER, never
/// trusted here — the answer is compared, not displayed).
#[derive(Clone, Debug)]
struct Answer {
    line: String,
}

enum ApplyDecision {
    Apply,
    Deny(DenyReason),
}

/// The apply gate — pure. An approval applies ONLY when: the human's answer is affirmative (a bare `y`,
/// or the exact confirmation code for a high-authority verb), the peer is still connected, and its
/// start-time is unchanged since we bound it. Every other case denies. (Target object identity is the
/// remaining tuple field; it is re-verified inside `bench_plane::commit_authority`, at the materialize.)
fn decide_apply(p: &Pending, live: &LiveFacts, ans: &Answer, code: Option<&str>) -> ApplyDecision {
    let affirmative = match code {
        Some(c) => ans.line.trim() == c, // high-authority: the exact typed code
        None => ans.line.trim() == "y",  // normal: a bare y
    };
    if !affirmative {
        return ApplyDecision::Deny(DenyReason::Declined);
    }
    if !live.peer_alive {
        return ApplyDecision::Deny(DenyReason::Disconnected);
    }
    match live.starttime {
        Some(t) if t == p.starttime => ApplyDecision::Apply,
        Some(_) => ApplyDecision::Deny(DenyReason::StarttimeChanged),
        None => ApplyDecision::Deny(DenyReason::PeerGone),
    }
}

// ---- the output-boundary sanitizer (kills escape injection + Unicode spoofing) ------------------

/// Sanitize ONE untrusted value for rendering on the VT. Allowlist printable ASCII + space; escape
/// everything else — control, C1, DEL, tab/newline, AND all non-ASCII — as a visible `\u{XXXX}`. In one
/// move this defeats CSI/OSC/C1 escape injection, line/entry forging (newline), and Unicode visual
/// spoofing (U+202E right-to-left override reordering a path, homoglyph paths like a Cyrillic-с). The
/// screen STRUCTURE (labels, framing, real newlines) is assembled by us from static text — only VALUES
/// pass through here, so escaping newline is correct: an untrusted value must never inject a line.
fn sanitize_value(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        if c == ' ' || c.is_ascii_graphic() {
            o.push(c);
        } else {
            o.push_str(&format!("\\u{{{:04X}}}", c as u32));
        }
    }
    o
}

// ---- the confirmation code (the nonce's only observable role) -----------------------------------

/// A 6-digit confirmation code derived from the daemon-minted nonce. Shown ON the VT; a high-authority
/// verb applies only if the human types it back. A blind/hijacked agent that never sees the VT cannot
/// produce it. The nonce itself never crosses any boundary; only these six digits are displayed.
fn conf_code(nonce: u128) -> String {
    format!("{:06}", (nonce % 1_000_000) as u64)
}

// ---- the rendered authority diff ----------------------------------------------------------------

/// Build the full consent screen. Every dynamic value is sanitized; the frame is static. Leads with an
/// SGR reset + clear + home so the VT is in a known state regardless of prior content.
fn render_screen(plan: &AuthorityPlan, pending: &Pending, code: Option<&str>) -> String {
    let mut s = String::new();
    s.push_str("\x1b[0m\x1b[2J\x1b[H");
    s.push_str("========================================================\r\n");
    s.push_str("  SHREK OS  —  AUTHORITY CHANGE REQUEST\r\n");
    s.push_str("========================================================\r\n\r\n");
    s.push_str(&format!("  A program running as uid {} (pid {}) is asking\r\n", pending.uid, pending.pid));
    s.push_str("  to EXPAND its own authority on this machine.\r\n");
    s.push_str("  Approve ONLY if you started this and understand it.\r\n\r\n");
    s.push_str(&format!("  Action:  {}\r\n\r\n", sanitize_value(&plan.action())));
    s.push_str("  Details:\r\n");
    for (k, v) in &plan.diff_rows {
        s.push_str(&format!("    {:<20} {}\r\n", sanitize_value(k), sanitize_value(v)));
    }
    if plan.trifecta {
        s.push_str("\r\n  !! WARNING: after this, the bench can READ your files\r\n");
        s.push_str("     AND reach the network — data could leave this box.\r\n");
    }
    s.push_str("\r\n--------------------------------------------------------\r\n");
    match code {
        Some(c) => s.push_str(&format!("  To APPROVE, type this code then Enter:  {c}\r\n  Anything else DENIES.\r\n", )),
        None => s.push_str("  Type  y  then Enter to APPROVE.  Anything else DENIES.\r\n"),
    }
    s
}

// ---- the console transport boundary (mocked in tests, real in production) -----------------------

/// The spoof-resistant surface. The ONLY thing faked in unit tests; the real impl is proven in the VM.
/// Every method returns a `DenyReason` on failure — the orchestration just sequences them and fails
/// closed. `restore()` must be idempotent and safe to call even if no VT was taken.
trait Console {
    fn arm_sak(&mut self) -> Result<(), DenyReason>;
    fn take_vt(&mut self) -> Result<(), DenyReason>;
    fn render(&mut self, screen: &str) -> Result<(), DenyReason>;
    fn read_answer(&mut self) -> Result<Answer, DenyReason>;
    fn restore(&mut self);
}

// ---- orchestration (pure sequencing over the Console trait; the fail-closed matrix) --------------

enum Outcome {
    Approved,
    Denied(DenyReason),
}

struct CeremonyResult {
    rc: i32,
    outcome: Outcome,
}

fn denied<C: Console>(console: &mut C, r: DenyReason) -> CeremonyResult {
    console.restore();
    CeremonyResult { rc: 1, outcome: Outcome::Denied(r) }
}

/// Sequence the ceremony. Each stage either advances or denies (restoring the VT). `commit` runs ONLY
/// on a passing apply decision, and its return code becomes the wire rc. Generic over the console so the
/// unit tests drive the whole matrix with a scripted mock — the decision/binding/apply logic is real.
fn run_ceremony<C: Console>(
    plan: &AuthorityPlan,
    pending: &Pending,
    console: &mut C,
    reread_starttime: impl Fn(u32) -> Option<u64>,
    peer_alive: impl Fn() -> bool,
    commit: impl FnOnce() -> i32,
) -> CeremonyResult {
    if let Err(r) = console.arm_sak() {
        return denied(console, r);
    }
    if let Err(r) = console.take_vt() {
        return denied(console, r);
    }
    let code = plan.high_authority().then(|| conf_code(pending.nonce));
    let screen = render_screen(plan, pending, code.as_deref());
    if let Err(r) = console.render(&screen) {
        return denied(console, r);
    }
    let answer = match console.read_answer() {
        Ok(a) => a,
        Err(r) => return denied(console, r),
    };
    let live = LiveFacts { starttime: reread_starttime(pending.pid), peer_alive: peer_alive() };
    match decide_apply(pending, &live, &answer, code.as_deref()) {
        ApplyDecision::Deny(r) => denied(console, r),
        ApplyDecision::Apply => {
            let rc = commit();
            console.restore();
            CeremonyResult { rc, outcome: Outcome::Approved }
        }
    }
}

// ---- anti-flood cooldown (per-uid, per verb-family, escalating) ----------------------------------

struct CoolEntry {
    until: u64,
    strikes: u32,
}

fn cooldown_map() -> &'static Mutex<HashMap<(u32, String), CoolEntry>> {
    static M: OnceLock<Mutex<HashMap<(u32, String), CoolEntry>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The escalation curve: strike 1 → base, then double each strike, capped. Pure — unit-tested.
fn cooldown_secs(strikes: u32) -> u64 {
    let shift = strikes.clamp(1, 6) - 1;
    (COOLDOWN_BASE_SECS.saturating_mul(1u64 << shift)).min(COOLDOWN_MAX_SECS)
}

/// If a cooldown is active for (uid, verb-family) at `now`, return the epoch second it expires.
fn cooldown_active(uid: u32, verb: &str, now: u64) -> Option<u64> {
    let m = cooldown_map().lock().unwrap();
    m.get(&(uid, verb.to_string())).filter(|e| e.until > now).map(|e| e.until)
}

/// Record a deny: bump the strike count and arm/extend the cooldown from `now`.
fn note_deny(uid: u32, verb: &str, now: u64) {
    let mut m = cooldown_map().lock().unwrap();
    let e = m.entry((uid, verb.to_string())).or_insert(CoolEntry { until: 0, strikes: 0 });
    e.strikes = e.strikes.saturating_add(1);
    e.until = now + cooldown_secs(e.strikes);
}

// ---- small host facts ---------------------------------------------------------------------------

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The peer's process start-time (field 22 of `/proc/<pid>/stat`, in clock ticks since boot). Bound at
/// arm and re-read at apply: a recycled PID lands on a different start-time → deny. The comm field (2)
/// may contain spaces/parens, so we parse fields AFTER the last ')'.
fn proc_starttime(pid: u32) -> Option<u64> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rparen = s.rfind(')')?;
    let after: Vec<&str> = s[rparen + 1..].split_whitespace().collect();
    // after[0] = state (field 3); start-time (field 22) is 19 tokens further on.
    after.get(19)?.parse::<u64>().ok()
}

/// Peer still connected? A non-destructive `poll` for hang-up on the connection fd.
fn peer_fd_alive(fd: std::os::fd::RawFd) -> bool {
    !linux_uapi::fd_hungup(fd)
}

/// 128-bit nonce from `/dev/urandom`. Reads EXACTLY 16 bytes — `std::fs::read` would loop forever on
/// the infinite device (it reads to EOF), hanging the daemon. `None` (urandom unreadable — a deeply
/// broken box) fails the ceremony closed; the nonce is never predictable and never logged.
fn mint_nonce() -> Option<u128> {
    use std::io::Read;
    let mut f = std::fs::File::open("/dev/urandom").ok()?;
    let mut a = [0u8; 16];
    f.read_exact(&mut a).ok()?;
    Some(u128::from_le_bytes(a))
}

/// Structured audit of a ceremony outcome (invariant 6). Lands in the journal via gatekeeperd's stderr —
/// the broker's existing audit surface. The nonce is deliberately absent.
fn audit(event: &str, uid: u32, verb: &str, detail: &str) {
    eprintln!("gatekeeperd/consent: event={event} uid={uid} verb={verb} detail={detail:?}");
}

// ---- the production entry point (called by bench_plane::dispatch_socket) -------------------------

fn refused(verb: &str, tag: &str) -> (i32, Vec<String>) {
    (2, vec![format!("RESULT bench-{verb} - refused {tag}")])
}

fn ceremony_result_line(verb: &str, bench: &str, rc: i32) -> Vec<String> {
    vec![format!("RESULT bench-{verb} {bench} {}", if rc == 0 { "ok" } else { "fail" })]
}

/// Drive the console consent ceremony for one authority-increasing socket request. Peer-gated (root uses
/// the in-process `cli()`, only the bench uid may drive this, `shrek` is refused), cooldown-gated,
/// precheck-validated (the human is never asked on a validation failure), then the real ceremony. Returns
/// the `(rc, RESULT lines)` the socket front end frames as `RESULT …` / `END <rc>`.
pub fn run_socket_consent(cred: Ucred, peer_fd: std::os::fd::RawFd, verb: &str, rest: &[String]) -> (i32, Vec<String>) {
    // 1. Peer gate. Root never uses the socket ceremony (it drives cli() in-process); only `dev` may.
    if cred.uid == 0 {
        return refused(verb, "root-uses-cli");
    }
    if cred.uid != bench_plane::bench_user_uid() {
        audit("reject-peer", cred.uid, verb, "not-bench-uid");
        return refused(verb, "not-bench-uid");
    }
    // 2. Anti-flood cooldown (sequential-spam / SAK-fatigue defense).
    let now = now_secs();
    if let Some(until) = cooldown_active(cred.uid, verb, now) {
        audit("reject-cooldown", cred.uid, verb, &format!("{}s-left", until.saturating_sub(now)));
        return refused(verb, "cooldown");
    }
    // 3. Precheck — every validator runs here; a failure denies and the human is NEVER asked. A precheck
    //    failure is cheap + local (no VT, no human, no SAK), so it does NOT arm the cooldown: that defends
    //    against SAK fatigue, which only begins once a request reaches the ceremony. (A typo'd bench name
    //    must not lock the user out for escalating minutes.)
    let plan = match bench_plane::precheck_authority(verb, rest) {
        Ok(p) => p,
        Err((rc, msg)) => {
            eprintln!("gatekeeperd/consent: precheck refused verb={verb} uid={}: {msg}", cred.uid);
            audit("reject-precheck", cred.uid, verb, &msg);
            return (rc, vec![format!("RESULT bench-{verb} - refused precheck")]);
        }
    };
    // 4. Bind the tuple. No peer start-time (peer already gone) → deny before we ever touch the VT.
    let Some(starttime) = proc_starttime(cred.pid as u32) else {
        audit("deny", cred.uid, verb, DenyReason::NoStarttime.tag());
        note_deny(cred.uid, verb, now);
        return (1, vec![format!("RESULT bench-{verb} - refused ceremony-{}", DenyReason::NoStarttime.tag())]);
    };
    let Some(nonce) = mint_nonce() else {
        audit("deny", cred.uid, verb, "nonce-unavailable");
        return (1, vec![format!("RESULT bench-{verb} - refused ceremony-nonce")]);
    };
    let pending = Pending { nonce, uid: cred.uid, pid: cred.pid as u32, starttime };

    // 5. Run the real ceremony (SAK + kernel VT). Commit re-verifies target identity at the materialize.
    let mut console = RealConsole::new();
    let result = run_ceremony(
        &plan,
        &pending,
        &mut console,
        proc_starttime,
        || peer_fd_alive(peer_fd),
        || bench_plane::commit_authority(&plan),
    );

    // 6. Audit + cooldown + wire framing.
    match result.outcome {
        Outcome::Approved => {
            audit("approve", cred.uid, verb, &plan.action());
            (result.rc, ceremony_result_line(verb, &plan.bench, result.rc))
        }
        Outcome::Denied(r) => {
            audit("deny", cred.uid, verb, r.tag());
            note_deny(cred.uid, verb, now);
            (1, vec![format!("RESULT bench-{verb} - refused ceremony-{}", r.tag())])
        }
    }
}

// ---- the real console (VT ioctls + busctl SAK) — proven ONLY by the sealed-VM dogfood ------------

/// The production console. Owns a dedicated reserved VT with raw kernel ioctls (never logind
/// `TakeControl`/`SwitchTo`, which would steal session-controller status from the compositor and can
/// wedge the desktop — a non-clean deny). logind is used ONLY for the SAK signal, via a sender-pinned
/// busctl match. Every failure denies. This whole type is exercised only in the sealed VM (no seat in a
/// container / the host oracle → `arm_sak`/`take_vt` fail closed, which is exactly the headless assertion).
struct RealConsole {
    ctl: Option<std::fs::File>, // /dev/tty0 — VT control ioctls
    vt: Option<std::fs::File>,  // /dev/tty<CONSENT_VT> — the rendered screen + the answer
    saved_vt: u16,
    sak_child: Option<std::process::Child>,
}

impl RealConsole {
    fn new() -> RealConsole {
        let ctl = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty0").ok();
        let vt = std::fs::OpenOptions::new().read(true).write(true).open(format!("/dev/tty{CONSENT_VT}")).ok();
        RealConsole { ctl, vt, saved_vt: 0, sak_child: None }
    }
}

impl Console for RealConsole {
    fn arm_sak(&mut self) -> Result<(), DenyReason> {
        // VM-PROVEN ONLY (see the module header's verify items). Subscribe to logind's SecureAttentionKey
        // with a SENDER-PINNED match (the bus daemon fills `sender` authoritatively — an unpinned match
        // would let the `dev` agent forge its own SAK and flip the screen). Fail closed on ANY anomaly.
        use std::io::BufRead;
        use std::process::{Command, Stdio};
        let mut child = Command::new("busctl")
            .args([
                "--system",
                "--match",
                "type='signal',sender='org.freedesktop.login1',interface='org.freedesktop.login1.Manager',member='SecureAttentionKey'",
                "monitor",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .spawn()
            .map_err(|_| DenyReason::NoSak)?;
        let Some(out) = child.stdout.take() else {
            let _ = child.kill();
            return Err(DenyReason::NoSak);
        };
        let fd = out.as_raw_fd();
        self.sak_child = Some(child); // killed on restore()
        let deadline = Instant::now() + Duration::from_millis(SAK_ARM_TIMEOUT_MS as u64);
        let mut reader = std::io::BufReader::new(out);
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Err(DenyReason::NoSak);
            }
            let remaining = (deadline - now).as_millis() as i32;
            let rev = linux_uapi::poll_one(fd, linux_uapi::POLLIN, remaining.max(1)).unwrap_or(0);
            if rev & (linux_uapi::POLLHUP | linux_uapi::POLLERR | linux_uapi::POLLNVAL) != 0 {
                return Err(DenyReason::NoSak);
            }
            if rev & linux_uapi::POLLIN == 0 {
                continue;
            }
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                return Err(DenyReason::NoSak);
            }
            if line.contains("SecureAttentionKey") {
                return Ok(());
            }
        }
    }

    fn take_vt(&mut self) -> Result<(), DenyReason> {
        let cfd = self.ctl.as_ref().ok_or(DenyReason::NoVt)?.as_raw_fd();
        let st = linux_uapi::vt_getstate(cfd).map_err(|_| DenyReason::NoVt)?;
        self.saved_vt = st.v_active;
        linux_uapi::vt_activate(cfd, CONSENT_VT).map_err(|_| DenyReason::NoVt)?;
        // BOUNDED poll for the switch — never the unbounded VT_WAITACTIVE (which would wedge the serial
        // broker forever if a compositor in VT_PROCESS mode never releases). Deadline → deny.
        let deadline = Instant::now() + Duration::from_millis(VT_SWITCH_DEADLINE_MS);
        loop {
            if matches!(linux_uapi::vt_getstate(cfd), Ok(s) if s.v_active == CONSENT_VT as u16) {
                break;
            }
            if Instant::now() >= deadline {
                return Err(DenyReason::NoVt);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let vfd = self.vt.as_ref().ok_or(DenyReason::NoVt)?.as_raw_fd();
        let _ = linux_uapi::kd_setmode(vfd, linux_uapi::KD_TEXT);
        let _ = linux_uapi::tcflush_in(vfd);
        Ok(())
    }

    fn render(&mut self, screen: &str) -> Result<(), DenyReason> {
        use std::io::Write;
        let vt = self.vt.as_mut().ok_or(DenyReason::RenderFail)?;
        vt.write_all(screen.as_bytes()).map_err(|_| DenyReason::RenderFail)?;
        vt.flush().map_err(|_| DenyReason::RenderFail)
    }

    fn read_answer(&mut self) -> Result<Answer, DenyReason> {
        use std::io::Read;
        let vt = self.vt.as_mut().ok_or(DenyReason::ReadFail)?;
        let fd = vt.as_raw_fd();
        // Drop anything queued before the screen existed (a keystroke mid-typed when the VT flipped must
        // not be read as the answer), then wait — BOUNDED — for a line.
        let _ = linux_uapi::tcflush_in(fd);
        let rev = linux_uapi::poll_one(fd, linux_uapi::POLLIN, ANSWER_TIMEOUT_MS).map_err(|_| DenyReason::ReadFail)?;
        if rev & (linux_uapi::POLLHUP | linux_uapi::POLLERR | linux_uapi::POLLNVAL) != 0 {
            return Err(DenyReason::ReadFail);
        }
        if rev & linux_uapi::POLLIN == 0 {
            return Err(DenyReason::Timeout);
        }
        let mut buf = [0u8; 256];
        let n = vt.read(&mut buf).map_err(|_| DenyReason::ReadFail)?;
        if n == 0 {
            return Err(DenyReason::ReadFail);
        }
        let raw = String::from_utf8_lossy(&buf[..n]);
        Ok(Answer { line: raw.lines().next().unwrap_or("").to_string() })
    }

    fn restore(&mut self) {
        if let Some(ctl) = self.ctl.as_ref() {
            let cfd = ctl.as_raw_fd();
            if self.saved_vt != 0 {
                let _ = linux_uapi::vt_activate(cfd, self.saved_vt as u32);
                let _ = linux_uapi::vt_disallocate(cfd, CONSENT_VT);
            }
        }
        if let Some(mut c) = self.sak_child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.saved_vt = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- a scripted console: fake ONLY the VT/SAK/keyboard boundary; the decision logic is real ----
    struct MockConsole {
        arm: Result<(), DenyReason>,
        vt: Result<(), DenyReason>,
        render: Result<(), DenyReason>,
        answer: Result<Answer, DenyReason>,
        rendered: Option<String>,
        restores: u32,
    }

    impl MockConsole {
        fn happy(answer: &str) -> MockConsole {
            MockConsole {
                arm: Ok(()),
                vt: Ok(()),
                render: Ok(()),
                answer: Ok(Answer { line: answer.to_string() }),
                rendered: None,
                restores: 0,
            }
        }
    }

    impl Console for MockConsole {
        fn arm_sak(&mut self) -> Result<(), DenyReason> {
            self.arm
        }
        fn take_vt(&mut self) -> Result<(), DenyReason> {
            self.vt
        }
        fn render(&mut self, screen: &str) -> Result<(), DenyReason> {
            self.rendered = Some(screen.to_string());
            self.render
        }
        fn read_answer(&mut self) -> Result<Answer, DenyReason> {
            self.answer.clone()
        }
        fn restore(&mut self) {
            self.restores += 1;
        }
    }

    fn a_pending() -> Pending {
        Pending { nonce: 123_456_789, uid: 1000, pid: 4242, starttime: 999 }
    }

    // A read-only grant plan (bare `y`) and a high-authority export plan (typed code), built through the
    // real precheck would need a live record; for the pure orchestration/binding tests we drive with a
    // minimal hand-made plan via the test-only constructor on AuthorityPlan.
    fn ro_grant_plan() -> AuthorityPlan {
        bench_plane::AuthorityPlan::test_plan("grant", "media", false, false)
    }
    fn rw_grant_plan() -> AuthorityPlan {
        bench_plane::AuthorityPlan::test_plan("grant", "media", true, false)
    }
    fn trifecta_plan() -> AuthorityPlan {
        bench_plane::AuthorityPlan::test_plan("network", "media", false, true)
    }

    // ---- sanitizer ----
    #[test]
    fn sanitizer_escapes_control_c1_and_non_ascii() {
        assert_eq!(sanitize_value("ok/path-1_2.txt"), "ok/path-1_2.txt");
        assert_eq!(sanitize_value("keep spaces"), "keep spaces");
        // ANSI CSI escape (color) — the \x1b must be neutralized.
        assert_eq!(sanitize_value("\x1b[31mred"), "\\u{001B}[31mred");
        // newline / tab must not forge lines or columns.
        assert_eq!(sanitize_value("a\nb\tc"), "a\\u{000A}b\\u{0009}c");
        // C1 NEL.
        assert_eq!(sanitize_value("x\u{0085}y"), "x\\u{0085}y");
        // U+202E right-to-left override (path reordering spoof).
        assert_eq!(sanitize_value("a\u{202E}b"), "a\\u{202E}b");
        // Cyrillic homoglyph 'с' (U+0441) that looks like ASCII 'c'.
        assert_eq!(sanitize_value("proje\u{0441}ts"), "proje\\u{0441}ts");
        // DEL.
        assert_eq!(sanitize_value("a\x7fb"), "a\\u{007F}b");
    }

    // ---- nonce: MUST terminate (read_exact 16 bytes, not read-to-EOF on the infinite device) ----
    #[test]
    fn mint_nonce_terminates_and_varies() {
        let a = mint_nonce().expect("urandom readable in the test env");
        let b = mint_nonce().expect("urandom readable in the test env");
        assert_ne!(a, b, "two draws from urandom must differ");
    }

    // ---- confirmation code ----
    #[test]
    fn conf_code_is_six_digits_and_deterministic() {
        let c = conf_code(123_456_789);
        assert_eq!(c.len(), 6);
        assert!(c.chars().all(|c| c.is_ascii_digit()));
        assert_eq!(conf_code(123_456_789), c);
        assert_eq!(conf_code(1_000_000), "000000");
        assert_eq!(conf_code(7), "000007");
    }

    // ---- trifecta predicate (the pure bench-semantics rule) ----
    #[test]
    fn trifecta_truth_table() {
        use bench_plane::trifecta_after as t;
        assert!(!t(false, false, false, false));
        assert!(!t(true, false, false, false)); // fs only
        assert!(!t(false, true, false, false)); // net only
        assert!(t(true, false, false, true)); // has fs, adding net
        assert!(t(false, true, true, false)); // has net, adding fs
        assert!(t(true, true, false, false)); // already both
    }

    // ---- the apply gate: binding / swap / replay matrix ----
    #[test]
    fn decide_apply_accepts_exact_match() {
        let p = a_pending();
        let live = LiveFacts { starttime: Some(999), peer_alive: true };
        assert!(matches!(decide_apply(&p, &live, &Answer { line: "y".into() }, None), ApplyDecision::Apply));
    }

    #[test]
    fn decide_apply_denies_wrong_answer() {
        let p = a_pending();
        let live = LiveFacts { starttime: Some(999), peer_alive: true };
        assert!(matches!(
            decide_apply(&p, &live, &Answer { line: "n".into() }, None),
            ApplyDecision::Deny(DenyReason::Declined)
        ));
        // a bare `y` must NOT satisfy a high-authority verb that requires the code.
        assert!(matches!(
            decide_apply(&p, &live, &Answer { line: "y".into() }, Some("424242")),
            ApplyDecision::Deny(DenyReason::Declined)
        ));
    }

    #[test]
    fn decide_apply_accepts_correct_code() {
        let p = a_pending();
        let live = LiveFacts { starttime: Some(999), peer_alive: true };
        assert!(matches!(
            decide_apply(&p, &live, &Answer { line: "424242".into() }, Some("424242")),
            ApplyDecision::Apply
        ));
    }

    #[test]
    fn decide_apply_denies_starttime_change_and_peer_gone_and_disconnect() {
        let p = a_pending();
        // PID reused: same pid, different start-time.
        let changed = LiveFacts { starttime: Some(1000), peer_alive: true };
        assert!(matches!(
            decide_apply(&p, &changed, &Answer { line: "y".into() }, None),
            ApplyDecision::Deny(DenyReason::StarttimeChanged)
        ));
        // peer vanished (no /proc entry).
        let gone = LiveFacts { starttime: None, peer_alive: true };
        assert!(matches!(
            decide_apply(&p, &gone, &Answer { line: "y".into() }, None),
            ApplyDecision::Deny(DenyReason::PeerGone)
        ));
        // socket disconnected mid-ceremony.
        let dead = LiveFacts { starttime: Some(999), peer_alive: false };
        assert!(matches!(
            decide_apply(&p, &dead, &Answer { line: "y".into() }, None),
            ApplyDecision::Deny(DenyReason::Disconnected)
        ));
    }

    // ---- orchestration: the fail-closed matrix over a scripted console ----
    fn run_with(console: &mut MockConsole, plan: &AuthorityPlan, live_starttime: Option<u64>, peer_alive: bool) -> (CeremonyResult, u32) {
        let p = a_pending();
        let committed = std::cell::Cell::new(0u32);
        let res = run_ceremony(
            plan,
            &p,
            console,
            |_pid| live_starttime,
            || peer_alive,
            || {
                committed.set(committed.get() + 1);
                0
            },
        );
        (res, committed.get())
    }

    #[test]
    fn ceremony_happy_path_commits_once_and_restores() {
        let plan = ro_grant_plan();
        let mut c = MockConsole::happy("y");
        let (res, commits) = run_with(&mut c, &plan, Some(999), true);
        assert!(matches!(res.outcome, Outcome::Approved));
        assert_eq!(res.rc, 0);
        assert_eq!(commits, 1, "commit runs exactly once on approval");
        assert_eq!(c.restores, 1, "VT restored");
        assert!(c.rendered.as_ref().unwrap().contains("AUTHORITY CHANGE REQUEST"));
    }

    #[test]
    fn ceremony_high_authority_requires_the_code() {
        let plan = rw_grant_plan(); // read-write grant → code required
        let code = conf_code(a_pending().nonce);
        // wrong answer (bare y) denies, commit never runs.
        let mut c = MockConsole::happy("y");
        let (res, commits) = run_with(&mut c, &plan, Some(999), true);
        assert!(matches!(res.outcome, Outcome::Denied(DenyReason::Declined)));
        assert_eq!(commits, 0);
        // correct code approves.
        let mut c2 = MockConsole::happy(&code);
        let (res2, commits2) = run_with(&mut c2, &plan, Some(999), true);
        assert!(matches!(res2.outcome, Outcome::Approved));
        assert_eq!(commits2, 1);
        assert!(c2.rendered.as_ref().unwrap().contains(&code), "the code is shown on the VT");
    }

    #[test]
    fn ceremony_fails_closed_at_every_stage() {
        let plan = ro_grant_plan();
        // no SAK
        let mut c = MockConsole { arm: Err(DenyReason::NoSak), ..MockConsole::happy("y") };
        let (res, commits) = run_with(&mut c, &plan, Some(999), true);
        assert!(matches!(res.outcome, Outcome::Denied(DenyReason::NoSak)));
        assert_eq!(commits, 0);
        assert_eq!(c.restores, 1);
        // no VT
        let mut c = MockConsole { vt: Err(DenyReason::NoVt), ..MockConsole::happy("y") };
        let (res, commits) = run_with(&mut c, &plan, Some(999), true);
        assert!(matches!(res.outcome, Outcome::Denied(DenyReason::NoVt)));
        assert_eq!(commits, 0);
        // render fail
        let mut c = MockConsole { render: Err(DenyReason::RenderFail), ..MockConsole::happy("y") };
        let (res, _) = run_with(&mut c, &plan, Some(999), true);
        assert!(matches!(res.outcome, Outcome::Denied(DenyReason::RenderFail)));
        // read timeout
        let mut c = MockConsole { answer: Err(DenyReason::Timeout), ..MockConsole::happy("y") };
        let (res, commits) = run_with(&mut c, &plan, Some(999), true);
        assert!(matches!(res.outcome, Outcome::Denied(DenyReason::Timeout)));
        assert_eq!(commits, 0);
    }

    #[test]
    fn ceremony_denies_on_starttime_swap_even_with_yes() {
        let plan = ro_grant_plan();
        let mut c = MockConsole::happy("y");
        // human said yes, but the peer's start-time changed since we bound it (PID reuse).
        let (res, commits) = run_with(&mut c, &plan, Some(1234), true);
        assert!(matches!(res.outcome, Outcome::Denied(DenyReason::StarttimeChanged)));
        assert_eq!(commits, 0, "a swapped peer never reaches commit");
        assert_eq!(c.restores, 1);
    }

    #[test]
    fn trifecta_plan_renders_the_warning() {
        let plan = trifecta_plan();
        let mut c = MockConsole::happy("y");
        let (_res, _) = run_with(&mut c, &plan, Some(999), true);
        assert!(c.rendered.as_ref().unwrap().contains("WARNING"), "lethal-trifecta warning shown");
    }

    // ---- cooldown escalation (pure curve + the stateful map with a unique uid) ----
    #[test]
    fn cooldown_curve_escalates_and_caps() {
        assert_eq!(cooldown_secs(1), 10);
        assert_eq!(cooldown_secs(2), 20);
        assert_eq!(cooldown_secs(3), 40);
        assert_eq!(cooldown_secs(4), 80);
        assert_eq!(cooldown_secs(5), 160);
        assert_eq!(cooldown_secs(6), 300); // 320 capped to 300
        assert_eq!(cooldown_secs(99), 300);
    }

    #[test]
    fn cooldown_arms_and_expires() {
        let uid = 909_090; // unique to this test to avoid cross-test contamination of the static map
        assert_eq!(cooldown_active(uid, "grant", 1000), None);
        note_deny(uid, "grant", 1000);
        assert_eq!(cooldown_active(uid, "grant", 1005), Some(1010)); // 10s cooldown active
        assert_eq!(cooldown_active(uid, "grant", 1010), None); // expired at the boundary
        note_deny(uid, "grant", 1010); // second strike → 20s
        assert_eq!(cooldown_active(uid, "grant", 1015), Some(1030));
        // a different verb-family is independent.
        assert_eq!(cooldown_active(uid, "export", 1015), None);
    }
}
