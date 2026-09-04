//! provider_bind — the sealed CLOSED SET of model-provider hook-up tokens (ADR-008, S1).
//!
//! ADR-008 makes root the sole author of `/etc/hosts` and gives uid 1000 exactly ONE way to influence
//! name resolution: bind the ADDRESS of a model-provider broker via the egressd `bind <token> <addr>`
//! verb. This module is the sealed policy DATA behind that verb — the thing that makes "uid 1000 can
//! only ever name the 4 model brokers" true BY CONSTRUCTION rather than by a sanitizer's blocklist.
//!
//! Two pieces, both PURE/TOTAL/DEP-FREE like the rest of this crate (no I/O, no state, no allocation):
//!
//!   * [`PROVIDER_BINDINGS`] — the closed `token → sealed-name` map. `shrek-connect <provider> <addr>`
//!     sends the TOKEN (`local`/`anthropic`/`claude`/`codex`); egressd maps it to the sealed egress
//!     HOST NAME here, SERVER-SIDE, and composes `<addr> <host>` into the root-owned projection. The
//!     client never sends a free hostname, so it can never introduce a name into `/etc/hosts` that is
//!     not one of these four — never `github.com`, an NTP host, or a CA/OCSP host (the #3121 authority).
//!     The sealed host strings are the SAME ones the agent egress table reaches ([`crate::egress`]);
//!     the `bindings_match_the_sealed_egress_table` test is the anti-drift guarantee tying the two
//!     together, so a rename in one place fails the build until reconciled.
//!
//!   * [`valid_bind_addr`] — the sealed address grammar. STRICT IPv4 dotted-quad ONLY. glibc's NSS
//!     `files` parser requires an IP LITERAL in the address column (a hostname line is SILENTLY SKIPPED
//!     and never resolves — ADR-008 §1c [R1-MF2], the latent bug in the old shell `shrek-connect`), and
//!     gatekeeperd fail-closes on a no-A-record host (IPv6 would only self-DoS), so the one legal shape
//!     is a v4 literal. Returns the parsed [`Ipv4Addr`]; callers write `addr.to_string()` — the
//!     CANONICAL dotted-decimal render — into the store and projection, which kills any parse-
//!     differential with `inet_aton`'s hex/octal/short forms (whatever `from_str` accepts is re-emitted
//!     canonically, and the hex/octal/short forms `from_str` rejects never reach the store).
//!
//! Swamp note: `shrek-swamp-broker` ([`crate::egress::SWAMP_QUERY_HOST`]) is a 5th sealed name but is
//! DELIBERATELY ABSENT here — it is the un-masqueraded, identity-preserving destination (a strictly
//! worse thing to let uid 1000 steer), its address is a fixed local bridge (not an owner choice), and
//! swamp-query is bench-only on shipped images (ADR-008 §3 / R-2). It is never owner-bindable.

use std::net::Ipv4Addr;
use std::str::FromStr;

/// One row of the closed provider map: the uid-1000-facing TOKEN, the egress PROFILE it belongs to,
/// and the sealed egress HOST NAME that profile reaches. `host` is what lands in `/etc/hosts`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderBinding {
    /// The token uid 1000 sends over the egressd `bind` verb (`shrek-connect <provider>`).
    pub token: &'static str,
    /// The sealed [`crate::egress`] profile whose destination this binds (single source of truth).
    pub profile: &'static str,
    /// The sealed egress host name — the LHS-of-a-hosts-line target the bound IPv4 resolves.
    pub host: &'static str,
}

/// THE closed set. Exactly the four model brokers, nothing else. A token absent here resolves to
/// `None` ([`provider_host`]) and the `bind` verb is refused — fail-closed, mirroring
/// [`crate::egress::resolve`]. Order is display order (matches the old shell `PROVIDERS` list).
pub const PROVIDER_BINDINGS: &[ProviderBinding] = &[
    ProviderBinding { token: "local",     profile: "model-local",      host: "shrek-model" },
    ProviderBinding { token: "anthropic", profile: "model-anthropic",  host: "shrek-model-proxy" },
    ProviderBinding { token: "claude",    profile: "model-claude-cli", host: "shrek-claude-cli" },
    ProviderBinding { token: "codex",     profile: "model-codex-cli",  host: "shrek-codex-cli" },
];

/// Map a provider TOKEN to its sealed egress host name, or `None` for anything not in the closed set.
/// This is the server-side gate egressd applies before writing a binding: only a token that maps to a
/// sealed host may ever reach the projection, so uid 1000 cannot name an arbitrary host.
pub fn provider_host(token: &str) -> Option<&'static str> {
    PROVIDER_BINDINGS.iter().find(|b| b.token == token).map(|b| b.host)
}

/// Is `token` a member of the closed provider set? (`provider_host(token).is_some()`, named for call
/// sites that only need the membership predicate.)
pub fn is_provider_token(token: &str) -> bool {
    provider_host(token).is_some()
}

/// Is `host` a SEALED ALIAS — a name a daemon resolves through the ROOT-owned `/etc/hosts` (an owner
/// model binding) or a fixed local bridge, NEVER through public DNS/DoT? The 4 model-provider sealed
/// names plus the swamp-query broker ([`crate::egress::SWAMP_QUERY_HOST`]). ADR-008 S4 uses this so
/// gatekeeperd resolves an UNBOUND alias fail-closed (no brain connected) instead of leaking its label
/// to a public resolver: the public-name egress path is sealed DoT; the alias path is the hosts file.
pub fn is_sealed_alias_host(host: &str) -> bool {
    PROVIDER_BINDINGS.iter().any(|b| b.host == host) || host == crate::egress::SWAMP_QUERY_HOST
}

/// The sealed bind-address grammar: a STRICT IPv4 dotted-quad, or `None`. Rejects hostnames, IPv6, and
/// anything with whitespace, control, or non-`[0-9.]` bytes BEFORE parsing (so `inet_aton` hex like
/// `0x7f000001`, octal, and short forms like `127.1` are refused decisively). Returns the parsed
/// [`Ipv4Addr`]; the caller writes its canonical `to_string()` form. See the module docs for why a
/// literal is the only shape glibc `files` + gatekeeperd will honor [R1-MF2].
pub fn valid_bind_addr(s: &str) -> Option<Ipv4Addr> {
    // Pre-filter: only decimal digits and dots. Kills hex/whitespace/IPv6/hostname up front and makes
    // the intent explicit; `Ipv4Addr::from_str` then enforces the 4-octet 0..=255 dotted structure.
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    Ipv4Addr::from_str(s).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::egress::{resolve, Proto};

    #[test]
    fn closed_set_is_exactly_the_four_model_brokers() {
        assert_eq!(PROVIDER_BINDINGS.len(), 4);
        let tokens: Vec<&str> = PROVIDER_BINDINGS.iter().map(|b| b.token).collect();
        assert_eq!(tokens, vec!["local", "anthropic", "claude", "codex"]);
    }

    #[test]
    fn provider_host_maps_tokens_and_fails_closed() {
        assert_eq!(provider_host("local"), Some("shrek-model"));
        assert_eq!(provider_host("anthropic"), Some("shrek-model-proxy"));
        assert_eq!(provider_host("claude"), Some("shrek-claude-cli"));
        assert_eq!(provider_host("codex"), Some("shrek-codex-cli"));
        // Anything outside the closed set — including a real sealed name, a public host, or the swamp
        // broker — is NOT a bindable token.
        for junk in ["", "swamp", "shrek-model", "github.com", "shrek-swamp-broker", "LOCAL", "local "] {
            assert_eq!(provider_host(junk), None, "{junk:?} must not map");
            assert!(!is_provider_token(junk), "{junk:?} must not be a provider token");
        }
    }

    /// ANTI-DRIFT: every provider host string must be a REAL destination of its named sealed egress
    /// profile. If someone renames `shrek-model-proxy` in egress.rs without updating this table (or
    /// vice-versa), this fails — the two sources of the sealed name can never silently diverge.
    #[test]
    fn bindings_match_the_sealed_egress_table() {
        for b in PROVIDER_BINDINGS {
            let p = resolve(b.profile)
                .unwrap_or_else(|| panic!("profile {:?} is not a sealed egress profile", b.profile));
            // The model profiles each reach EXACTLY their one broker host; confirm the bound host is it.
            assert_eq!(p.rules.len(), 1, "model profile {:?} must reach exactly one host", b.profile);
            assert_eq!(p.rules[0].host, b.host, "profile {:?} host drifted from the bind table", b.profile);
            // And it is a tcp destination (every model broker is plaintext tcp; ADR-008 §1d).
            assert!(matches!(p.rules[0].proto, Proto::Tcp), "model broker {:?} must be tcp", b.host);
        }
    }

    /// The swamp broker is a sealed name but MUST NOT be reachable through the bind map (ADR-008 §3).
    #[test]
    fn swamp_broker_is_not_owner_bindable() {
        assert!(PROVIDER_BINDINGS.iter().all(|b| b.host != crate::egress::SWAMP_QUERY_HOST));
        assert_eq!(provider_host("swamp"), None);
    }

    #[test]
    fn sealed_aliases_are_the_model_hosts_plus_swamp() {
        // the 4 model broker names + the swamp broker resolve via /etc/hosts, never public DoT.
        for h in ["shrek-model", "shrek-model-proxy", "shrek-claude-cli", "shrek-codex-cli"] {
            assert!(is_sealed_alias_host(h), "{h} must be a sealed alias");
        }
        assert!(is_sealed_alias_host(crate::egress::SWAMP_QUERY_HOST));
        // public DNS names are NOT aliases — they take the sealed-DoT path.
        for h in ["github.com", "deb.debian.org", "pypi.org", "crates.io", "localhost"] {
            assert!(!is_sealed_alias_host(h), "{h} must NOT be a sealed alias");
        }
    }

    #[test]
    fn valid_bind_addr_accepts_and_canonicalizes_ipv4() {
        let a = valid_bind_addr("192.168.1.152").expect("plain dotted-quad");
        assert_eq!(a.to_string(), "192.168.1.152");
        // A 100.x tailnet address is fine (the common remote-broker case).
        assert_eq!(valid_bind_addr("100.66.228.66").unwrap().to_string(), "100.66.228.66");
        // Canonical re-render is the store/projection form — no matter what `from_str` tolerates, the
        // written form is unambiguous decimal, so glibc `files` reads exactly what we intended.
        let a = valid_bind_addr("127.0.0.1").unwrap();
        assert_eq!(a.to_string(), "127.0.0.1");
    }

    #[test]
    fn valid_bind_addr_rejects_everything_that_is_not_a_v4_literal() {
        for bad in [
            "",                     // empty
            "shrek-model",          // a name
            "myhost.lan",           // a hostname (the OLD shell bug — silently skipped by NSS files)
            "example.com",          // hostname
            "::1",                  // IPv6
            "fe80::1",              // IPv6
            "0x7f000001",           // inet_aton hex
            "127.1",                // inet_aton short form
            "127.0.0.1:8100",       // has a port
            "127.0.0.1 ",           // trailing space (would smuggle a 2nd token)
            " 127.0.0.1",           // leading space
            "127.0.0.1\n",          // newline (would smuggle a 2nd hosts line)
            "1.2.3.4.5",            // five octets
            "256.0.0.1",            // octet out of range
            "1.2.3",                // three octets
            "a.b.c.d",              // letters
        ] {
            assert!(valid_bind_addr(bad).is_none(), "{bad:?} must be rejected");
        }
    }
}
