//! provenance — `Evidence → TrustBand`, the B1 trust-band derivation. Phase-5 slice-7.
//!
//! This closes the last caller-asserted input to the tier decision. Before this slice the band rode
//! in on the construction request as a string and `gatekeeperd` took it on faith (only the fail-high
//! *parse* protected it); everything else in the pipeline was already re-derived from sealed sources.
//! Here the band itself becomes a value `gatekeeperd` DERIVES by measuring the code object it is
//! about to run, against its own sealed/compiled-in roots — never a value the caller supplies.
//!
//! Design of record: [`docs/phase5-slice7-trust-provenance.md`](../../../docs/phase5-slice7-trust-provenance.md);
//! security requirement pinned by security-model.md §6/B1; the four bands + floor are isolation.md §5.
//!
//! Like the rest of `shrek-policy` this is PURE, TOTAL, DEPENDENCY-FREE: no I/O, no allocation, no
//! state. `gatekeeperd` performs the actual measurement (a TOCTOU-safe `openat2`+`statx` of the
//! entrypoint, matching `st_dev` against the sealed dm-verity root — the same pin-and-identify
//! machinery as `mount_plane`) and hands the result in as [`Evidence`]. Keeping the *logic* here means
//! it is compiled into both daemons and sealed by dm-verity, and every branch is unit-testable
//! without a kernel.
//!
//! The derivation is a MONOTONIC FAIL-HIGH LATTICE (docs §3): `T-hostile` is the unambiguous floor,
//! and every band ABOVE it requires its OWN affirmative, integrity-checked proof. Anything unknown,
//! unverifiable, absent, or a proposal/measurement MISMATCH resolves to `T-hostile`, always. There is
//! exactly one answer for doubt, and it is the strongest wall the floor allows.

use crate::TrustBand;

/// An affirmative origin classification drawn from an integrity-checked provenance record (the sealed
/// §4 log). The two deferred stores it names (untrusted-ingest and generated records) do not exist in
/// the MVP, so `gatekeeperd` only ever produces [`Origin::None`] today — which is exactly the
/// fail-high floor. The variants are declared so the lattice is complete and testable now, and so the
/// deferred stores drop in without a signature change.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Origin {
    /// An affirmative, integrity-checked record classifies the code's origin as untrusted-ingest
    /// (cloned repo / downloaded artifact). This is the ONLY thing that earns `T-untrust`: the weaker
    /// band still has to be *proven*, never inferred from mere absence of a stronger proof. (Deferred
    /// store — needs the §4 mutable provenance log; never produced in the MVP.)
    UntrustedIngest,
    /// Affirmatively recorded as agent-authored / generated code. Explicitly `T-hostile`. (Deferred
    /// store; never produced in the MVP.)
    Generated,
    /// No qualifying record — absent, unverifiable, or contradicted by the measurement. The dominant
    /// case, and the fail-high floor: resolves `T-hostile`.
    None,
}

/// What `gatekeeperd` measured about the code object it is about to execute. Every field is an
/// integrity-checked FACT the broker established itself (or `None`/`false` when it could not) — never
/// a caller assertion. The band is a pure function of these facts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Evidence {
    /// The workload entrypoint object resolved — TOCTOU-safely, `openat2(RESOLVE_NO_SYMLINKS)` then
    /// `statx` — to the sealed dm-verity ROOT device (docs §5). Because dm-verity re-verifies every
    /// block against a roothash sealed into the signed UKI at boot (security-model.md §2), residency
    /// on that device IS the measurement. A writable mount (`/var`, `/run`, tmpfs, a grant bind) has a
    /// different `st_dev`, so a mutable *entrypoint* cannot satisfy this. But this proves ENTRYPOINT
    /// PROVENANCE ONLY — a sealed entrypoint that then interprets/JITs/loads mutable content is not
    /// caught by mount identity; forbidding that laundering is [`Evidence::domain_execution_sealed`]'s
    /// job (docs §5.1), a separate fact.
    pub entrypoint_sealed: bool,

    /// The resolved sealed EXECUTION PROFILE is closed-world: a fixed sealed program that does NOT
    /// read-and-execute external / mutable / interpreted / generated code (docs §5.1). Derived by
    /// `gatekeeperd` from compiled-in sealed policy — NOT from mount flags (`noexec` is defense-in-
    /// depth, never this proof) and NEVER from the caller. REQUIRED for `T-first`/`T-pinned`: a sealed
    /// entrypoint that is itself an interpreter / JIT / plugin host can execute mutable content
    /// without that content being executable to the kernel, so [`Evidence::entrypoint_sealed`] proves
    /// provenance only; this is the separate fact that the band does not launder onto code the
    /// entrypoint runs. An arbitrary-code-capable (open-world) profile leaves this `false` and fails
    /// high to `T-hostile`.
    pub domain_execution_sealed: bool,

    /// The entrypoint's content digest matched an entry in a sealed pin-manifest (the §4 pin store).
    /// Deferred store — always `false` in the MVP.
    pub pinned_digest_match: bool,

    /// An affirmative origin classification from the sealed provenance log. `Origin::None` in the MVP.
    pub origin: Origin,
}

impl Evidence {
    /// The MVP evidence: only the two sealed-domain facts are ever established; the deferred pin and
    /// origin stores are off (`false`/`None`) — so anything not provably first-party is `T-hostile`,
    /// the correct fail-safe posture until those stores ship (docs §8).
    pub fn mvp(entrypoint_sealed: bool, domain_execution_sealed: bool) -> Evidence {
        Evidence {
            entrypoint_sealed,
            domain_execution_sealed,
            pinned_digest_match: false,
            origin: Origin::None,
        }
    }

    /// The fail-high floor value: nothing proven. Used when measurement itself failed (an
    /// `openat2`/`statx` error, a resolve rejection, or a proposal/measurement mismatch) — the broker
    /// must not distinguish "could not measure" from "measured hostile": both are `T-hostile`.
    pub const UNVERIFIABLE: Evidence = Evidence {
        entrypoint_sealed: false,
        domain_execution_sealed: false,
        pinned_digest_match: false,
        origin: Origin::None,
    };
}

/// Derive the trust band from measured [`Evidence`] — isolation.md §5 bands, docs §3 lattice.
///
/// Strongest-first, and every non-hostile band is gated on its own affirmative proof. There is
/// deliberately no path that yields a band from the *absence* of a stronger one: absence is the floor.
pub fn derive_band(e: &Evidence) -> TrustBand {
    // T-first: measured under the sealed root AND the whole execution domain stays sealed (§5.1). The
    // domain gate is what forbids transitive/laundered T-first.
    if e.entrypoint_sealed && e.domain_execution_sealed {
        return TrustBand::First;
    }
    // T-pinned: a sealed-pin digest match, under the same domain gate (deferred store ⇒ never in MVP).
    if e.pinned_digest_match && e.domain_execution_sealed {
        return TrustBand::Pinned;
    }
    // Below the positive-proof bands: only an AFFIRMATIVE untrusted-ingest record earns T-untrust;
    // a generated record, no record, an unverifiable one, or a mismatch is the T-hostile floor.
    match e.origin {
        Origin::UntrustedIngest => TrustBand::Untrust,
        Origin::Generated | Origin::None => TrustBand::Hostile,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_entrypoint_in_sealed_domain_is_first() {
        assert_eq!(derive_band(&Evidence::mvp(true, true)), TrustBand::First);
    }

    #[test]
    fn sealed_entrypoint_but_mutable_domain_cannot_launder_to_first() {
        // The no-laundering rule (§5.1): a first-party entrypoint that can reach mutable code does
        // NOT get T-first. With no other affirmative evidence it falls to the hostile floor.
        assert_eq!(derive_band(&Evidence::mvp(true, false)), TrustBand::Hostile);
    }

    #[test]
    fn unsealed_entrypoint_is_hostile_in_mvp() {
        // Code on a writable mount (different st_dev) is never sealed ⇒ hostile until an evidence
        // store (pin / untrusted-ingest) exists to earn it a weaker band.
        assert_eq!(derive_band(&Evidence::mvp(false, true)), TrustBand::Hostile);
        assert_eq!(derive_band(&Evidence::mvp(false, false)), TrustBand::Hostile);
    }

    #[test]
    fn absence_of_evidence_is_hostile_not_untrust() {
        // The core resolution invariant: T-untrust is NOT the default for "no strong proof".
        assert_eq!(derive_band(&Evidence::UNVERIFIABLE), TrustBand::Hostile);
    }

    #[test]
    fn only_affirmative_record_earns_untrust() {
        let mut e = Evidence::UNVERIFIABLE;
        e.origin = Origin::UntrustedIngest;
        assert_eq!(derive_band(&e), TrustBand::Untrust);
    }

    #[test]
    fn generated_record_is_hostile() {
        let mut e = Evidence::UNVERIFIABLE;
        e.origin = Origin::Generated;
        assert_eq!(derive_band(&e), TrustBand::Hostile);
    }

    #[test]
    fn pinned_match_needs_the_domain_gate_too() {
        // A pin match with a leaky (mutable-exec) domain cannot be T-pinned either — same no-laundering
        // gate as T-first. (pinned_digest_match is a deferred store, exercised here for completeness.)
        let sealed_domain = Evidence { pinned_digest_match: true, domain_execution_sealed: true, entrypoint_sealed: false, origin: Origin::None };
        let leaky_domain = Evidence { domain_execution_sealed: false, ..sealed_domain };
        assert_eq!(derive_band(&sealed_domain), TrustBand::Pinned);
        assert_eq!(derive_band(&leaky_domain), TrustBand::Hostile);
    }

    #[test]
    fn first_dominates_when_all_evidence_present() {
        // Strongest-first ordering: a fully-evidenced object is T-first, not a weaker band.
        let e = Evidence { entrypoint_sealed: true, domain_execution_sealed: true, pinned_digest_match: true, origin: Origin::UntrustedIngest };
        assert_eq!(derive_band(&e), TrustBand::First);
    }
}
