//! egress — the sealed named-egress-profile table. Phase-5 slice-3, step-2 (checkpoint #2563).
//!
//! `profile-name → the CLOSED set of {host:proto:port} destinations that profile may reach` — and
//! nothing else (deny-by-default). It is the network analog of [`crate::tier`]'s matrix: compiled
//! in, dm-verity-sealed in `/usr`, resolved INDEPENDENTLY by gatekeeperd. The load-bearing
//! invariant (#2563 adjustment B):
//!
//! > The construction request carries a profile IDENTITY, not destinations. agentd may only pass a
//! > profile NAME (`--egress-profile github-https`); the `host:proto:port` set is authored ONLY
//! > here. So agentd can *name* a profile but can never manufacture `evil.example:443` and trick
//! > gatekeeperd into blessing it.
//!
//! Boundaries (deliberately NOT here — they are gatekeeperd's, because it constructs): resolving a
//! `host` to concrete PINNED IPs (DNS A-records, pre-resolved at construction, IPv4-only), and the
//! netns/veth/nftables enforcement + ready-barrier (proven end-to-end by the C4 oracle in
//! scripts/egress-plane-repro.sh). PER-REQUEST dynamic destinations (agentd supplying its own
//! `host:proto:port` instead of a name) are also deferred — they await the later sealed grant
//! protocol. This module is pure policy DATA + a fail-closed lookup; it does no I/O and no DNS.

/// L4 protocol of a sealed egress grant. Egress enforcement is IPv4-only: gatekeeperd pre-resolves
/// each grant's host to pinned A-records and shells to a sealed nftables rule matching dst-IP +
/// proto + dport. `Udp` exists for grammar completeness (`host:proto:port`), but NO sealed profile
/// uses it today — DNS egress is deliberately absent (hosts are pre-resolved into `/etc/hosts`,
/// there is no in-sandbox resolver), and the enforcement path proven in the oracle is tcp-dport.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub enum Proto {
    Tcp,
    Udp,
}

impl Proto {
    pub fn label(self) -> &'static str {
        match self {
            Proto::Tcp => "tcp",
            Proto::Udp => "udp",
        }
    }
}

/// One sealed egress destination — a single `host:proto:port` triple (#2563 adjustment A). `host` is
/// a DNS NAME: the sealed, stable IDENTITY of the destination, authored lowercase. It is resolved to
/// concrete pinned IPs by gatekeeperd at construction — never here — because the IP is a runtime
/// detail that must not leak into the seal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EgressRule {
    pub host: &'static str,
    pub proto: Proto,
    pub port: u16,
}

/// A named, sealed egress profile: the CLOSED allow-list a workload naming this profile may reach.
/// Compiled in and dm-verity-sealed exactly like the tier matrix, so gatekeeperd resolves the set
/// from sealed policy and trusts none of agentd's numbers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EgressProfile {
    pub name: &'static str,
    pub rules: &'static [EgressRule],
}

impl EgressProfile {
    /// Deny-by-default membership: is this exact `(host, proto, port)` destination granted? Only an
    /// EXACT match is allowed — no port ranges, no wildcards, no host suffixes. gatekeeperd uses
    /// this to independently confirm a destination against the sealed set before installing a rule.
    pub fn allows(&self, host: &str, proto: Proto, port: u16) -> bool {
        self.rules
            .iter()
            .any(|r| r.host == host && r.proto == proto && r.port == port)
    }

    /// The empty profile grants nothing — a legitimate, auditable "named but reaches nothing" state,
    /// distinct from an UNKNOWN name (which resolves to `None`; see [`resolve`]).
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

// ---- the sealed profiles ------------------------------------------------------------------------
// Authored here as typed literals (not parsed from strings) so the policy is sealed by compilation.
// Extend the catalog HERE; sealing the *source* of this list (vs the compiled table) is a later
// slice (#2563). IPv4-only, tcp-only, no DNS egress. Most destinations are https/tcp:443; the
// coding-agent model endpoint (`model-local`) is the one plaintext tcp:8100 (a single LAN model
// service, docs/phase6-slice2-coder-agent.md §3) — the enforcement path is proto+dport, so the port
// is immaterial to it.

const GITHUB_HTTPS: &[EgressRule] = &[
    EgressRule { host: "github.com", proto: Proto::Tcp, port: 443 },
    EgressRule { host: "codeload.github.com", proto: Proto::Tcp, port: 443 },
    EgressRule { host: "objects.githubusercontent.com", proto: Proto::Tcp, port: 443 },
];

const RUST_CRATES: &[EgressRule] = &[
    EgressRule { host: "crates.io", proto: Proto::Tcp, port: 443 },
    EgressRule { host: "static.crates.io", proto: Proto::Tcp, port: 443 },
    EgressRule { host: "index.crates.io", proto: Proto::Tcp, port: 443 },
];

// The coding-agent's model endpoint: exactly ONE destination. `shrek-model` is the sealed, stable
// NAME gatekeeperd pre-resolves to a pinned A-record at construction — the acceptance gate points it
// at a canned responder, the `--live` smoke at the real LAN model. Same seal, only the resolution
// differs. One destination keeps the lethal-trifecta surface (agents.md §8) as small as an egressing
// coding session can be.
const MODEL_LOCAL: &[EgressRule] = &[
    EgressRule { host: "shrek-model", proto: Proto::Tcp, port: 8100 },
];

// The coding-agent's HOSTED-model endpoint (Anthropic), Phase-6 slice-3. The box reaches EXACTLY ONE
// destination: `shrek-model-proxy` — the broker-side authenticated egress proxy (crates/model-proxy,
// security-model §7), NOT api.anthropic.com. The proxy holds the API key + injects auth + terminates
// TLS to Anthropic; the sandbox speaks PLAINTEXT to the proxy and never holds the secret or reaches
// Anthropic directly (breaks the lethal trifecta: the box has untrusted-read + egress but NO secret).
// So the sealed dst here is the LOCAL proxy on plaintext tcp:8200 — the key + TLS live outside this
// policy, in the proxy. `--provider anthropic` (crates/coder) speaks the messages API to this dst.
const MODEL_ANTHROPIC: &[EgressRule] = &[
    EgressRule { host: "shrek-model-proxy", proto: Proto::Tcp, port: 8200 },
];

/// THE sealed egress table — the single, compiled-in source of egress policy. gatekeeperd resolves
/// names against this; there is no runtime or writable source. Deny-by-default: a name absent here
/// resolves to `None` and gatekeeperd fails the C-net construct closed.
pub const EGRESS_PROFILES: &[EgressProfile] = &[
    // The canonical empty grant: a C-net request may name it to assert "reaches nothing" explicitly.
    EgressProfile { name: "none", rules: &[] },
    EgressProfile { name: "github-https", rules: GITHUB_HTTPS },
    EgressProfile { name: "rust-crates", rules: RUST_CRATES },
    EgressProfile { name: "model-local", rules: MODEL_LOCAL },
    EgressProfile { name: "model-anthropic", rules: MODEL_ANTHROPIC },
];

/// Resolve a profile NAME to its sealed destination set. STRICT + FAIL-CLOSED, mirroring
/// [`crate::tier::Tier::parse`]: an unrecognized name ⇒ `None`, and gatekeeperd MUST deny egress for
/// it (install no allow rule / refuse the C-net construct). It is NEVER treated as "allow all" or
/// silently substituted. Unlike trust/caps there is no fail-HIGH here: for egress the safe default
/// is the EMPTY set, and `None` already denotes "no destinations", so mapping garbage to some
/// profile would only ever be less safe.
pub fn resolve(name: &str) -> Option<&'static EgressProfile> {
    EGRESS_PROFILES.iter().find(|p| p.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_known_profiles() {
        assert_eq!(resolve("github-https").unwrap().rules.len(), 3);
        assert_eq!(resolve("rust-crates").unwrap().rules.len(), 3);
        assert!(resolve("none").unwrap().is_empty());
    }

    #[test]
    fn model_local_is_one_plaintext_destination() {
        let m = resolve("model-local").unwrap();
        assert_eq!(m.rules.len(), 1, "model-local must reach exactly one destination");
        assert!(m.allows("shrek-model", Proto::Tcp, 8100));
        // Deny-by-default around it: not a wildcard host, not another port, not the https default.
        assert!(!m.allows("shrek-model", Proto::Tcp, 443));
        assert!(!m.allows("shrek-model", Proto::Tcp, 80));
        assert!(!m.allows("evil.example", Proto::Tcp, 8100));
    }

    #[test]
    fn model_anthropic_reaches_only_the_broker_proxy() {
        // The hosted-model profile reaches exactly ONE dst — the broker proxy — NEVER Anthropic
        // directly and NEVER any other port/host. The key + TLS live in the proxy, outside this policy.
        let m = resolve("model-anthropic").unwrap();
        assert_eq!(m.rules.len(), 1, "model-anthropic must reach exactly one destination (the proxy)");
        assert!(m.allows("shrek-model-proxy", Proto::Tcp, 8200));
        // The box must NOT be able to reach Anthropic (or anything) directly.
        assert!(!m.allows("api.anthropic.com", Proto::Tcp, 443));
        assert!(!m.allows("shrek-model-proxy", Proto::Tcp, 443));
        assert!(!m.allows("shrek-model", Proto::Tcp, 8100)); // not the local-model dst either
    }

    #[test]
    fn unknown_name_fails_closed_to_none() {
        // The whole point of adjustment B: a name we didn't author grants NOTHING, and is NOT
        // silently mapped to some default. gatekeeperd turns this None into a refusal.
        assert!(resolve("evil-exfil").is_none());
        assert!(resolve("").is_none());
        assert!(resolve("GITHUB-HTTPS").is_none()); // exact match; names are case-sensitive
    }

    #[test]
    fn allows_is_exact_and_deny_by_default() {
        let gh = resolve("github-https").unwrap();
        assert!(gh.allows("github.com", Proto::Tcp, 443));
        // deny-by-default: wrong port / proto / host, or a suffix/substring, are all refused.
        assert!(!gh.allows("github.com", Proto::Tcp, 80));
        assert!(!gh.allows("github.com", Proto::Udp, 443));
        assert!(!gh.allows("evil.example", Proto::Tcp, 443));
        assert!(!gh.allows("gist.github.com", Proto::Tcp, 443)); // not a wildcard
        assert!(!gh.allows("github.co", Proto::Tcp, 443)); // no prefix/suffix matching
    }

    #[test]
    fn empty_profile_allows_nothing() {
        let none = resolve("none").unwrap();
        assert!(!none.allows("github.com", Proto::Tcp, 443));
        assert!(!none.allows("anything", Proto::Tcp, 443));
    }

    #[test]
    fn table_integrity_no_dup_names_valid_rules() {
        // Every profile name is unique (resolve() would otherwise shadow silently).
        for (i, a) in EGRESS_PROFILES.iter().enumerate() {
            for b in &EGRESS_PROFILES[i + 1..] {
                assert_ne!(a.name, b.name, "duplicate profile name {:?}", a.name);
            }
            assert!(!a.name.is_empty(), "empty profile name");
        }
        // Every rule is well-formed: non-empty host, port in [1, 65535].
        for p in EGRESS_PROFILES {
            for r in p.rules {
                assert!(!r.host.is_empty(), "empty host in {:?}", p.name);
                assert!(r.port >= 1, "zero port in {:?}", p.name);
                assert_eq!(r.host, r.host.to_ascii_lowercase(), "host must be lowercase: {:?}", r.host);
            }
        }
    }

    #[test]
    fn current_policy_is_ipv4_tcp_only() {
        // Documents the enforcement reality (oracle-proven tcp-dport, IPv4-only, no DNS egress):
        // no sealed profile grants UDP today. If this ever changes, the enforcement path must too.
        for p in EGRESS_PROFILES {
            for r in p.rules {
                assert_eq!(r.proto, Proto::Tcp, "non-tcp rule {:?} in {:?}", r.host, p.name);
            }
        }
    }

    #[test]
    fn proto_labels() {
        assert_eq!(Proto::Tcp.label(), "tcp");
        assert_eq!(Proto::Udp.label(), "udp");
    }
}
