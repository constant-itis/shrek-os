# Shrek OS — Filesystem intelligence (the model)

> Donkey doesn't memorize the swamp. He asks the swamp, and the swamp remembers.

This document owns the *conceptual model* of Shrek's filesystem intelligence: what it means
for the machine to have a continuously-maintained, queryable understanding of its own storage,
and the invariants that model must never break. Where [`architecture.md`](architecture.md) §5
*introduces* the Swamp and §6 the capability vocabulary, and [`swamp.md`](swamp.md) specs the
`swampd` *component* that implements this, this doc fixes the **shape of the idea** the
component is built to satisfy: the three maps, logical domains, inherited policy, and the
search-escalation ladder.

It is not a security document of its own. Every access rule here is a restatement of the
deterministic wall defined in [`security-model.md`](security-model.md) §4–§5 and
[`architecture.md`](architecture.md) §6–§7. When this doc and the security model appear to
disagree, the security model wins.

The invariant everything below serves:

```
semantic authority ≤ data authority          (system-wide, architecture.md §5)
  An agent can reach a byte by INFERENCE (index / embedding / summary / relationship)
  only where it could already reach that byte DIRECTLY (VFS). The intelligence layer
  never widens authority; it only makes already-authorized data easier to find.
```

---

## 1. Scope & non-goals

**This document owns the model:** the three superimposed maps, the notion of a *logical
domain* decoupled from physical paths, how policy is inherited down a domain, and the
tiered search-escalation ladder that answers a query with the cheapest sufficient method.

**It does not own the component.** The daemon, its worker model, the on-disk index schema,
event coalescing, the initial crawl, and — critically — the kernel *confinement* of `swampd`
live in [`swamp.md`](swamp.md). Read this for *what the intelligence is*; read that for *how
it is built without becoming a confused deputy*.

**It does not own the wall.** *Whether* an agent may read a tree is decided by the Landlock
allow-set compiled from its capability profile (§6, [`security-model.md`](security-model.md)
§5). This doc assumes those decisions as input and is careful never to describe a feature that
would route around them.

**Non-goal: a general-purpose portable index service.** The intelligence layer is welded to
this OS's sealed policy plane, not an app you point at arbitrary storage. The reasoning is in
§7; it is the single most important line in this document.

## 2. Three maps, superimposed

The filesystem is understood through three maps over the same objects. A query may touch one,
two, or all three. They are layered, not alternatives.

```
PHYSICAL MAP     where the bytes are        /home/sebastian/Projects/shrek-os/docs/isolation.md
                 path · inode · mount · size · mtime · owner · mode

STRUCTURAL MAP   how it is organized        project → docs → security docs → isolation.md
                 parent/child · repo · branch · project · logical domain (§3)

SEMANTIC MAP     what it is about           isolation.md ↔ sandboxing ↔ Landlock ↔ capability profiles
                 extracted text · embedding · relationships · classification
```

- The **physical map** is authoritative and always present — it is just the filesystem, read
  cheaply. It never lies because it *is* the ground truth being described.
- The **structural map** is derived and cheap: repo detection, project boundaries, the
  parent/child tree, and the logical-domain overlay of §3.
- The **semantic map** is derived and expensive, and therefore **optional and tiered** (§6).
  Its absence degrades capability, never correctness (§8, and [`architecture.md`](architecture.md) §9).

An agent authorized for a domain may reason over all three maps *of that domain*. It never sees
any map — not even the physical one — of a domain outside its authority. `discover: false`
(§6, architecture.md) means the object's existence is absent from every map the agent can query.

## 3. Logical domains — meaning decoupled from physical paths

The Unix hierarchy is where bytes live, not how a person thinks. One project is routinely
scattered:

```
/home/sebastian/Projects/shrek-os      source
/home/sebastian/Documents/Shrek notes  documentation
/mnt/nas/shrek-assets                  assets
```

Physically three trees; semantically one thing. A **logical domain** is a named set of physical
subtrees plus the policy that governs them as a unit:

```
domain project:shrek-os
  members:  ~/Projects/shrek-os
            ~/Documents/Shrek notes
            /mnt/nas/shrek-assets
  policy:   agents = discover + read + search        (default for the domain)
```

This is the property that makes the intelligence layer worth more than `locate`: *"everything
related to Shrek OS isolation"* resolves across all three trees, the repo, prior artifacts, and
the relationship graph — not just filename matches under one path.

**Domains are an overlay on the physical map, never a replacement for it.** Every object still
has exactly one physical home; a domain is a *view* that groups homes. This matters for the
wall:

```
AUTHORITY COMPOSITION (load-bearing — a domain must never launder authority)

  A domain's effective read-scope, and any agent capability scoped to the domain,
  is the UNION of its member subtrees — but resolved against each member's OWN policy,
  and on any conflict MOST-RESTRICTIVE WINS.

  A permissive member can NEVER lift a stricter member's denial. If /mnt/nas/shrek-assets
  is agent-readable but ~/Documents/Shrek notes contains a subtree marked agents=DENY,
  membership in project:shrek-os does not grant the agent that subtree. The deny survives
  the union.
```

Restated: a domain composes *reach* (you can search across trees at once) but composes
*authority downward-only* — exactly the rule §6 already fixes for the deny-list. Grouping trees
is a convenience for the human and the search planner; it is never a mechanism for widening what
the kernel allow-set permits. `swampd`'s own confinement (`swamp.md` §5) enforces this from
below: it can only index the trees on its allow-set, so a domain that *names* a forbidden tree
simply has no map of it to expose.

## 4. Inherited policy — narrow, never widen

Domains and directories carry policy that flows to their children, in the manager/org-chart
sense: a region sets a default, a district narrows it, an object may carry an exception. But the
flow is strictly one-directional in authority.

```
Projects/                        agents = discover + search + read      (default)
├── shrek-os/                    ← inherits
│   ├── docs/                    ← inherits
│   └── src/                     ← inherits
└── ClientFoo/
    └── secrets/                 ← OVERRIDE: agents = NONE              (narrows)
```

The rule, identical to the deny-list discipline in [`architecture.md`](architecture.md) §6:

```
Child policy may NARROW inherited authority. It can NEVER grant authority the parent forbids.
A permissive tag on a file inside a denied domain does nothing.

  ~/Private/                       agents = NONE
  ~/Private/safe-for-agents.txt    (tag: agents = read)   ← INERT. Still denied.

Because the effective ruleset compiles to a DEFAULT-DENY Landlock allow-set (§6): a path
reachable only via a tag the parent's deny already covers is never added to the allow-set,
so the kernel denies it regardless of the tag.
```

This is why inherited policy is safe to make ergonomic: the human writes intent as
inheritance and overrides, but the *enforcement* is a compiled default-deny allow-list on the
kernel, not a walk of tags at query time. A misplaced permissive tag is a no-op, not a breach.

## 5. The search-escalation ladder

A query is answered by the **cheapest method that suffices**, escalating only when the cheaper
tier comes up short. Most queries never reach the expensive tiers — and the expensive tiers are
exactly the optional ones (§6), so a light install still answers most questions.

```
INTENT: "the doc where we worked out the isolation tiers for agents"

  0. Domain / context     already know it's project:shrek-os          ← free
  1. Path & metadata      filename/extension/mtime match              ← cheap, always present
  2. FTS                  full-text term match                        ← opt tier
  3. Semantic / vector    embedding similarity                        ← opt tier
  4. Relationship graph   "related to isolation, agents, sandboxing"  ← opt tier
  5. LLM read             model reads candidate sections              ← last resort, rare

  Escalate to tier N+1 only if tier N returns too little. Stop at the first sufficient tier.
```

Two non-negotiable properties of the ladder, both from the security model:

- **Authorize before retrieve — never the reverse.** The planner scopes to the caller's
  authorized domains *first*, then searches within them. It never runs a global search, ranks
  hits, and filters afterward — a post-filter is a leak waiting for a bug
  ([`architecture.md`](architecture.md) §5, [`security-model.md`](security-model.md) §5). The
  authorized domain is an input to the planner, not a filter on its output.
- **The ladder degrades, it does not fail.** If a tier is not installed, the planner skips it
  and uses the best available lower tier. Absent embeddings means "search is less clever
  today," never "search is wrong" (§8).

`swamp.md` §9 specs the query API that carries the caller identity into this planner; this doc
fixes only the ordering and the two properties above.

## 6. The intelligence stack is modular and light by default

The maps of §2 are not a monolith. They are a stack of tiers a person opts into by need. A
light box runs the mandatory core and stops; a power user enables the whole stack. Each tier is
independently installable and each degrades gracefully to the tier below it.

| Tier | Provides | Cost | Status |
|------|----------|------|--------|
| **Metadata core** | physical map, structural map, path/metadata search | cheap, always on | **mandatory** |
| **FTS** | full-text search over extracted text | moderate | opt-in |
| **Semantic** | embeddings, vector similarity | heavy (model + storage) | opt-in |
| **Relationships** | the semantic-map graph between objects | moderate over semantic | opt-in |
| **Living graph** | co-access strengthening, decay, reinforcement | — | **deferred** (§8) |

Design consequences:

- **Light by default.** A base install has the metadata core only: it maps the filesystem,
  detects projects and domains, and answers path/metadata queries. Everything richer is a
  deliberate opt-in, so Shrek does not roast a modest machine's CPU building embeddings for
  every meme in `~/Downloads`.
- **Modular by area, not just on/off.** Tiers can be enabled per domain — a person may want
  full semantic search over `~/Projects` and nothing but metadata over `~/Media`. "Works good,
  or works better in different areas depending on need" is a first-class configuration, not a
  workaround.
- **Modularity never touches the wall.** This is the one hard limit. A person may drop the
  semantic tier, the FTS tier, the whole enrichment stack — but may **not** detach `swampd`
  from its Landlock allow-set. Choosing *which enrichment runs* is user policy; *whether
  `swampd` is a kernel-confined subject* is not negotiable (§7, `swamp.md` §5). Every tier,
  installed or not, runs inside the same allow-set.

## 7. Why this is welded to the OS, not a portable DB

The tempting design is a general, attachable index service — point it at any directory, like a
personal-memory app you install and feed. **Shrek's intelligence layer deliberately is not
that, and cannot be, without voiding the security model.**

```
mycelium-style trusted app          Shrek filesystem intelligence
────────────────────────            ─────────────────────────────
you HAND it your data               it is a SUBJECT of the kernel policy
trusted to hold everything          Landlocked default-deny; physically cannot open ~/Vault
detachable, portable sidecar        welded to the sealed policy plane; part of the OS
its read-scope = whatever you feed   its read-scope = the kernel-enforced allow-set (swamp.md §5)
```

The index is a **side channel around the file wall**: guarding `~/Vault/foo.pdf` at the VFS is
worthless if a detachable indexer already slurped it and an agent can query the embedding
([`architecture.md`](architecture.md) §5). The only reason the Swamp is safe is that its
read-scope *is* the allow-set — a compromise leaks nothing because the protected bytes never
entered its address space. An attachable, point-it-anywhere DB is a confused deputy *by
construction*: it reopens exactly the side channel §5 exists to close.

Hence the rule that governs the whole design:

```
Borrow the DATA model of a rich personal-memory graph (relationships, retrieval, the three maps).
REJECT its TRUST model. The Swamp is an OS component with a graph-shaped brain,
not a memory app that happens to run on the OS.
```

The data-model richness — a relationship graph, tiered retrieval, eventually the living
reinforcement of §8 — is welcome and is the point of §2. What is refused is the *trust posture*
that would let that brain read outside the kernel fence.

## 8. Deferred

- **Living graph (co-access strengthening, decay, path reinforcement).** The relationship map
  of §2 is static at v1: edges are derived from content and structure, not from access
  behavior. The *fields* for a living graph — edge weights, co-access counts, last-access
  timestamps — are reserved in the object record now (`swamp.md` §3) but inert. The upgrade
  path is the existing mycelium FOSS engine, adapted to run *inside* `swampd`'s confinement
  rather than as a trusted service (§7). Deferred because a self-reinforcing graph means
  `swampd` writes behavioral data about the human's access patterns — itself an asset an
  adversary would want — and that surface earns its own threat pass before it ships.
- **Cross-machine / shared domains.** Domains (§3) are single-machine at v1. A domain spanning
  hosts multiplies the confused-deputy surface and is out of scope until the single-machine
  model is proven.
- **Learned tier-escalation.** The ladder (§5) escalates on fixed sufficiency thresholds. A
  planner that *learns* when to escalate is a later optimization; the fixed ladder must work
  first and remain the fallback.
- **Per-domain semantic model selection.** §6 allows enabling the semantic tier per domain;
  choosing *different embedding models* per domain (e.g. code vs prose) is deferred.

None of these deferrals weaken the invariant of §0/§7: every one of them, if built, is built
as a further subject of the same kernel confinement, never as an exception to it.
