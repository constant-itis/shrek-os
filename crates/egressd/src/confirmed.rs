//! confirmed — the SHARED raw/browser reconcile engine for the ceremony tier (S4; S6 fix #4 redesign).
//!
//! The one-click socket path (S2/S3) only ever admits Tier-B (`weather`). The high-consequence tier —
//! `web-browsing` (broad egress) and any user-authored raw `host:proto:port` destination — is granted
//! ONLY through the full console SAK/VT ceremony, which lives in gatekeeperd (`consent.rs`). On a
//! CONFIRMED ceremony, gatekeeperd (running as root) RELAYS the confirmed op over the root-gated egressd
//! socket; the running daemon commits it via `supervisor::Supervisor::confirmed_*`. The daemon is the
//! sole nft mutator — there is no longer a transient root `confirmed-*` CLI process editing the table
//! under the broker's cap umbrella, so gatekeeperd needs no `CAP_NET_ADMIN`.
//!
//! This module is the ENGINE those daemon handlers (and the boot `reconcile`) call, NOT a command
//! surface. The trust boundary (MF-1) it enforces: the destination string ORIGINATED from a uid-1000
//! socket request — the ceremony proves human INTENT, not that the string is well-formed — so the daemon
//! re-checks `bless_tier == Ceremony`, re-parses the raw triple through the ONE sealed grammar
//! ([`parse_raw_triple`]), holds the store lock (MF-4), and writes the durable record INTENT-FIRST
//! (MF-3) before resolve/apply, so a ceremony approved before the clock/network converges persists as
//! "blessed, waiting" and heals on the next reconcile. The functions HERE are the resolve/apply steps
//! those handlers compose; the tier/identity gates live at the socket boundary (`authorize`).

use std::net::Ipv4Addr;
use std::path::Path;
use std::time::Duration;

use shrek_policy::egress::Proto;

use crate::apply::{self, ApplyError, NftExec};
use crate::store::{self};

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

// ---- ceremony-commit engine note --------------------------------------------------------------
//
// The `confirmed-*` VERBS themselves live in the daemon now (`supervisor::Supervisor::confirmed_*`),
// reached over the root-gated socket (ADR-007 S6 fix #4 redesign). The old `egressd confirmed-*` CLI —
// a transient root process that ran nft under gatekeeperd's cap umbrella and forced `CAP_NET_ADMIN`
// into the broker's bounding set — is GONE. This module keeps only the SHARED engine those daemon
// handlers (and the boot `reconcile`) call: `reconcile_raw`, `reconcile_web_browsing`, the raw resolver,
// and the browser-cgroup convention. Single nft mutator = the running daemon; gatekeeperd only relays.

#[cfg(test)]
mod tests {
    use super::*;
    use shrek_policy::desktop_egress::parse_raw_triple;
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

    // NB: the `confirmed-bless`/`confirmed-add-raw` VERB behavior (ceremony-tier gate, intent-first
    // persistence, the root-peer authorization) now lives on the daemon and is tested in
    // `supervisor::tests` (the socket is the trust boundary). Here we test the shared reconcile ENGINE.

    #[test]
    fn confirmed_add_raw_is_intent_first_when_resolve_fails() {
        // MF-3: a ceremony approved before the network is up must PERSIST the intent, not vanish.
        let d = tmp("intent");
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
