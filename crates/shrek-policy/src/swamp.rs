//! swamp — the sealed indexable-domain table + the human-only exclusion. Phase-6 Swamp slice-1.
//!
//! This is `swampd`'s read-scope AS A SUBJECT of the wall (swamp.md §5, security-model.md §5): a
//! default-DENY allow-list of the trees `swampd` may index, authored ONLY here — never a writable
//! config, which is the deny-list misconfiguration class §5 exists to eliminate. Compiled in and
//! dm-verity-sealed exactly like [`crate::egress`]; `swampd` resolves it INDEPENDENTLY and Landlocks
//! ITSELF to it before any crawl, so a protected tree's bytes never enter its address space. Like the
//! rest of this crate it is PURE policy DATA + total lookups: no I/O, no allocation, no `$HOME`
//! expansion (that concrete-path step is `swampd`'s, at load — this table is host-relative TEMPLATE).
//!
//! Two things live here, both load-bearing:
//!
//!   * [`INDEXABLE_DOMAINS`] — the allow-set TEMPLATE. Each domain is a NAME + a set of `$HOME`-
//!     relative member subtrees + a per-domain [`DomainCeiling`]. The ceiling is a CEILING, never a
//!     grant: "a logical domain combines knowledge, never authority" (filesystem-intelligence.md §3).
//!     It is one term of the per-object authority intersection the query gate resolves (swamp.md §9),
//!     and it can only ever NARROW.
//!   * [`NEVER_INDEXABLE`] — human-only path-component markers that must NOT be indexed even when
//!     they nest UNDER an indexable member (e.g. `~/Projects/foo/.ssh`). Default-deny already excludes
//!     non-members; this is the authoritative statement that these names are unreachable by ANY path,
//!     and the crawler's belt-and-suspenders skip for the nested case (the OUTER boundary — a top-
//!     level `~/Vault` that is simply not a member — is enforced by Landlock, not by this list).
//!
//! What is deliberately NOT here (later slices): per-machine allow-set ADDITIONS (the §5 counter-
//! anchored grant path — this slice ships only the sealed template); sealing the *source* of this
//! list vs the compiled table; the FTS/semantic/relationship enrichment tiers (swamp.md §8).

/// The read-intelligence capability ceiling a domain imposes: the three query verbs of the model
/// (filesystem-intelligence.md §3/§5). Every term is a CEILING — the query gate intersects this with
/// the caller's session grants and each object's inherited policy, and intersection can only narrow.
/// There is no `write`/`exec` here: `swampd` never writes files and never runs code (swamp.md §9).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DomainCeiling {
    /// May the existence of matching objects be revealed at all? `false` ⇒ objects are ABSENT from
    /// the caller's projection (swamp.md §9's `discover:false`), never returned-and-filtered.
    pub discover: bool,
    /// May object metadata/content be returned to the caller?
    pub read: bool,
    /// May the object participate in full-text / semantic search?
    pub search: bool,
}

impl DomainCeiling {
    /// The read-only intelligence ceiling: discoverable, readable, searchable. The uniform ceiling
    /// for every indexable domain at slice-1 — the intelligence layer is read-only by construction.
    pub const RO_SEARCH: DomainCeiling = DomainCeiling { discover: true, read: true, search: true };
    /// The floor: reveals nothing. The result of intersecting with an unauthorized term — an object
    /// at this ceiling is invisible (absent from the projection), not merely unreadable.
    pub const NONE: DomainCeiling = DomainCeiling { discover: false, read: false, search: false };

    /// Intersection — the ONLY composition. Every verb is granted iff BOTH sides grant it, so the
    /// result can only be equal-or-narrower than either input. This is the load-bearing property:
    /// composing ceilings never widens authority (filesystem-intelligence.md §3).
    pub fn intersect(self, other: DomainCeiling) -> DomainCeiling {
        DomainCeiling {
            discover: self.discover && other.discover,
            read: self.read && other.read,
            search: self.search && other.search,
        }
    }

    /// Nothing is granted — the object is invisible to this caller.
    pub fn is_none(self) -> bool {
        !self.discover && !self.read && !self.search
    }
}

/// One sealed indexable domain: a NAME, the `$HOME`-relative subtree roots it groups, and the ceiling
/// governing them as a unit. Members are TEMPLATE fragments (`"Projects"`, `"Documents/Shrek notes"`)
/// with NO leading `/`: `swampd` joins them against the invoking user's home at load and canonicalizes.
/// A domain "combines knowledge, never authority" — membership groups trees for search reach; the
/// authority over each object stays attached at that object's physical home (filesystem-intelligence.md
/// §3). Compiled-in + sealed, so `swampd` resolves the set from HERE, never from a caller or a config.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IndexableDomain {
    pub name: &'static str,
    pub members: &'static [&'static str],
    pub ceiling: DomainCeiling,
    /// SWAMP SEMANTIC tier enablement for THIS domain (slice-4, swamp.md §8 "per-domain enablement").
    /// `true` ⇒ the embedder chunks+vectorizes this domain's readable text; `false` ⇒ metadata+FTS only
    /// (no embeddings — "nothing but metadata over ~/Media", filesystem-intelligence.md §6). This is an
    /// enrichment switch, NEVER an authority term: it only governs *which enrichment runs*, so a domain
    /// with `semantic:false` is fully FTS-searchable; it just contributes no vectors. The mandatory FTS
    /// floor is unconditional (search ceiling), independent of this flag.
    pub semantic: bool,
}

// ---- the sealed allow-set template --------------------------------------------------------------
// Authored as typed literals so the policy is sealed by compilation. `$HOME`-relative members only:
// the union of these (expanded + existing) is EXACTLY the Landlock allow-set `swampd` confines itself
// to before crawling (swamp.md §5). Extend the TEMPLATE here; per-machine additions are the deferred
// §5 counter-anchored grant path, NEVER a writable config.

/// The default template of swamp.md §5 ("e.g. ~/Projects, ~/Documents, ~/Downloads"). Every domain is
/// read-only intelligence at slice-1. Human-only trees (`~/Vault`, keys, identity) are ABSENT by
/// construction — they are not members, so default-deny covers them, and [`NEVER_INDEXABLE`] makes
/// the nested case authoritative too.
// `semantic`: SWAMP SEMANTIC is opt-in PER DOMAIN (swamp.md §8). Text-heavy domains that benefit from
// similarity — `projects`, `documents` — enable it; `downloads` (memes, installers, binaries) stays
// FTS+metadata only so a modest box does not burn CPU embedding junk (filesystem-intelligence.md §6).
// The FTS floor is unconditional regardless of this flag.
pub const INDEXABLE_DOMAINS: &[IndexableDomain] = &[
    IndexableDomain { name: "projects", members: &["Projects"], ceiling: DomainCeiling::RO_SEARCH, semantic: true },
    IndexableDomain { name: "documents", members: &["Documents"], ceiling: DomainCeiling::RO_SEARCH, semantic: true },
    IndexableDomain { name: "downloads", members: &["Downloads"], ceiling: DomainCeiling::RO_SEARCH, semantic: false },
];

/// Human-only path-COMPONENT markers. A path any of whose components equals one of these is NEVER
/// indexed — even nested under an indexable member. This is the authoritative "cannot be added by any
/// path" of security-model.md §5, and the crawler's skip for the nested case. Single-component markers
/// only (matched component-wise), so the check stays a pure, allocation-free scan.
pub const NEVER_INDEXABLE: &[&str] = &["Vault", ".ssh", ".gnupg", "Identity", ".shrek"];

/// Split a `$HOME`-relative path into non-empty components. Pure; borrows the input.
fn components(rel: &str) -> impl Iterator<Item = &str> {
    rel.split('/').filter(|c| !c.is_empty() && *c != ".")
}

/// Does `member` (a template fragment) prefix `rel` COMPONENT-WISE? `"Projects"` matches `"Projects"`
/// and `"Projects/a"` but NOT `"ProjectsX"` — a substring/byte-prefix match would leak an adjacent
/// tree, so the match is strictly per component.
fn member_prefixes(member: &str, rel: &str) -> bool {
    let mut r = components(rel);
    for m in components(member) {
        match r.next() {
            Some(c) if c == m => continue,
            _ => return false,
        }
    }
    true
}

/// Resolve an indexable-domain NAME to its sealed record. STRICT + FAIL-CLOSED, mirroring
/// [`crate::egress::resolve`]: an unrecognized name ⇒ `None`, and the caller MUST treat that as "no
/// domain / no authority", never as a wildcard.
pub fn resolve_domain(name: &str) -> Option<&'static IndexableDomain> {
    INDEXABLE_DOMAINS.iter().find(|d| d.name == name)
}

/// Is `rel` (a `$HOME`-relative path) barred from indexing by the human-only exclusion? True iff ANY
/// component is a [`NEVER_INDEXABLE`] marker. This holds regardless of domain membership — it is the
/// override that a permissive membership can never defeat (filesystem-intelligence.md §4).
pub fn is_never_indexable(rel: &str) -> bool {
    components(rel).any(|c| NEVER_INDEXABLE.contains(&c))
}

/// The indexable domain that owns `rel`, if any — the first domain with a member that component-wise
/// prefixes it. `None` means `rel` is outside the allow-set (default-deny). A never-indexable path
/// resolves to `None` here too, so this single function answers "may swampd map this object at all".
pub fn domain_for(rel: &str) -> Option<&'static IndexableDomain> {
    if is_never_indexable(rel) {
        return None;
    }
    INDEXABLE_DOMAINS
        .iter()
        .find(|d| d.members.iter().any(|m| member_prefixes(m, rel)))
}

/// May `swampd` index `rel` at all? True iff it falls under some domain member AND is not human-only.
/// This is the userspace mirror of the Landlock allow-set: the crawler consults it per object, and
/// the Landlock ruleset is built from the same [`INDEXABLE_DOMAINS`] members so the kernel enforces
/// the OUTER boundary independently.
pub fn is_indexable(rel: &str) -> bool {
    domain_for(rel).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_known_domains() {
        assert_eq!(resolve_domain("projects").unwrap().members, &["Projects"]);
        assert!(resolve_domain("documents").is_some());
        assert!(resolve_domain("downloads").is_some());
    }

    #[test]
    fn unknown_domain_fails_closed_to_none() {
        assert!(resolve_domain("vault").is_none());
        assert!(resolve_domain("").is_none());
        assert!(resolve_domain("Projects").is_none()); // case-sensitive, exact
    }

    #[test]
    fn member_match_is_component_wise_not_substring() {
        // The load-bearing anti-leak: an adjacent tree whose name merely SHARES a byte-prefix with a
        // member is NOT indexable. `ProjectsSecret` must not ride in on `Projects`.
        assert!(is_indexable("Projects"));
        assert!(is_indexable("Projects/shrek-os/docs/swamp.md"));
        assert!(!is_indexable("ProjectsSecret/leak.txt"));
        assert!(!is_indexable("Projectsx"));
    }

    #[test]
    fn multi_component_member_matches_component_wise() {
        // A hypothetical multi-component member matches only on a full component boundary.
        assert!(member_prefixes("Documents", "Documents/notes.md"));
        assert!(member_prefixes("Documents/Shrek notes", "Documents/Shrek notes/a.md"));
        assert!(!member_prefixes("Documents/Shrek notes", "Documents/Shrek notesX/a.md"));
        assert!(!member_prefixes("Documents/Shrek notes", "Documents"));
    }

    #[test]
    fn outside_the_allow_set_is_denied() {
        // Anything not under a domain member is denied by construction (default-deny). A fresh
        // top-level tree — a `~/NewSecrets` created after the ruleset was built — is unreadable
        // instantly, with zero policy change (security-model.md §5).
        assert!(!is_indexable("Vault"));
        assert!(!is_indexable("Vault/passwords.kdbx"));
        assert!(!is_indexable("NewSecrets/whatever"));
        assert!(!is_indexable(".ssh/id_ed25519"));
        assert!(!is_indexable("")); // $HOME itself is not a member
    }

    #[test]
    fn never_indexable_overrides_membership_when_nested() {
        // A human-only marker nested UNDER an indexable member is still barred — a permissive parent
        // (the indexable domain) can never re-grant it (filesystem-intelligence.md §4).
        assert!(is_never_indexable("Projects/app/.ssh/id_rsa"));
        assert!(!is_indexable("Projects/app/.ssh/id_rsa"));
        assert!(is_never_indexable("Documents/Vault/secret"));
        assert!(!is_indexable("Documents/Vault/secret"));
        // A normal file under the same member IS indexable.
        assert!(!is_never_indexable("Projects/app/src/main.rs"));
        assert!(is_indexable("Projects/app/src/main.rs"));
    }

    #[test]
    fn domain_for_returns_the_owning_domain_and_ceiling() {
        let d = domain_for("Projects/shrek-os/README.md").unwrap();
        assert_eq!(d.name, "projects");
        assert_eq!(d.ceiling, DomainCeiling::RO_SEARCH);
        assert!(domain_for("Vault/x").is_none());
    }

    #[test]
    fn ceiling_intersection_only_narrows() {
        let ro = DomainCeiling::RO_SEARCH;
        // Intersecting with itself is a no-op.
        assert_eq!(ro.intersect(ro), ro);
        // Intersecting with a narrower ceiling narrows.
        let read_only = DomainCeiling { discover: true, read: true, search: false };
        assert_eq!(ro.intersect(read_only), read_only);
        // Intersecting with NONE yields NONE (invisible), never widens back.
        assert!(ro.intersect(DomainCeiling::NONE).is_none());
        // Commutative, and can never produce a verb neither side had.
        let a = DomainCeiling { discover: true, read: false, search: true };
        let b = DomainCeiling { discover: true, read: true, search: false };
        assert_eq!(a.intersect(b), b.intersect(a));
        assert_eq!(a.intersect(b), DomainCeiling { discover: true, read: false, search: false });
    }

    #[test]
    fn none_ceiling_reveals_nothing() {
        assert!(DomainCeiling::NONE.is_none());
        assert!(!DomainCeiling::RO_SEARCH.is_none());
    }

    #[test]
    fn table_integrity_no_dup_names_valid_members() {
        for (i, a) in INDEXABLE_DOMAINS.iter().enumerate() {
            for b in &INDEXABLE_DOMAINS[i + 1..] {
                assert_ne!(a.name, b.name, "duplicate domain name {:?}", a.name);
            }
            assert!(!a.name.is_empty(), "empty domain name");
            assert!(!a.members.is_empty(), "domain {:?} has no members", a.name);
            for m in a.members {
                assert!(!m.is_empty(), "empty member in {:?}", a.name);
                assert!(!m.starts_with('/'), "member must be $HOME-relative (no leading /): {:?}", m);
                assert!(!m.contains(".."), "member must not escape home: {:?}", m);
            }
        }
    }

    #[test]
    fn semantic_is_per_domain_and_never_affects_the_search_ceiling() {
        // Text-heavy domains enable the semantic tier; downloads does not.
        assert!(resolve_domain("projects").unwrap().semantic);
        assert!(resolve_domain("documents").unwrap().semantic);
        assert!(!resolve_domain("downloads").unwrap().semantic);
        // The FTS floor is unconditional: EVERY domain still grants search regardless of the flag.
        for d in INDEXABLE_DOMAINS {
            assert!(d.ceiling.search, "domain {:?} must keep the FTS search ceiling", d.name);
        }
    }

    #[test]
    fn no_never_indexable_marker_is_also_a_domain_member() {
        // A human-only marker must never appear as (or under) a domain member — that would be a
        // self-contradiction the sealed table must not contain.
        for d in INDEXABLE_DOMAINS {
            for m in d.members {
                assert!(
                    !is_never_indexable(m),
                    "domain {:?} member {:?} collides with a NEVER_INDEXABLE marker",
                    d.name, m
                );
            }
        }
    }
}
