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

    /// Swamp slice-2: does this profile grant the in-sandbox swamp-query destination
    /// (`SWAMP_QUERY_HOST:SWAMP_QUERY_PORT`)? This is THE swamp-capability predicate — gatekeeperd gates
    /// the session-identity transaction on it, so a model-only or non-swamp egress mints nothing (the
    /// coder self-mints). Deliberately an exact destination check, not "reaches any identity-preserving
    /// host", so a future identity-preserving dst that is NOT the swamp broker can never trip it.
    pub fn grants_swamp_query(&self) -> bool {
        self.allows(SWAMP_QUERY_HOST, Proto::Tcp, SWAMP_QUERY_PORT)
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

// A Debian apt WORKSHOP bench (ADR-002/003): reach the Debian archive to `apt-get install`. ONE host by
// design — deb.debian.org is a direct Fastly-CDN service (NOT the old httpredir 302-redirector) serving
// /debian, /debian-security AND /debian-updates, so the seed's deb822 sources point ALL suites at it and
// no second pin is needed (security.debian.org is a separately-rotating round-robin — deliberately NOT
// listed). HTTPS/tcp:443 only: on a shared-CDN IP allow-list, plaintext :80 would let a `Host:` header
// reach any Fastly customer, so every profile in this catalog is uniformly 443. apt validates the cert
// against the SNI name (deb.debian.org), never the spawn-time-pinned IP, exactly like `github-https`.
// SEALED-EGRESS INVARIANT: a bench holding this (or any internet) profile must NEVER receive a secret via
// grant/env/export — the shared-CDN aperture (any Fastly site is reachable to a workload crafting its own
// TLS) is contained only by the no-secrets rule, not by the network layer (host-side apt broker = post-MVP).
const DEBIAN_APT: &[EgressRule] = &[
    EgressRule { host: "deb.debian.org", proto: Proto::Tcp, port: 443 },
];

// A pip/PyPI WORKSHOP bench (ADR-002/003, sibling of `debian-apt`): reach the Python Package Index to
// `pip install`. TWO hosts by design — pypi.org fronts the Simple/JSON index (package metadata + which
// files exist, incl. pip's own version self-check on pypi.org/pypi/…/json), and files.pythonhosted.org is
// the SEPARATE CDN that serves the actual wheels/sdists (and PEP 658 `.metadata` sidecars). Both are
// Fastly-fronted — the SAME shared-CDN aperture as `debian-apt`/`github-https` — so https/tcp:443 only: a
// plaintext :80 to a shared-CDN IP allow-list would let a `Host:` header reach any Fastly customer. pip
// validates the cert against the SNI name (never the spawn-time-pinned IP), and vendors its own certifi
// bundle so there is no CA-fetch host. `pypi.python.org` (a 301 relic) and `test.pypi.org` are deliberately
// NOT listed. SEALED-EGRESS INVARIANT (identical to debian-apt): a bench holding this (or any internet)
// profile must NEVER receive a secret via grant/env/export — the shared-CDN aperture is contained only by
// the no-secrets rule, not by the network layer.
const PYPI_HTTPS: &[EgressRule] = &[
    EgressRule { host: "pypi.org", proto: Proto::Tcp, port: 443 },
    EgressRule { host: "files.pythonhosted.org", proto: Proto::Tcp, port: 443 },
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

// The coding-agent's SUBSCRIPTION-model endpoint (Claude via the logged-in official CLI), Phase-6
// slice-4. Same shape as `model-anthropic` — the box reaches EXACTLY ONE destination — but the broker
// here is `crates/claude-broker` (`shrek-claude-cli`), NOT the api-key TLS proxy. That broker shells
// the ALREADY-authenticated `claude -p` CLI: Shrek never handles the subscription OAuth credential at
// all (the CLI owns its own login), so no secret enters this policy OR the box. Deliberately a DISTINCT
// profile name from `model-anthropic` (not a reused name pointed at a different backend): the box must
// EXPLICITLY select the subscription path, and the api-key proxy path stays byte-for-byte unchanged —
// no silent backend swap on one egress name (docs/phase6-slice3-provider-abstraction.md §2). Plaintext
// tcp:8300; the box speaks the same messages-API wire, the broker translates to the CLI broker-side.
const MODEL_CLAUDE_CLI: &[EgressRule] = &[
    EgressRule { host: "shrek-claude-cli", proto: Proto::Tcp, port: 8300 },
];

// The coding-agent's SECOND subscription-model endpoint (Codex via the logged-in official CLI),
// Phase-6 slice-6. IDENTICAL shape to `model-claude-cli` — the box reaches EXACTLY ONE destination —
// but the broker is `crates/codex-broker` (`shrek-codex-cli`), a sibling of claude-broker that shells
// the ALREADY-authenticated `codex exec` CLI under an unprivileged bubblewrap confinement. Shrek
// handles NO subscription credential (the CLI owns its own login). A THIRD distinct profile name (not
// `model-anthropic`, not `model-claude-cli`): selecting Codex is an explicit, separately-sealed choice
// — the no-silent-backend-swap invariant (docs/phase6-slice3-provider-abstraction.md §2) now proven to
// GENERALIZE across two subscription providers. Plaintext tcp:8301 (distinct port from claude's 8300);
// the box speaks the same messages-API wire, the broker adapts it to the Codex wire broker-side.
const MODEL_CODEX_CLI: &[EgressRule] = &[
    EgressRule { host: "shrek-codex-cli", proto: Proto::Tcp, port: 8301 },
];

// The IN-SANDBOX SWAMP QUERY endpoint (Phase-6 Swamp slice-2, docs/phase6-swamp-slice2-broker-routed-
// find.md). Same one-destination shape as the model brokers, but this broker (`crates/swamp-broker`,
// `shrek-swamp-broker`) does NOT reach the network — it bridges an in-sandbox `swamp_find` to the
// host-side `swampd` unix socket, letting a T2 coding session query the semantic index WITHOUT a hole
// in its wall. A FOURTH distinct name (not any `model-*`): selecting swamp query is an explicit sealed
// choice, and the broker's `cont_ip→session` binding is what authorizes forwarding — the box's egress
// authority (this profile) is orthogonal to its query authority (the sealed authority record swampd
// resolves). Plaintext tcp:8400 (distinct from model 8200/8300/8301); the caller's `cont_ip` must NOT
// be masqueraded to this dst (net_plane Mechanism A) so the broker can bind the session by transport.
/// The sealed swamp-query destination — the in-sandbox `swamp_find` broker (`crates/swamp-broker`).
/// Exposed as named constants so a session's swamp-query CAPABILITY is recognized by an EXPLICIT check
/// against this exact `(host, port)` — never a proxy (e.g. "some carve-out exists"). gatekeeperd gates
/// the whole session-identity transaction (mint H + authority record + `cont_ip→H` binding +
/// `SHREK_SESSION` injection) on [`EgressProfile::grants_swamp_query`], and `net_plane`'s masquerade
/// carve-out reads the same host, so there is one source of truth for the swamp broker's identity.
pub const SWAMP_QUERY_HOST: &str = "shrek-swamp-broker";
pub const SWAMP_QUERY_PORT: u16 = 8400;

const SWAMP_QUERY: &[EgressRule] = &[
    EgressRule { host: SWAMP_QUERY_HOST, proto: Proto::Tcp, port: SWAMP_QUERY_PORT },
];

/// THE sealed egress table — the single, compiled-in source of egress policy. gatekeeperd resolves
/// names against this; there is no runtime or writable source. Deny-by-default: a name absent here
/// resolves to `None` and gatekeeperd fails the C-net construct closed.
pub const EGRESS_PROFILES: &[EgressProfile] = &[
    // The canonical empty grant: a C-net request may name it to assert "reaches nothing" explicitly.
    EgressProfile { name: "none", rules: &[] },
    EgressProfile { name: "github-https", rules: GITHUB_HTTPS },
    EgressProfile { name: "debian-apt", rules: DEBIAN_APT },
    EgressProfile { name: "pypi-https", rules: PYPI_HTTPS },
    EgressProfile { name: "rust-crates", rules: RUST_CRATES },
    EgressProfile { name: "model-local", rules: MODEL_LOCAL },
    EgressProfile { name: "model-anthropic", rules: MODEL_ANTHROPIC },
    EgressProfile { name: "model-claude-cli", rules: MODEL_CLAUDE_CLI },
    EgressProfile { name: "model-codex-cli", rules: MODEL_CODEX_CLI },
    EgressProfile { name: "swamp-query", rules: SWAMP_QUERY },
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
    fn debian_apt_is_one_https_host_deny_by_default() {
        let d = resolve("debian-apt").unwrap();
        // ONE host by design (deb.debian.org fronts every suite); https/tcp:443 only.
        assert_eq!(d.rules.len(), 1, "debian-apt must reach exactly one host");
        assert!(d.allows("deb.debian.org", Proto::Tcp, 443));
        // Deny-by-default around it: no plaintext :80, no separately-rotating security mirror, no wildcard.
        assert!(!d.allows("deb.debian.org", Proto::Tcp, 80));
        assert!(!d.allows("security.debian.org", Proto::Tcp, 443));
        assert!(!d.allows("snapshot.debian.org", Proto::Tcp, 443));
    }

    #[test]
    fn pypi_https_is_two_https_hosts_deny_by_default() {
        let p = resolve("pypi-https").unwrap();
        // Exactly TWO hosts by design: the index (pypi.org) and the file CDN (files.pythonhosted.org).
        assert_eq!(p.rules.len(), 2, "pypi-https must reach exactly the index + the file CDN");
        assert!(p.allows("pypi.org", Proto::Tcp, 443));
        assert!(p.allows("files.pythonhosted.org", Proto::Tcp, 443));
        // Deny-by-default around them: no plaintext :80, no legacy relic host, no test index, no wildcard.
        assert!(!p.allows("pypi.org", Proto::Tcp, 80));
        assert!(!p.allows("files.pythonhosted.org", Proto::Tcp, 80));
        assert!(!p.allows("pypi.python.org", Proto::Tcp, 443)); // 301 relic — deliberately unlisted
        assert!(!p.allows("test.pypi.org", Proto::Tcp, 443));
        assert!(!p.allows("files.pythonhosted.com", Proto::Tcp, 443)); // no suffix/typo matching
    }

    #[test]
    fn pypi_and_debian_are_distinct_profiles_no_cross_reach() {
        // The two workshop profiles that COMPOSE on one bench must stay distinct: neither reaches the
        // other's hosts on its own (the union is built at run time from BOTH, never implied by EITHER).
        let pypi = resolve("pypi-https").unwrap();
        let deb = resolve("debian-apt").unwrap();
        assert_ne!(pypi.name, deb.name);
        assert!(!pypi.allows("deb.debian.org", Proto::Tcp, 443));
        assert!(!deb.allows("pypi.org", Proto::Tcp, 443));
        assert!(!deb.allows("files.pythonhosted.org", Proto::Tcp, 443));
        // And pip's index is not a model/broker/swamp destination.
        assert!(!pypi.grants_swamp_query());
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
    fn model_claude_cli_reaches_only_the_broker() {
        // The subscription-model profile reaches exactly ONE dst — the CLI broker — NEVER Anthropic
        // directly and NEVER any other port/host. No secret lives here: the broker shells the
        // logged-in `claude` CLI, which owns its own auth (Phase-6 slice-4).
        let m = resolve("model-claude-cli").unwrap();
        assert_eq!(m.rules.len(), 1, "model-claude-cli must reach exactly one destination (the broker)");
        assert!(m.allows("shrek-claude-cli", Proto::Tcp, 8300));
        // The box must NOT reach Anthropic directly, the api-key proxy, or the local model.
        assert!(!m.allows("api.anthropic.com", Proto::Tcp, 443));
        assert!(!m.allows("shrek-model-proxy", Proto::Tcp, 8200)); // not the api-key proxy dst
        assert!(!m.allows("shrek-model", Proto::Tcp, 8100));       // not the local-model dst
    }

    #[test]
    fn subscription_and_apikey_paths_are_distinct_profiles() {
        // The two hosted paths must be SEPARATE sealed names pointing at DIFFERENT brokers — never one
        // name silently reused for two backends. This is the no-silent-backend-swap invariant made
        // testable: neither profile's single destination overlaps the other's.
        let cli = resolve("model-claude-cli").unwrap();
        let api = resolve("model-anthropic").unwrap();
        assert_ne!(cli.name, api.name);
        assert!(!cli.allows("shrek-model-proxy", Proto::Tcp, 8200));
        assert!(!api.allows("shrek-claude-cli", Proto::Tcp, 8300));
    }

    #[test]
    fn model_codex_cli_reaches_only_the_broker() {
        // The SECOND subscription-model profile reaches exactly ONE dst — the Codex CLI broker — and
        // nothing else. No secret lives here: the broker shells the logged-in `codex exec` CLI, which
        // owns its own auth (Phase-6 slice-6). Distinct port (8301) from the claude broker (8300).
        let m = resolve("model-codex-cli").unwrap();
        assert_eq!(m.rules.len(), 1, "model-codex-cli must reach exactly one destination (the broker)");
        assert!(m.allows("shrek-codex-cli", Proto::Tcp, 8301));
        // The box must NOT reach OpenAI directly, the claude broker, the api-key proxy, or the local model.
        assert!(!m.allows("api.openai.com", Proto::Tcp, 443));
        assert!(!m.allows("chatgpt.com", Proto::Tcp, 443));
        assert!(!m.allows("shrek-claude-cli", Proto::Tcp, 8300)); // not the claude broker
        assert!(!m.allows("shrek-model-proxy", Proto::Tcp, 8200)); // not the api-key proxy
        assert!(!m.allows("shrek-model", Proto::Tcp, 8100));       // not the local-model dst
    }

    #[test]
    fn all_three_provider_paths_are_mutually_distinct_profiles() {
        // The no-silent-backend-swap invariant now GENERALIZES: api-key, Claude-subscription, and
        // Codex-subscription are THREE separate sealed names at THREE different brokers/ports. No
        // profile's single destination overlaps any other's — selecting a provider is always explicit.
        let api = resolve("model-anthropic").unwrap();
        let claude = resolve("model-claude-cli").unwrap();
        let codex = resolve("model-codex-cli").unwrap();
        // Names are pairwise distinct.
        assert_ne!(api.name, claude.name);
        assert_ne!(api.name, codex.name);
        assert_ne!(claude.name, codex.name);
        // Destinations are pairwise non-overlapping (each reaches exactly its own broker).
        assert!(!codex.allows("shrek-model-proxy", Proto::Tcp, 8200));
        assert!(!codex.allows("shrek-claude-cli", Proto::Tcp, 8300));
        assert!(!claude.allows("shrek-codex-cli", Proto::Tcp, 8301));
        assert!(!api.allows("shrek-codex-cli", Proto::Tcp, 8301));
    }

    #[test]
    fn swamp_query_reaches_only_the_swamp_broker() {
        // The in-sandbox swamp-query profile reaches exactly ONE dst — the swamp broker — on its own
        // distinct port (8400), overlapping NONE of the model brokers. Selecting swamp query is an
        // explicit sealed choice, orthogonal to any model egress a session may also hold.
        let s = resolve("swamp-query").unwrap();
        assert_eq!(s.rules.len(), 1, "swamp-query must reach exactly one destination (the swamp broker)");
        assert!(s.allows("shrek-swamp-broker", Proto::Tcp, 8400));
        // Not any model broker/proxy, not the local model, not a wildcard port on its own host.
        assert!(!s.allows("shrek-model-proxy", Proto::Tcp, 8200));
        assert!(!s.allows("shrek-claude-cli", Proto::Tcp, 8300));
        assert!(!s.allows("shrek-codex-cli", Proto::Tcp, 8301));
        assert!(!s.allows("shrek-model", Proto::Tcp, 8100));
        assert!(!s.allows("shrek-swamp-broker", Proto::Tcp, 443));
        // And no model profile can reach the swamp broker.
        assert!(!resolve("model-anthropic").unwrap().allows("shrek-swamp-broker", Proto::Tcp, 8400));
        assert!(!resolve("model-codex-cli").unwrap().allows("shrek-swamp-broker", Proto::Tcp, 8400));
    }

    #[test]
    fn swamp_capability_predicate_is_explicit_to_the_swamp_query_destination() {
        // The swamp-capability gate keys on the EXACT swamp-query dst, nothing weaker. Only a profile
        // that grants `SWAMP_QUERY_HOST:SWAMP_QUERY_PORT` is swamp-capable; every model-only profile and
        // the empty profile are not. This is the single predicate gatekeeperd gates the session-identity
        // transaction on, so it must never be true for a session lacking the swamp-query grant.
        assert!(resolve("swamp-query").unwrap().grants_swamp_query());
        for name in ["none", "model-local", "model-anthropic", "model-claude-cli", "model-codex-cli", "github-https"] {
            assert!(!resolve(name).unwrap().grants_swamp_query(), "{name} must NOT be swamp-capable");
        }
        // The predicate is bound to the sealed constants, not a magic string.
        assert_eq!(SWAMP_QUERY_HOST, "shrek-swamp-broker");
        assert_eq!(SWAMP_QUERY_PORT, 8400);
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
