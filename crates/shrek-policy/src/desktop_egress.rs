//! desktop_egress — the sealed named-egress-profile table for the DESKTOP SESSION (ADR-007, S1).
//!
//! Sibling of [`crate::egress`], sharing its exact types ([`EgressProfile`]/[`EgressRule`]/[`Proto`]),
//! but answering a DIFFERENT question: not "what may an agent sandbox reach" (agentd names a profile,
//! gatekeeperd resolves it per-sandbox) but "what may the deny-by-default DESKTOP session (`dev`, uid
//! 1000) reach, once the human BLESSES it". Same seal discipline: compiled-in typed literals,
//! dm-verity-sealed in `/usr`, resolved by a root supervisor that trusts none of uid 1000's numbers —
//! the load-bearing invariant carried over from egress.rs adjustment B: the bless request crosses a
//! PROFILE NAME, never a destination, so a compromised uid-1000 process can *name* `weather` but can
//! never manufacture `evil.example:443` (an unknown name resolves to `None`, fail-closed).
//!
//! What this module is (S1): pure policy DATA + fail-closed lookup + the tier/shape predicates the
//! S2 supervisor consumes. It does NO I/O, NO DNS, NO nft. Enforcement (the single baked
//! `shrek_desktop_egress` nft table, the DoT re-pin client, the `/run` pinned projection) is the
//! supervisor's, exactly as egress.rs defers netns/veth/nftables to gatekeeperd.
//!
//! Three shape distinctions this table encodes (ADR-007 §6), surfaced as EXPLICIT predicates rather
//! than overloaded rule-sets (mirroring egress.rs's `grants_swamp_query` idiom — an explicit check,
//! never "some carve-out exists"):
//!
//!   * BASELINE vs BLESSED ([`is_baseline_profile`]). `desktop-ntp`/`desktop-updates` are
//!     system-service egress (matched at the nft layer by the SERVICE uid — timesyncd, the updater —
//!     always-allowed so the box keeps time and updates with zero user action). `weather`/
//!     `web-browsing` are uid-1000 grants, deny-until-blessed.
//!   * PRE-PINNED vs DoT-RESOLVED ([`is_prepinned_profile`]). `desktop-ntp`'s rule hosts are DOTTED-
//!     QUAD LITERALS the supervisor seals into `@ntp_pinned` VERBATIM — no resolution at all. This is
//!     what breaks the DoT↔clock circular dependency (ADR-007 §5 `[R2-MF-C]`): DoT needs a valid TLS
//!     clock, the clock needs NTP, so NTP must reach its endpoints WITHOUT resolving anything. Every
//!     other profile's host is a NAME the supervisor resolves over sealed-DoT after the clock is good.
//!   * PINNABLE vs BROAD ([`is_broad_profile`]). `web-browsing` reaches ARBITRARY hosts, so it is
//!     unpinnable by construction: its rule-set is empty NOT because it "reaches nothing" (that is the
//!     `none`/stub meaning) but because its breadth is enforced by an nft cgroup-scope accept on
//!     `shrek-browser.slice`, not by a pinned destination set. `resolve_desktop` still returns it so
//!     the console ceremony can validate the name; `is_broad_profile` is how the supervisor learns to
//!     install the cgroup rule instead of a pin.

use crate::egress::{EgressProfile, EgressRule, Proto};

// ---- the sealed desktop profiles ----------------------------------------------------------------
// Authored as typed literals, sealed by compilation (extend the catalog HERE). Unlike the agent
// table (tcp-only), the desktop baseline includes UDP: SNTP is udp/123.

// desktop-ntp — the system time-sync baseline. SEALED LITERAL IPs, never resolved [R2-MF-C]. These
// are Cloudflare's `time.cloudflare.com` anycast addresses (Q6a), pinned here as dotted-quad literals
// so timesyncd corrects the clock at boot with ZERO name resolution — the bootstrap that lets the
// supervisor's sealed-DoT re-pin of `weather`/`updates` succeed afterward (clock-good → DoT → weather).
// SNTP verifies no cert against a name, so pinning the IP loses nothing. The shipped `timesyncd.conf`
// sets `NTP=` to these same IPs; `@ntp_pinned` is populated from them at seal/build time and NEVER
// re-pinned. udp/123.
const DESKTOP_NTP: &[EgressRule] = &[
    EgressRule { host: "162.159.200.1", proto: Proto::Udp, port: 123 },
    EgressRule { host: "162.159.200.123", proto: Proto::Udp, port: 123 },
];

// desktop-updates — the layer-store / sysupdate fetch baseline. STUB: the actual update source
// endpoint is not yet defined in-repo (ADR-007 Q6b, TBD). Intentionally EMPTY so it renders an INERT
// allow (empty set matches nothing = fail-closed, NOT accept-all) — the box does not silently reach
// anything for "updates" until S5 wires a real endpoint here. Baseline-tier (matched by the updater's
// service uid), so blessing it is never a uid-1000 action; it simply reaches nothing yet.
const DESKTOP_UPDATES: &[EgressRule] = &[];

// weather — the one keyless weather API (Q6c: open-meteo, no account, privacy-forward). User-blessed,
// deny-until-blessed. A NAME (not a literal IP): the supervisor resolves it over sealed-DoT at bless
// time and on the bounded re-pin, seals the result into `@weather_pinned`, and publishes it to the
// widget via the `/run/shrek/egress/pinned` map so the widget dials `--resolve api.open-meteo.com:443:
// <ip>` with TLS hostname verification intact. https/tcp:443, exactly like the agent internet profiles.
const WEATHER: &[EgressRule] = &[
    EgressRule { host: "api.open-meteo.com", proto: Proto::Tcp, port: 443 },
];

// web-browsing — the BROAD grant. Deliberately EMPTY of pin rules because a browser reaches arbitrary
// hosts: it CANNOT be pin-scoped, so its breadth is enforced by an nft cgroup accept on the browser's
// own `shrek-browser.slice` (ADR-007 §7, Q7), NOT by a destination set here. Present in the table so
// `resolve_desktop("web-browsing")` succeeds (the console ceremony validates the name), and flagged by
// `is_broad_profile` so the supervisor installs the cgroup rule rather than treating an empty set as
// "reaches nothing". This empty-but-broad state is WHY `is_broad_profile` exists as an explicit
// predicate — `is_empty()` alone cannot distinguish it from `desktop-updates`'s stub.
const WEB_BROWSING: &[EgressRule] = &[];

/// THE sealed desktop-egress table — the single compiled-in source of desktop-session egress policy.
/// The S2 supervisor resolves names against this; there is no runtime or writable source. Deny-by-
/// default: a name absent here resolves to `None` and the supervisor installs no allow / refuses the
/// bless. Order is not significant (lookup is by name); grouped baseline-then-blessed for legibility.
pub const DESKTOP_EGRESS_PROFILES: &[EgressProfile] = &[
    // baseline (system-service egress, always-on, zero user action):
    EgressProfile { name: "desktop-ntp", rules: DESKTOP_NTP },
    EgressProfile { name: "desktop-updates", rules: DESKTOP_UPDATES },
    // user-blessed (deny until the human blesses):
    EgressProfile { name: "weather", rules: WEATHER },
    EgressProfile { name: "web-browsing", rules: WEB_BROWSING },
];

/// Resolve a desktop profile NAME to its sealed destination set. STRICT + FAIL-CLOSED, identical
/// discipline to [`crate::egress::resolve`]: an unrecognized name ⇒ `None` (the supervisor MUST refuse
/// the bless / install no rule), never "allow all", never a silent substitution. Case-sensitive.
pub fn resolve_desktop(name: &str) -> Option<&'static EgressProfile> {
    DESKTOP_EGRESS_PROFILES.iter().find(|p| p.name == name)
}

/// Baseline-tier predicate: is this a SYSTEM-SERVICE egress profile (always-allowed by service uid),
/// as opposed to a uid-1000 user-blessed one? Baseline profiles (`desktop-ntp`, `desktop-updates`) are
/// on out of the box with no user action; everything else is deny-until-blessed. Explicit check, not
/// a rule-shape inference — the tier is policy, not derivable from destinations.
pub fn is_baseline_profile(name: &str) -> bool {
    matches!(name, "desktop-ntp" | "desktop-updates")
}

/// Pre-pinned predicate: are this profile's rule hosts SEALED LITERAL IPs the supervisor uses VERBATIM
/// (no resolution), rather than names to resolve over DoT? True only for `desktop-ntp` — the clock
/// bootstrap that must not depend on DNS (`[R2-MF-C]`). The supervisor can also confirm this by
/// parsing each host as an `Ipv4Addr`; the predicate names the intent so a future literal-IP baseline
/// is an explicit choice, not an accident of a host string that happens to parse.
pub fn is_prepinned_profile(name: &str) -> bool {
    name == "desktop-ntp"
}

/// Broad-grant predicate: does blessing this profile lift the deny for a BROAD, unpinnable scope
/// (enforced by an nft cgroup accept), rather than adding a pinned destination? True only for
/// `web-browsing`. This is the high-consequence tier that takes the full console ceremony (ADR-007
/// §3), and it is why a `web-browsing` profile with an empty rule-set is NOT "reaches nothing".
pub fn is_broad_profile(name: &str) -> bool {
    name == "web-browsing"
}

/// The bless TIER a desktop profile requires (ADR-007 §3, Q3). This is SEALED policy — the supervisor
/// consults it to decide what a uid-1000 socket request may grant, and the tier is authored here, not
/// inferred by the daemon. `None` for an unknown name (fail-closed: no tier ⇒ no bless).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlessTier {
    /// System-service egress, always on, NOT user-blessable (`desktop-ntp`, `desktop-updates`). A
    /// socket bless of a baseline profile is meaningless and refused.
    Baseline,
    /// One-click grant over the uid-1000 socket, no console ceremony — a pinned, bounded destination
    /// (`weather`). The ONLY tier the S2 supervisor admits over the socket.
    OneClick,
    /// High-consequence BROAD grant (`web-browsing`): only through the full console ceremony (S4),
    /// NEVER over the uid-1000 socket. A compromised uid 1000 must not be able to open arbitrary
    /// browsing by naming a profile.
    Ceremony,
}

/// Resolve a desktop profile to its sealed [`BlessTier`]. `None` ⇒ unknown profile (fail-closed).
pub fn bless_tier(name: &str) -> Option<BlessTier> {
    resolve_desktop(name)?;
    Some(if is_baseline_profile(name) {
        BlessTier::Baseline
    } else if is_broad_profile(name) {
        BlessTier::Ceremony
    } else {
        BlessTier::OneClick
    })
}

/// The Tier-B admission gate the supervisor applies to EVERY socket bless/re-pin: is this profile
/// grantable one-click over the uid-1000 socket? True ONLY for a sealed, non-baseline, non-broad
/// profile (`weather` today). Baseline is always-on (nothing to bless); broad requires the console
/// ceremony (S4). SO_PEERCRED proves the requester is uid 1000, but authorization to grant still rests
/// entirely on THIS sealed rule — identity is not authority.
pub fn admits_socket_bless(name: &str) -> bool {
    bless_tier(name) == Some(BlessTier::OneClick)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn resolve_known_desktop_profiles() {
        assert_eq!(resolve_desktop("desktop-ntp").unwrap().rules.len(), 2);
        assert_eq!(resolve_desktop("weather").unwrap().rules.len(), 1);
        assert!(resolve_desktop("desktop-updates").unwrap().is_empty());
        assert!(resolve_desktop("web-browsing").unwrap().is_empty());
    }

    #[test]
    fn bless_tier_and_socket_admission() {
        // The sealed Tier-B rule the S2 supervisor enforces on every socket bless.
        assert_eq!(bless_tier("weather"), Some(BlessTier::OneClick));
        assert_eq!(bless_tier("web-browsing"), Some(BlessTier::Ceremony));
        assert_eq!(bless_tier("desktop-ntp"), Some(BlessTier::Baseline));
        assert_eq!(bless_tier("desktop-updates"), Some(BlessTier::Baseline));
        assert_eq!(bless_tier("evil"), None); // unknown ⇒ fail-closed
        // Only the one-click tier is admissible over the uid-1000 socket: NOT baseline (always-on),
        // NOT broad (ceremony, S4), NOT unknown. This is the whole "identity != authority" gate.
        assert!(admits_socket_bless("weather"));
        assert!(!admits_socket_bless("web-browsing"));
        assert!(!admits_socket_bless("desktop-ntp"));
        assert!(!admits_socket_bless("desktop-updates"));
        assert!(!admits_socket_bless("evil"));
        assert!(!admits_socket_bless(""));
    }

    #[test]
    fn unknown_desktop_name_fails_closed_to_none() {
        // Same fail-closed core as the agent table: a name we didn't author grants NOTHING and is not
        // silently mapped to a default. The supervisor turns this None into a refused bless.
        assert!(resolve_desktop("evil-exfil").is_none());
        assert!(resolve_desktop("").is_none());
        assert!(resolve_desktop("WEATHER").is_none()); // exact match; case-sensitive
        assert!(resolve_desktop("open-meteo").is_none()); // the endpoint is not the profile name
    }

    #[test]
    fn desktop_ntp_is_sealed_literal_ips_udp123() {
        let n = resolve_desktop("desktop-ntp").unwrap();
        // TWO Cloudflare anycast IPs, udp/123, sealed as LITERALS (the clock bootstrap [R2-MF-C]).
        assert_eq!(n.rules.len(), 2);
        assert!(n.allows("162.159.200.1", Proto::Udp, 123));
        assert!(n.allows("162.159.200.123", Proto::Udp, 123));
        // Every host is a parseable IPv4 literal — the supervisor seals these VERBATIM, no DoT.
        for r in n.rules {
            assert!(r.host.parse::<Ipv4Addr>().is_ok(), "desktop-ntp host must be a literal IP: {:?}", r.host);
            assert_eq!(r.proto, Proto::Udp, "SNTP is udp");
            assert_eq!(r.port, 123);
        }
        assert!(is_prepinned_profile("desktop-ntp"));
        assert!(is_baseline_profile("desktop-ntp"));
        assert!(!is_broad_profile("desktop-ntp"));
        // Deny-by-default around it: not tcp, not the ntp port over the wrong proto, not a rogue IP.
        assert!(!n.allows("162.159.200.1", Proto::Tcp, 123));
        assert!(!n.allows("6.6.6.6", Proto::Udp, 123));
    }

    #[test]
    fn weather_is_one_https_name_dot_resolved() {
        let w = resolve_desktop("weather").unwrap();
        assert_eq!(w.rules.len(), 1);
        assert!(w.allows("api.open-meteo.com", Proto::Tcp, 443));
        // A NAME, not a literal IP: the supervisor DoT-resolves it (NOT pre-pinned), after the clock
        // is good. So it must NOT parse as an IPv4 literal, and is neither baseline nor broad.
        assert!(w.rules[0].host.parse::<Ipv4Addr>().is_err(), "weather host must be a NAME, not a literal IP");
        assert!(!is_prepinned_profile("weather"));
        assert!(!is_baseline_profile("weather"));
        assert!(!is_broad_profile("weather"));
        // Deny-by-default: no plaintext :80, no wildcard/suffix, no other host.
        assert!(!w.allows("api.open-meteo.com", Proto::Tcp, 80));
        assert!(!w.allows("open-meteo.com", Proto::Tcp, 443));
        assert!(!w.allows("api.open-meteo.co", Proto::Tcp, 443));
    }

    #[test]
    fn web_browsing_is_broad_not_reaches_nothing() {
        // web-browsing has an EMPTY rule-set, but that is NOT the `none`/stub meaning. It is BROAD:
        // enforced by an nft cgroup accept, flagged by is_broad_profile. The distinction is the whole
        // reason the predicate exists — is_empty() alone can't tell it apart from desktop-updates.
        let b = resolve_desktop("web-browsing").unwrap();
        assert!(b.is_empty(), "web-browsing pins nothing (broad, cgroup-enforced)");
        assert!(is_broad_profile("web-browsing"));
        assert!(!is_baseline_profile("web-browsing"));
        assert!(!is_prepinned_profile("web-browsing"));
        // No pinned rule grants anything — its breadth is NOT expressed as destinations here.
        assert!(!b.allows("example.com", Proto::Tcp, 443));
        // And nothing ELSE is broad: the stub, the baseline, and unknown names are not.
        assert!(!is_broad_profile("desktop-updates"));
        assert!(!is_broad_profile("weather"));
        assert!(!is_broad_profile("desktop-ntp"));
        assert!(!is_broad_profile("nonexistent"));
    }

    #[test]
    fn empty_profiles_render_inert_allows_not_accept_all() {
        // THE S1 test (ADR-007 §11): an empty set / empty profile grants NOTHING — never accept-all.
        // Both desktop-updates (stub, Q6b) and web-browsing (broad) are empty; neither allows a packet
        // through the pin path. This is what makes the baked nft named sets fail-closed when empty.
        for name in ["desktop-updates", "web-browsing"] {
            let p = resolve_desktop(name).unwrap();
            assert!(p.is_empty(), "{name} is empty");
            assert!(!p.allows("anything", Proto::Tcp, 443), "{name} must allow nothing via pins");
            assert!(!p.allows("162.159.200.1", Proto::Udp, 123), "{name} must allow nothing via pins");
        }
    }

    #[test]
    fn desktop_updates_is_baseline_stub() {
        // Stub, not broad: baseline-tier (updater service uid), empty until S5 wires Q6b's endpoint.
        assert!(is_baseline_profile("desktop-updates"));
        assert!(!is_broad_profile("desktop-updates"));
        assert!(!is_prepinned_profile("desktop-updates"));
        assert!(resolve_desktop("desktop-updates").unwrap().is_empty());
    }

    #[test]
    fn desktop_and_agent_tables_are_separate_vocabularies() {
        // The two sealed tables must not bleed: a desktop name is unknown to the agent resolver and
        // vice versa, so neither can borrow the other's destinations by naming across the boundary.
        assert!(crate::egress::resolve("weather").is_none());
        assert!(crate::egress::resolve("desktop-ntp").is_none());
        assert!(resolve_desktop("github-https").is_none());
        assert!(resolve_desktop("model-anthropic").is_none());
        assert!(resolve_desktop("swamp-query").is_none());
    }

    #[test]
    fn table_integrity_no_dup_names_valid_rules() {
        // Unique names (resolve_desktop would otherwise shadow silently).
        for (i, a) in DESKTOP_EGRESS_PROFILES.iter().enumerate() {
            for b in &DESKTOP_EGRESS_PROFILES[i + 1..] {
                assert_ne!(a.name, b.name, "duplicate desktop profile name {:?}", a.name);
            }
            assert!(!a.name.is_empty(), "empty desktop profile name");
        }
        // Well-formed rules: non-empty host, port in [1, 65535], host lowercase (literal IPs are
        // lowercase-invariant, names are authored lowercase like the agent table).
        for p in DESKTOP_EGRESS_PROFILES {
            for r in p.rules {
                assert!(!r.host.is_empty(), "empty host in {:?}", p.name);
                assert!(r.port >= 1, "zero port in {:?}", p.name);
                assert_eq!(r.host, r.host.to_ascii_lowercase(), "host must be lowercase: {:?}", r.host);
            }
        }
    }

    #[test]
    fn exactly_one_baseline_is_prepinned_and_exactly_one_profile_is_broad() {
        // Guardrails on the shape predicates so a future edit can't quietly make two profiles broad
        // (two cgroup broad-accepts) or drop the ntp literal-IP bootstrap.
        let prepinned: Vec<_> = DESKTOP_EGRESS_PROFILES.iter().filter(|p| is_prepinned_profile(p.name)).collect();
        let broad: Vec<_> = DESKTOP_EGRESS_PROFILES.iter().filter(|p| is_broad_profile(p.name)).collect();
        assert_eq!(prepinned.len(), 1, "exactly one pre-pinned (literal-IP) profile: desktop-ntp");
        assert_eq!(prepinned[0].name, "desktop-ntp");
        assert_eq!(broad.len(), 1, "exactly one broad profile: web-browsing");
        assert_eq!(broad[0].name, "web-browsing");
        // Baseline set is exactly the two system-service profiles.
        let baseline: Vec<_> = DESKTOP_EGRESS_PROFILES.iter().filter(|p| is_baseline_profile(p.name)).collect();
        assert_eq!(baseline.len(), 2, "exactly two baseline profiles");
    }
}
