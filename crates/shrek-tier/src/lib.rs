//! shrek-tier — the deterministic `(trust × caps) → tier` decision plane. Phase-5 slice-2.
//!
//! This is the whole of isolation.md §4–§5.2 as code: the two discretized axes, the selection
//! matrix, the `floor(trust)` rule, and `effective_tier = max(matrix, floor, escalation)`. It is
//! PURE, TOTAL, and DEPENDENCY-FREE (no I/O, no allocation, no `std` state).
//!
//! Both `agentd` (the unprivileged resolver) and `gatekeeperd` (the privileged re-checker) compile
//! this in. The matrix and floor are therefore baked into every binary — sealed by dm-verity when
//! shipped in `/usr` — so gatekeeperd's independent recheck reads sealed policy, never writable
//! state (isolation.md §7; security-model.md §4/§6). There is deliberately NO `min` anywhere in the
//! resolution: caps can never lower the wall, only the floor and matrix (and upward escalation) set
//! it.
//!
//! Fail-high on doubt (security-model.md §6/B1): an unrecognized trust label parses to `Hostile`
//! and an unrecognized caps label to `Broad`, so a spoofed/garbled label can only ever RAISE the
//! wall, never lower it. An unrecognized *tier* does not fail-high — it is `None`, and the caller
//! (gatekeeperd) fails the request closed.
//!
//! What is NOT here (later slices, by design): how the trust band is derived/attested (OPEN B1,
//! owed to agents.md); the T0/T2/T3 constructors; the egress plane; the crypto seal + socket
//! transport. This crate only DECIDES; it never constructs.

#![forbid(unsafe_code)]

/// Provenance of the *code being executed* (never the data — that is caps). Four bands of
/// increasing risk. Declaration order is the risk order (`First` < … < `Hostile`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub enum TrustBand {
    /// First-party or Shrek-signed code; provenance verified, authored/audited by us.
    First,
    /// Third-party but pinned & vetted at a known hash (distro package, pinned crate/toolchain).
    Pinned,
    /// Untrusted: cloned repos, downloaded scripts/binaries, anything off the internet unvetted.
    Untrust,
    /// Adversarial-by-assumption: AI-generated / autonomous-agent-authored code, unknown plugins.
    Hostile,
}

/// What is inside the blast radius — the mount-set and egress, discretized by the *worst* thing
/// reachable. Declaration order is the danger order: each variant is a strict superset of privilege
/// over the previous (isolation.md §4: `C-net` = "any of the above PLUS network"). So `a <= b`
/// means "a is contained within the authority of b", which is exactly the `caps ⊆ profile` test.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub enum CapsProfile {
    /// Read-only, NO secret domains, NO network.
    RoNosec,
    /// Read-write to a single project scope; NO secrets; NO network.
    ProjRw,
    /// Any of the above PLUS network egress (even if allow-listed).
    Net,
    /// Broad `$HOME` visibility, or any secret-domain path, or unrestricted egress. The danger
    /// column that must never be granted to low-trust code.
    Broad,
}

/// The isolation tier — strength of the containment wall. Declaration order IS the strength order
/// `T0 < T1 < T2 < T3`, so `max` over tiers is "the strongest wall required". This ordering is the
/// load-bearing invariant of the whole slice.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub enum Tier {
    /// Process sandbox (Landlock/seccomp) — shared kernel.
    T0,
    /// System container (`systemd-nspawn`/LXC) — shared kernel, full userland.
    T1,
    /// Userspace kernel (gVisor) — no host syscalls. The default for untrusted-but-fast.
    T2,
    /// MicroVM (libkrun/Firecracker) — KVM hardware wall.
    T3,
}

impl CapsProfile {
    /// True if `self`'s authority is fully contained within `profile` (`caps ⊆ profile`). Uses the
    /// danger ordering above — this is the resolver's step-1 grant check.
    pub fn subset_of(self, profile: CapsProfile) -> bool {
        self <= profile
    }
}

/// The selection matrix — isolation.md §5, verbatim. Total over all 16 `(trust, caps)` pairs.
///
/// ```text
///               C-ro-nosec   C-proj-rw   C-net   C-broad
///   T-first        T0           T0         T1       T1
///   T-pinned       T0           T1         T1       T2
///   T-untrust      T2           T2         T2       T3
///   T-hostile      T2           T3         T3       T3
/// ```
pub fn matrix(trust: TrustBand, caps: CapsProfile) -> Tier {
    use CapsProfile::*;
    use Tier::*;
    use TrustBand::*;
    match (trust, caps) {
        (First, RoNosec) => T0,
        (First, ProjRw) => T0,
        (First, Net) => T1,
        (First, Broad) => T1,

        (Pinned, RoNosec) => T0,
        (Pinned, ProjRw) => T1,
        (Pinned, Net) => T1,
        (Pinned, Broad) => T2,

        (Untrust, RoNosec) => T2,
        (Untrust, ProjRw) => T2,
        (Untrust, Net) => T2,
        (Untrust, Broad) => T3,

        (Hostile, RoNosec) => T2,
        (Hostile, ProjRw) => T3,
        (Hostile, Net) => T3,
        (Hostile, Broad) => T3,
    }
}

/// The floor rule — isolation.md §5.1. The weakest tier a trust band may EVER run at, regardless of
/// how narrow its caps look. Provenance sets the floor; caps cannot buy it back.
pub fn floor(trust: TrustBand) -> Tier {
    use Tier::*;
    use TrustBand::*;
    match trust {
        First => T0,
        Pinned => T0,
        Untrust => T2,
        Hostile => T2,
    }
}

/// `effective_tier = max(matrix[trust][caps], floor(trust), escalation)` — isolation.md §5.2.
///
/// `escalation` is an OPTIONAL operator/policy request for a STRONGER wall; because it enters only
/// through `max`, it can never lower the result. There is no path here that lowers a tier: this is
/// the "downward-forbidden" property expressed in arithmetic.
pub fn effective_tier(trust: TrustBand, caps: CapsProfile, escalation: Option<Tier>) -> Tier {
    let mut t = matrix(trust, caps).max(floor(trust));
    if let Some(e) = escalation {
        t = t.max(e);
    }
    t
}

impl TrustBand {
    /// Parse a trust label. UNKNOWN/UNVERIFIABLE ⇒ `Hostile` (fail-high, security-model.md §6/B1):
    /// provenance spoofing can then only ever raise the wall, never lower it.
    pub fn parse(s: &str) -> TrustBand {
        match s {
            "T-first" | "first" => TrustBand::First,
            "T-pinned" | "pinned" => TrustBand::Pinned,
            "T-untrust" | "untrust" => TrustBand::Untrust,
            _ => TrustBand::Hostile,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TrustBand::First => "T-first",
            TrustBand::Pinned => "T-pinned",
            TrustBand::Untrust => "T-untrust",
            TrustBand::Hostile => "T-hostile",
        }
    }
}

impl CapsProfile {
    /// Parse a caps label. UNKNOWN ⇒ `Broad` (fail-high): the danger column, which forces the
    /// strongest wall and fails the `⊆ profile` check against anything narrower.
    pub fn parse(s: &str) -> CapsProfile {
        match s {
            "C-ro-nosec" | "ro-nosec" => CapsProfile::RoNosec,
            "C-proj-rw" | "proj-rw" => CapsProfile::ProjRw,
            "C-net" | "net" => CapsProfile::Net,
            _ => CapsProfile::Broad,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CapsProfile::RoNosec => "C-ro-nosec",
            CapsProfile::ProjRw => "C-proj-rw",
            CapsProfile::Net => "C-net",
            CapsProfile::Broad => "C-broad",
        }
    }

    /// True if realizing this profile requires network egress — which the egress plane (a later
    /// slice) does not yet exist to enforce. gatekeeperd uses this to fail closed rather than hand a
    /// workload a cap it cannot enforce.
    pub fn needs_egress(self) -> bool {
        matches!(self, CapsProfile::Net | CapsProfile::Broad)
    }
}

impl Tier {
    /// Parse a tier label STRICTLY. Unknown ⇒ `None` — the caller MUST fail the request closed. A
    /// tier is not fail-high: a garbled tier is a malformed request, not a weak wall.
    pub fn parse(s: &str) -> Option<Tier> {
        match s {
            "T0" => Some(Tier::T0),
            "T1" => Some(Tier::T1),
            "T2" => Some(Tier::T2),
            "T3" => Some(Tier::T3),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tier::T0 => "T0",
            Tier::T1 => "T1",
            Tier::T2 => "T2",
            Tier::T3 => "T3",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use CapsProfile::*;
    use Tier::*;
    use TrustBand::*;

    const TRUSTS: [TrustBand; 4] = [First, Pinned, Untrust, Hostile];
    const CAPS: [CapsProfile; 4] = [RoNosec, ProjRw, Net, Broad];

    #[test]
    fn matrix_matches_isolation_md_5() {
        // Rows top→bottom, cols left→right, exactly as the doc's table.
        let expect = [
            [T0, T0, T1, T1], // T-first
            [T0, T1, T1, T2], // T-pinned
            [T2, T2, T2, T3], // T-untrust
            [T2, T3, T3, T3], // T-hostile
        ];
        for (r, &t) in TRUSTS.iter().enumerate() {
            for (c, &cap) in CAPS.iter().enumerate() {
                assert_eq!(matrix(t, cap), expect[r][c], "cell [{:?}][{:?}]", t, cap);
            }
        }
    }

    #[test]
    fn floor_matches_isolation_md_5_1() {
        assert_eq!(floor(First), T0);
        assert_eq!(floor(Pinned), T0);
        assert_eq!(floor(Untrust), T2);
        assert_eq!(floor(Hostile), T2);
    }

    #[test]
    fn tier_ordering_is_t0_lt_t1_lt_t2_lt_t3() {
        assert!(T0 < T1 && T1 < T2 && T2 < T3);
    }

    #[test]
    fn caps_danger_ordering_is_the_subset_relation() {
        assert!(RoNosec < ProjRw && ProjRw < Net && Net < Broad);
        assert!(RoNosec.subset_of(Broad));
        assert!(ProjRw.subset_of(Net));
        assert!(!Broad.subset_of(Net)); // broad is NOT contained by net
        assert!(!Net.subset_of(ProjRw));
    }

    #[test]
    fn effective_is_never_below_floor_or_matrix() {
        for &t in &TRUSTS {
            for &cap in &CAPS {
                let e = effective_tier(t, cap, None);
                assert!(e >= floor(t), "{:?}/{:?} below floor", t, cap);
                assert!(e >= matrix(t, cap), "{:?}/{:?} below matrix", t, cap);
            }
        }
    }

    #[test]
    fn escalation_only_ever_raises() {
        // Escalating DOWN is a no-op; escalating UP raises.
        let base = effective_tier(Pinned, ProjRw, None); // T1
        assert_eq!(base, T1);
        assert_eq!(effective_tier(Pinned, ProjRw, Some(T0)), T1); // down-request ignored
        assert_eq!(effective_tier(Pinned, ProjRw, Some(T3)), T3); // up-request honored
        // Escalation can never take the result below the floor for any input.
        for &t in &TRUSTS {
            for &cap in &CAPS {
                for &esc in &[T0, T1, T2, T3] {
                    let e = effective_tier(t, cap, Some(esc));
                    assert!(e >= floor(t) && e >= matrix(t, cap));
                }
            }
        }
    }

    #[test]
    fn hostile_with_write_or_net_is_always_t3_except_readonly() {
        // isolation.md §5: the one T-hostile concession is C-ro-nosec → T2; everything else → T3.
        assert_eq!(effective_tier(Hostile, RoNosec, None), T2);
        assert_eq!(effective_tier(Hostile, ProjRw, None), T3);
        assert_eq!(effective_tier(Hostile, Net, None), T3);
        assert_eq!(effective_tier(Hostile, Broad, None), T3);
    }

    #[test]
    fn worked_examples_from_isolation_md_8() {
        assert_eq!(effective_tier(First, RoNosec, None), T0); // "summarize my notes"
        assert_eq!(effective_tier(Pinned, ProjRw, None), T1); // "compile Shrek's kernel"
        assert_eq!(effective_tier(Untrust, Net, None), T2); // "clone repo, run tests"
        assert_eq!(effective_tier(Hostile, Net, None), T3); // "autonomous agent + egress"
    }

    #[test]
    fn parse_fails_high_on_garbage() {
        assert_eq!(TrustBand::parse("T-first"), First);
        assert_eq!(TrustBand::parse("nonsense"), Hostile); // fail-high
        assert_eq!(TrustBand::parse(""), Hostile);
        assert_eq!(CapsProfile::parse("C-net"), Net);
        assert_eq!(CapsProfile::parse("nonsense"), Broad); // fail-high
        assert_eq!(Tier::parse("T2"), Some(T2));
        assert_eq!(Tier::parse("T9"), None); // strict — caller fails closed
    }

    #[test]
    fn labels_roundtrip() {
        for &t in &TRUSTS {
            assert_eq!(TrustBand::parse(t.label()), t);
        }
        for &c in &CAPS {
            assert_eq!(CapsProfile::parse(c.label()), c);
        }
        for &t in &[T0, T1, T2, T3] {
            assert_eq!(Tier::parse(t.label()), Some(t));
        }
    }
}
