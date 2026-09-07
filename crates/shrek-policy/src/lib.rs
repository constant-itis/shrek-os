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
//!   * [`egress_capability`] — the DATA-DRIVEN desktop-egress capability layer (ADR-009 v2, S1): the
//!                  flat-manifest grammar + parser, the one-click `tcp:443` tier invariant (what makes
//!                  a plaintext host structurally unclickable), the merged sealed+owner catalog
//!                  (sealed-always-wins), and the §4.4 isolation predicates (`is_sealed_deliverable_host`,
//!                  `is_system_reserved_host`). Pure grammar+catalog logic; egressd (S2) loads the two
//!                  on-disk dirs and feeds parsed manifests in. Sibling of [`desktop_egress`], which
//!                  keeps the COMPILED baseline/broad profiles (ADR-009 §4.1).
//!   * [`provider_bind`] — the sealed CLOSED SET of model-provider hook-up tokens (ADR-008, S1): the
//!                  `token → sealed-name` map behind the egressd `bind` verb + the strict-IPv4
//!                  bind-address grammar. Makes "uid 1000 may name only the 4 model brokers" true by
//!                  construction; root is the sole author of `/etc/hosts` (security-model.md, #3121).
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

pub mod desktop_egress;
pub mod egress;
pub mod egress_capability;
pub mod provenance;
pub mod provider_bind;
pub mod swamp;
pub mod tier;

// The tier vocabulary is the crate's original public surface — re-export it flat so existing
// callers use `shrek_policy::{Tier, TrustBand, CapsProfile, effective_tier, …}` unchanged.
pub use tier::*;

// The B1 derivation (slice-7) is flat-exported alongside it: `shrek_policy::{Evidence, Origin,
// derive_band}`. gatekeeperd measures the code object and calls `derive_band` instead of trusting a
// caller-supplied band string.
pub use provenance::{derive_band, Evidence, Origin};
