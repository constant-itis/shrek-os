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
//!   * [`provenance`] — `Evidence → TrustBand` (B1, slice-7): the band is DERIVED from an
//!                  integrity-checked measurement of the code object, not asserted by the caller. The
//!                  pure lattice lives here; gatekeeperd does the measuring.
//!   * [`swamp`]   — the sealed indexable-domain allow-set + human-only exclusion (Phase-6 Swamp
//!                  slice-1): the default-DENY read-scope `swampd` Landlocks itself to, and the
//!                  per-domain ceiling the query gate intersects. Pure policy DATA, resolved
//!                  independently by `swampd` (swamp.md §5/§9, security-model.md §5).
//!
//! What is NOT here (later slices, by design): the deferred B1 evidence STORES (the sealed pin-
//! manifest for T-pinned, the §4 provenance log for T-untrust — the MVP proves only T-first via the
//! dm-verity root and fails everything else high); the T0/T2/T3 constructors; egress ENFORCEMENT
//! (netns/veth/nftables, DNS pre-resolution, the
//! ready-barrier — all gatekeeperd's, because it constructs); per-request DYNAMIC egress
//! destinations (the later sealed grant protocol); the crypto seal + socket transport.

#![forbid(unsafe_code)]

pub mod egress;
pub mod provenance;
pub mod swamp;
pub mod tier;

// The tier vocabulary is the crate's original public surface — re-export it flat so existing
// callers use `shrek_policy::{Tier, TrustBand, CapsProfile, effective_tier, …}` unchanged.
pub use tier::*;

// The B1 derivation (slice-7) is flat-exported alongside it: `shrek_policy::{Evidence, Origin,
// derive_band}`. gatekeeperd measures the code object and calls `derive_band` instead of trusting a
// caller-supplied band string.
pub use provenance::{derive_band, Evidence, Origin};
