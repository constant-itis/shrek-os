//! dot — egressd's desktop-profile wrappers over the shared sealed-DoT resolver ([`shrek_dot`]).
//!
//! The sealed DoT CLIENT (the rustls transport, the dep-free DNS codec, the sealed resolver set, and the
//! sealed private trust base) moved to the shared `shrek-dot` crate in ADR-008 S4, so gatekeeperd
//! resolves its PRIVILEGED public-egress pins (github/debian/pypi/crates) over the exact same channel —
//! off `resolved` entirely (the #3121 "don't use the owner-controlled resolver as a security oracle"
//! principle). This module keeps only the egressd-specific layer: resolving a desktop egress PROFILE's
//! sealed rule hosts into [`store::Pin`]s for the bless/pin store + applier.

use std::net::Ipv4Addr;
use std::time::Duration;

use crate::store::Pin;

// Re-export the shared client surface so the existing `crate::dot::…` call sites keep working unchanged:
// `confirmed.rs`'s `resolve_over_dot`, and the `DotError` the supervisor/CLI journal on a resolve fault.
pub use shrek_dot::{resolve_over_dot, resolve_over_dot_logged, DotError};

/// Resolve every sealed rule host of a PINNABLE profile into [`Pin`]s ready for the store/applier.
/// Refuses a non-pinnable profile (broad / pre-pinned / baseline-empty / unknown) — those are never
/// DoT-resolved. A single host that fails to resolve fails the whole profile (fail-closed: a partial
/// pin set is not applied).
pub fn resolve_profile_pins(profile: &str, id: u16, timeout: Duration) -> Result<Vec<Pin>, DotError> {
    resolve_profile_pins_logged(profile, id, timeout).map(|(p, _)| p)
}

/// As [`resolve_profile_pins`], but reports the resolver that answered (the FIRST host's, for the
/// journal — profiles have one host today). Fail-closed on any host that won't resolve.
pub fn resolve_profile_pins_logged(
    profile: &str,
    id: u16,
    timeout: Duration,
) -> Result<(Vec<Pin>, Option<Ipv4Addr>), DotError> {
    use shrek_policy::desktop_egress::{is_broad_profile, is_prepinned_profile, resolve_desktop};
    let prof = resolve_desktop(profile).ok_or_else(|| DotError::BadName(profile.to_string()))?;
    if is_broad_profile(profile) || is_prepinned_profile(profile) || prof.is_empty() {
        return Err(DotError::BadName(format!("{profile} is not DoT-resolvable")));
    }
    let mut pins = Vec::new();
    let mut used: Option<Ipv4Addr> = None;
    for rule in prof.rules {
        let (addrs, resolver) = resolve_over_dot_logged(rule.host, id, timeout)?;
        used.get_or_insert(resolver);
        for addr in addrs {
            pins.push(Pin { name: rule.host.to_string(), addr });
        }
    }
    Ok((pins, used))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_profile_refuses_non_pinnable() {
        // broad / pre-pinned / unknown are never DoT-resolved (no network hit — errors before any I/O).
        for p in ["web-browsing", "desktop-ntp", "desktop-updates", "evil"] {
            assert!(resolve_profile_pins(p, 0, Duration::from_millis(1)).is_err(), "{p} must be refused");
        }
    }
}
