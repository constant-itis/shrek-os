//! confirmed — the ROOT-ONLY ceremony-commit surface + the shared raw/browser reconcile engine (S4).
//!
//! The one-click socket path (S2/S3) only ever admits Tier-B (`weather`). The high-consequence tier —
//! `web-browsing` (broad egress) and any user-authored raw `host:proto:port` destination — is granted
//! ONLY through the full console SAK/VT ceremony, which lives in gatekeeperd (`consent.rs`). On a
//! CONFIRMED ceremony, gatekeeperd (running as root) execs one of these `confirmed-*` verbs to persist +
//! apply the bless. This module is that verb engine.
//!
//! The trust boundary (MF-1): "root exec = trusted" is true only in that root already owns the `0700`
//! store; it does NOT excuse validation. The destination string ORIGINATED from a uid-1000 socket
//! request — the ceremony proves human INTENT, not that the string is well-formed — so every verb here:
//!   * refuses unless `geteuid() == 0` (a uid-1000 caller can never bypass the ceremony by execing it);
//!   * re-checks `bless_tier(profile) == Ceremony` for `confirmed-bless` (else the ceremony verb would
//!     become a second front door for `weather`/baseline and the tier matrix would drift);
//!   * re-parses a raw triple through the ONE sealed grammar ([`parse_raw_triple`]);
//!   * takes the store lock ([`store::lock_store`]) so it can never interleave with the running daemon
//!     (MF-4 — torn `/run` projections / lost `@raw_pinned` elements);
//!   * writes the durable record INTENT-FIRST (MF-3), before resolve/apply, so a ceremony approved
//!     before the clock/network converges persists as "blessed, waiting" and heals on the next
//!     [`reconcile`] rather than vanishing (which would force the human to redo the whole SAK ceremony).
//!
//! gatekeeperd invokes the ABSOLUTE sealed path with a CLEARED environment and argv taken from the
//! rendered ceremony plan (never re-read from the wire), so `oracle-env`'s store redirect (compiled out
//! of the shipped build anyway) can never be reached via an inherited env.

use std::net::Ipv4Addr;
use std::path::Path;
use std::time::Duration;

use shrek_policy::desktop_egress::{bless_tier, is_broad_profile, parse_raw_triple, BlessTier};
use shrek_policy::egress::Proto;

use crate::apply::{self, ApplyError, NftExec, ShellNft};
use crate::store::{self, BlessRecord};

// ---- raw resolution seam (mirrors supervisor::PinResolver) ---------------------------------------

/// Resolve a RAW host to IPv4s. Unlike [`crate::supervisor::PinResolver`] (which resolves a profile's
/// SEALED hosts), a raw host is user-authored — but it is ceremony-blessed (the human confirmed the
/// exact name), and the resolver itself is the sealed DoT client against sealed upstream IPs, so the
/// query name being user-chosen leaks no uid-1000 name-resolution authority into the pin path.
pub trait RawResolver {
    fn resolve_host(&mut self, host: &str) -> Result<Vec<Ipv4Addr>, String>;
}

/// Production raw resolver: an IPv4 literal is pinned VERBATIM (no DoT, like `desktop-ntp`); a name goes
/// over the sealed DoT client (never `resolved`/NM/`resolv.conf`/`getaddrinfo`).
pub struct DotRawResolver;
impl RawResolver for DotRawResolver {
    fn resolve_host(&mut self, host: &str) -> Result<Vec<Ipv4Addr>, String> {
        if let Ok(ip) = host.parse::<Ipv4Addr>() {
            return Ok(vec![ip]);
        }
        crate::dot::resolve_over_dot(host, 0x4a77, Duration::from_secs(5))
            .map(|v| v.into_iter().collect())
            .map_err(|e| e.to_string())
    }
}

// ---- browser cgroup convention (MF-7) -----------------------------------------------------------

/// The deterministic cgroup path + ancestor level for the browser slice under the desktop user's
/// session. Deterministic from the uid, so the `browser-up` socket verb needs NO path on the wire
/// (wire = verb only, per the S2 discipline) — a uid-1000-supplied path would be a spoof of "which
/// cgroup is the browser," so the daemon derives it and never trusts one.
///
/// The launch path is `systemd-run --user --scope --slice=shrekbrowser.slice` (S6a), which places the
/// scope INSIDE the user manager's own cgroup, so the real, measured path is FOUR components:
///   `user.slice/user-<uid>.slice/user@<uid>.service/shrekbrowser.slice`  →  nft ancestor level 4
/// (nft `socket cgroupv2 level N` is 1-indexed from the root and N == the component count). The slice
/// name is DELIBERATELY un-hyphenated: systemd treats `-` as a cgroup hierarchy separator, so a
/// `shrek-browser.slice` would be forced under a synthetic `shrek.slice` parent (5 components, fragile);
/// `shrekbrowser.slice` lands flat. `user@<uid>.service` is the user manager's own cgroup — stable and
/// identical in the sealed-VM autologin session and on the installed product. S6b asserts the LIVE path
/// equals this constant before trusting the matcher.
pub fn browser_cgroup(uid: u32) -> (String, u32) {
    (format!("user.slice/user-{uid}.slice/user@{uid}.service/shrekbrowser.slice"), 4)
}

/// Does the browser slice exist yet? The cgroupv2 rule can only be inserted once the slice does (nft
/// resolves the path to an id at load), so a bless before launch stays pending until `browser-up`.
pub fn browser_slice_exists(uid: u32) -> bool {
    let (path, _) = browser_cgroup(uid);
    Path::new("/sys/fs/cgroup").join(&path).is_dir()
}

// ---- the shared reconcile engine (used by both boot reconcile and the confirmed verbs) -----------

/// Re-resolve every blessed RAW destination and reconcile `@raw_pinned` to their UNION (MF-5). Rewrites
/// the resolved cache so the `/run` state view shows which raw entries are live vs still pending. A
/// per-entry resolve failure keeps the OTHER entries (that entry simply stays out of the union/cache =
/// "blessed, waiting"). Element-only + fail-closed inside [`apply::apply_raw`]. Returns (pinned, pending).
pub fn reconcile_raw(
    store: &Path,
    exec: &mut dyn NftExec,
    resolver: &mut dyn RawResolver,
    at: u64,
) -> Result<(usize, usize), ApplyError> {
    let mut desired: Vec<(Ipv4Addr, Proto, u16)> = Vec::new();
    let mut cache: Vec<store::RawPin> = Vec::new();
    let mut pending = 0usize;
    for t in store::list_raw(store) {
        // "literal → verbatim, no resolution" is POLICY (like desktop-ntp), independent of the transport,
        // so it lives here — every resolver path pins a dotted-quad host without a lookup.
        let resolved = if t.is_ip_literal() {
            t.host.parse::<Ipv4Addr>().map(|ip| vec![ip]).map_err(|_| "bad literal".to_string())
        } else {
            resolver.resolve_host(&t.host)
        };
        match resolved {
            Ok(ips) if !ips.is_empty() => {
                for ip in &ips {
                    desired.push((*ip, t.proto, t.port));
                }
                cache.push(store::RawPin { triple: t, pins: ips, resolved: at });
            }
            _ => pending += 1, // keep the intent; it heals on a later reconcile
        }
    }
    // Reconcile the live set to the union, THEN persist the cache (so a mid-apply crash never claims a
    // pin that isn't live).
    let present = apply::apply_raw(exec, &desired)?;
    let _ = store::write_raw_pins(store, &cache);
    Ok((present.len(), pending))
}

/// If `web-browsing` is blessed AND the browser slice exists AND the rules are not already present,
/// install the cgroup accept pair. Boot/idempotent: called from reconcile and `browser-up`. A slice that
/// doesn't exist yet leaves the record legibly blessed-but-not-live (heals at browser launch via
/// `browser-up`) — NOT a fault.
pub fn reconcile_web_browsing(
    store: &Path,
    exec: &mut dyn NftExec,
    uid: u32,
) -> Result<bool, ApplyError> {
    if store::load_bless(store, "web-browsing").is_none() {
        return Ok(false); // not blessed → nothing to install
    }
    if !browser_slice_exists(uid) {
        return Ok(false); // blessed but slice not up yet → pending, heals on browser-up
    }
    // Already installed? (idempotent — don't double-insert on every reconcile.)
    let listing = exec.run(&apply::list_chain()).map_err(ApplyError::Nft)?;
    if !apply::parse_browser_handles(&listing).is_empty() {
        return Ok(false);
    }
    let (path, level) = browser_cgroup(uid);
    apply::install_browser_rules(exec, &path, level)?;
    Ok(true)
}

// ---- the confirmed-* verb engine ----------------------------------------------------------------

/// Persist + apply a CONFIRMED ceremony bless of a broad profile (`web-browsing`). Intent-first: the
/// durable record is written first (tier `ceremony`), then the browser rule is installed IFF the slice
/// exists now (it usually won't at bless time — the browser isn't running — so it stays pending and
/// `browser-up` installs it at launch; not a fault). Root-only, lock-held by the caller.
fn confirmed_bless_profile(store: &Path, run: &Path, uid: u32, profile: &str, at: u64) -> i32 {
    if bless_tier(profile) != Some(BlessTier::Ceremony) || !is_broad_profile(profile) {
        eprintln!("egressd confirmed-bless: {profile} is not a ceremony-tier profile");
        return 2;
    }
    // intent-first (MF-3): the record persists even if the rule install below defers.
    if let Err(e) = store::write_bless(
        store,
        &BlessRecord { profile: profile.to_string(), tier: "ceremony".into(), blessed: at },
    ) {
        eprintln!("egressd confirmed-bless: store: {e}");
        return 1;
    }
    let _ = store::clear_fault(store, profile);
    let mut exec = ShellNft;
    let installed = reconcile_web_browsing(store, &mut exec, uid).unwrap_or(false);
    reproject(store, run);
    let _ = crate::supervisor::append_event(run, at, "bless", profile, if installed { "enabled" } else { "enabled (pending browser)" });
    println!("OK confirmed-bless {profile} {}", if installed { "live" } else { "pending" });
    0
}

/// Revoke a CONFIRMED ceremony profile bless: remove the durable record AND tear down live enforcement
/// (MF-5 — the browser rule must go, or the panel reads "Disabled" while broad egress persists).
fn confirmed_unbless_profile(store: &Path, run: &Path, profile: &str, at: u64) -> i32 {
    if bless_tier(profile) != Some(BlessTier::Ceremony) {
        eprintln!("egressd confirmed-unbless: {profile} is not a ceremony-tier profile");
        return 2;
    }
    let mut exec = ShellNft;
    // Tear down enforcement FIRST; only drop the record if that succeeds, so we never leave "revoked in
    // the store, still allowed in the kernel."
    if let Err(e) = apply::uninstall_browser_rules(&mut exec) {
        let _ = store::write_fault(store, profile, store::FaultKind::ApplyFail, &format!("{e:?}"), at);
        reproject(store, run);
        eprintln!("egressd confirmed-unbless: teardown: {e:?}");
        return 1;
    }
    let _ = store::remove_bless(store, profile);
    let _ = store::clear_fault(store, profile);
    reproject(store, run);
    let _ = crate::supervisor::append_event(run, at, "unbless", profile, "revoked");
    println!("OK confirmed-unbless {profile}");
    0
}

/// Add a CONFIRMED raw destination. Intent-first (MF-3): the triple is stored before the DoT resolve, so
/// a ceremony approved before the network is up persists as "blessed, waiting" and heals on reconcile.
fn confirmed_add_raw(store: &Path, run: &Path, wire: &str, at: u64) -> i32 {
    let t = match parse_raw_triple(wire) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("egressd confirmed-add-raw: {e}");
            return 2;
        }
    };
    if let Err(e) = store::add_raw(store, &t) {
        eprintln!("egressd confirmed-add-raw: store: {e}");
        return 1;
    }
    let mut exec = ShellNft;
    let mut resolver = DotRawResolver;
    match reconcile_raw(store, &mut exec, &mut resolver, at) {
        Ok((_, pending)) => {
            reproject(store, run);
            // Was THIS entry pinned, or is it waiting? (Re-read the cache — reconcile is a union.)
            let live = store::list_raw_pins(store).iter().any(|r| r.triple == t);
            let _ = crate::supervisor::append_event(run, at, "add-raw", "raw", if live { "pinned" } else { "pending" });
            if live {
                println!("OK confirmed-add-raw {} live", t.to_wire());
                0
            } else {
                // Persisted (intent-first), just not yet resolvable — legibly pending, not a hard fail.
                println!("OK confirmed-add-raw {} pending ({pending} pending total)", t.to_wire());
                0
            }
        }
        Err(e) => {
            reproject(store, run);
            let _ = crate::supervisor::append_event(run, at, "add-raw", "raw", "apply-failed");
            eprintln!("egressd confirmed-add-raw: apply: {e:?}");
            1 // record persisted (intent-first); reconcile will retry
        }
    }
}

/// Remove a CONFIRMED raw destination: drop the intent, then reconcile `@raw_pinned` to the UNION of the
/// REMAINING entries (MF-5 — never a per-entry element delete, which would kill a shared tuple).
fn confirmed_remove_raw(store: &Path, run: &Path, wire: &str, at: u64) -> i32 {
    let t = match parse_raw_triple(wire) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("egressd confirmed-remove-raw: {e}");
            return 2;
        }
    };
    if let Err(e) = store::remove_raw(store, &t) {
        eprintln!("egressd confirmed-remove-raw: store: {e}");
        return 1;
    }
    let mut exec = ShellNft;
    let mut resolver = DotRawResolver;
    match reconcile_raw(store, &mut exec, &mut resolver, at) {
        Ok(_) => {
            reproject(store, run);
            let _ = crate::supervisor::append_event(run, at, "remove-raw", "raw", "revoked");
            println!("OK confirmed-remove-raw {}", t.to_wire());
            0
        }
        Err(e) => {
            reproject(store, run);
            eprintln!("egressd confirmed-remove-raw: apply: {e:?}");
            1
        }
    }
}

fn reproject(store: &Path, run: &Path) {
    let _ = store::project_pinned(store, run);
    let _ = store::project_state(store, run);
}

/// `egressd confirmed-<bless|unbless|add-raw|remove-raw> <arg>` — the root-only ceremony-commit CLI
/// gatekeeperd execs. Gate: `geteuid()==0` (fail-closed). `now` is injected for the oracle; production
/// stamps wall-clock. Takes the store lock around the whole operation (MF-4).
pub fn cli(args: &[String], now: u64) -> i32 {
    if crate::uapi::geteuid() != 0 {
        eprintln!("egressd confirmed-*: refused — root only (this is the ceremony-commit surface)");
        return 2;
    }
    let verb = args.first().map(String::as_str).unwrap_or("");
    let arg = match args.get(1) {
        Some(a) => a.as_str(),
        None => {
            eprintln!("egressd confirmed-{verb}: needs an argument");
            return 2;
        }
    };
    let store = store::store_dir();
    let run = store::run_dir();
    if let Err(e) = store::ensure_store(&store) {
        eprintln!("egressd confirmed-{verb}: ensure store: {e}");
        return 1;
    }
    let uid = crate::supervisor::desktop_uid();
    let _lock = store::lock_store(&store).ok(); // best-effort; held for the whole op (MF-4)
    match verb {
        "bless" => confirmed_bless_profile(&store, &run, uid, arg, now),
        "unbless" => confirmed_unbless_profile(&store, &run, arg, now),
        "add-raw" => confirmed_add_raw(&store, &run, arg, now),
        "remove-raw" => confirmed_remove_raw(&store, &run, arg, now),
        _ => {
            eprintln!("egressd confirmed-*: unknown verb {verb:?}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Minimal nft double: canned `list set` reply, records mutations, never fails.
    struct Exec {
        live: String,
        cmds: Vec<Vec<String>>,
    }
    impl NftExec for Exec {
        fn run(&mut self, cmd: &crate::apply::NftCmd) -> Result<String, String> {
            self.cmds.push(cmd.0.clone());
            if cmd.0.first().map(String::as_str) == Some("list") {
                return Ok(self.live.clone());
            }
            Ok(String::new())
        }
    }

    /// Fake raw resolver: a per-host canned answer (Err ⇒ resolve failure).
    struct FakeRaw(HashMap<String, Result<Vec<Ipv4Addr>, String>>);
    impl RawResolver for FakeRaw {
        fn resolve_host(&mut self, host: &str) -> Result<Vec<Ipv4Addr>, String> {
            self.0.get(host).cloned().unwrap_or_else(|| Err("no answer".into()))
        }
    }

    fn tmp(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let base = std::env::var("CARGO_TARGET_TMPDIR").unwrap_or_else(|_| "/tmp".into());
        let d = std::path::PathBuf::from(base)
            .join(format!("confirmed-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        store::ensure_store(&d).unwrap();
        d
    }

    fn raw(s: &str) -> shrek_policy::desktop_egress::RawTriple {
        parse_raw_triple(s).unwrap()
    }

    #[test]
    fn reconcile_raw_pins_the_resolvable_and_keeps_the_rest_pending() {
        let d = tmp("recon");
        store::add_raw(&d, &raw("good.example.com:tcp:443")).unwrap();
        store::add_raw(&d, &raw("offline.example.org:udp:8883")).unwrap();
        store::add_raw(&d, &raw("203.0.113.7:tcp:8443")).unwrap(); // literal → verbatim, no resolver

        let mut answers = HashMap::new();
        answers.insert("good.example.com".to_string(), Ok(vec![Ipv4Addr::new(1, 2, 3, 4)]));
        answers.insert("offline.example.org".to_string(), Err("offline".into()));
        // NB: the literal is pinned verbatim, so the resolver is never asked for it.
        let mut resolver = FakeRaw(answers);
        let mut exec = Exec { live: String::new(), cmds: vec![] };

        let (pinned, pending) = reconcile_raw(&d, &mut exec, &mut resolver, 100).unwrap();
        assert_eq!(pending, 1, "the offline host stays pending");
        assert_eq!(pinned, 2, "the resolvable name + the literal are in @raw_pinned");
        // element-only, never a rule/flush.
        assert!(exec.cmds.iter().all(|c| c[0] != "add" || c[1] == "element"));
        assert!(!exec.cmds.iter().any(|c| c.iter().any(|t| t == "rule" || t == "flush")));
        // the cache reflects exactly the two live entries; the offline one is absent (pending in the view).
        let cache = store::list_raw_pins(&d);
        assert_eq!(cache.len(), 2);
        assert!(cache.iter().any(|r| r.triple == raw("good.example.com:tcp:443")));
        assert!(cache.iter().any(|r| r.triple == raw("203.0.113.7:tcp:8443")));
        assert!(!cache.iter().any(|r| r.triple.host == "offline.example.org"));
    }

    #[test]
    fn reconcile_raw_removal_recomputes_union_never_per_entry_delete() {
        // Two entries share the SAME resolved tuple; removing one must NOT drop the shared element (MF-5).
        let d = tmp("union");
        store::add_raw(&d, &raw("a.example.com:tcp:443")).unwrap();
        store::add_raw(&d, &raw("b.example.com:tcp:443")).unwrap();
        let shared = Ipv4Addr::new(5, 6, 7, 8);
        let mut answers = HashMap::new();
        answers.insert("a.example.com".to_string(), Ok(vec![shared]));
        answers.insert("b.example.com".to_string(), Ok(vec![shared]));
        let mut resolver = FakeRaw(answers);

        // First converge: @raw_pinned = { 5.6.7.8 . tcp . 443 } (union of both).
        let mut exec = Exec { live: String::new(), cmds: vec![] };
        reconcile_raw(&d, &mut exec, &mut resolver, 1).unwrap();

        // Now remove `a` and reconcile against a LIVE set that already has the shared tuple.
        store::remove_raw(&d, &raw("a.example.com:tcp:443")).unwrap();
        let mut exec2 = Exec { live: "elements = { 5.6.7.8 . tcp . 443 }".into(), cmds: vec![] };
        let (pinned, _) = reconcile_raw(&d, &mut exec2, &mut resolver, 2).unwrap();
        assert_eq!(pinned, 1, "b still pins the shared tuple");
        // The shared element is STILL in the desired union (b needs it), so NO delete is emitted.
        assert!(!exec2.cmds.iter().any(|c| c[0] == "delete"), "shared element must survive a's removal");
    }

    #[test]
    fn confirmed_bless_profile_gates_on_ceremony_tier() {
        let d = tmp("bless");
        let run = d.join("run");
        // weather is one-click, NOT ceremony → refused here (the ceremony verb is not a weather front door).
        assert_eq!(confirmed_bless_profile(&d, &run, 1000, "weather", 5), 2);
        assert!(store::load_bless(&d, "weather").is_none());
        // web-browsing is ceremony-tier → persisted (tier=ceremony), pending until browser-up (slice absent).
        assert_eq!(confirmed_bless_profile(&d, &run, 1000, "web-browsing", 9), 0);
        let rec = store::load_bless(&d, "web-browsing").unwrap();
        assert_eq!(rec.tier, "ceremony");
        let state = std::fs::read_to_string(store::state_map(&run)).unwrap();
        assert!(state.contains("profile web-browsing tier=ceremony blessed=1"), "{state}");
    }

    #[test]
    fn confirmed_add_raw_is_intent_first_when_resolve_fails() {
        // MF-3: a ceremony approved before the network is up must PERSIST the intent, not vanish.
        let d = tmp("intent");
        let run = d.join("run");
        // add_raw persists first; reconcile_raw (production DotRawResolver) will fail to resolve offline —
        // but the intent must remain so a later reconcile heals it.
        store::add_raw(&d, &raw("unresolvable.invalid:tcp:443")).unwrap();
        assert_eq!(store::list_raw(&d).len(), 1, "intent persisted before any resolve");
        // and the state view shows it pending (pins=-), never dropped.
        let mut exec = Exec { live: String::new(), cmds: vec![] };
        let mut resolver = FakeRaw(HashMap::new()); // every host → Err
        let (_, pending) = reconcile_raw(&d, &mut exec, &mut resolver, 1).unwrap();
        assert_eq!(pending, 1);
        assert_eq!(store::list_raw(&d).len(), 1, "intent still there after a failed resolve");
    }
}
