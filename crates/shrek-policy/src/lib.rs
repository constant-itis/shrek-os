//! shrek-policy — the deterministic, compiled-in policy both `agentd` and `gatekeeperd` resolve
//! INDEPENDENTLY. Every piece here is PURE, TOTAL, and DEPENDENCY-FREE (no I/O, no allocation, no
//! state), and is baked into every binary — sealed by dm-verity when shipped in `/usr` — so
//! gatekeeperd's privileged re-check reads sealed policy, never writable state (isolation.md §7;
//! security-model.md §4/§6). This crate only DECIDES; it never constructs.
//!
//! Two policy dimensions, deliberately separate because they answer different questions:
//!
//!   * [`tier`]   — `(trust × caps) → minimum isolation tier` (the selection matrix + floor rule).
//!   * [`egress`] — `profile-name → the CLOSED set of {host:proto:port} it may reach`. The
//!                  construction request carries a profile IDENTITY, not destinations; gatekeeperd
//!                  resolves that identity from THIS sealed table, so agentd can name `github-dev`
//!                  but can never manufacture `evil.example:443` and trick the broker into it.
//!
//! The tier axis is re-exported at the crate root for ergonomics (`shrek_policy::Tier` etc.); egress
//! is reached through its module (`shrek_policy::egress::…`) so the two policy vocabularies stay
//! visibly distinct.
//!
//! What is NOT here (later slices, by design): how the trust band is derived/attested (OPEN B1);
//! the T0/T2/T3 constructors; egress ENFORCEMENT (netns/veth/nftables, DNS pre-resolution, the
//! ready-barrier — all gatekeeperd's, because it constructs); per-request DYNAMIC egress
//! destinations (the later sealed grant protocol); the crypto seal + socket transport.

#![forbid(unsafe_code)]

pub mod egress;
pub mod tier;

// The tier vocabulary is the crate's original public surface — re-export it flat so existing
// callers use `shrek_policy::{Tier, TrustBand, CapsProfile, effective_tier, …}` unchanged.
pub use tier::*;
