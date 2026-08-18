# Shrek OS — The Swamp (`swampd` component)

> The swamp is nobody's. It belongs to everybody. That is exactly why it must be Landlocked.

This is the implementation spec for `swampd`, the daemon that builds and serves Shrek's
filesystem intelligence. Where [`filesystem-intelligence.md`](filesystem-intelligence.md) fixes
the *model* (three maps, logical domains, the escalation ladder) and
[`architecture.md`](architecture.md) §5 introduces the confused-deputy problem, this document
makes the component buildable: the daemon shape, the object record, the on-disk indexes, the
event pipeline, the initial crawl, and — the spine of the whole thing — the **kernel
confinement** that keeps the index from becoming a side channel around the file wall.

`swampd` is the most dangerous process in the system (architecture.md §5). To build an index it
reads everything it is *allowed* to read; anything that can query the index can potentially
reach what it read. Every design choice below is subordinate to one property:

```
swampd is a SUBJECT of the deterministic policy, not an exception to it.
Its read-scope IS the kernel-enforced allow-set — never "trusted to skip the secrets."
A swampd compromise leaks NOTHING from a protected domain, because those bytes
never entered its address space.        (architecture.md §5, security-model.md §5)
```

---

## 1. Scope & non-goals

**This document owns the component:** the daemon and its workers, the record schema, the three
indexes, event coalescing, the initial crawl, the confinement mechanism, and the query API
surface.

**It does not own the model.** The three maps, logical domains, inherited policy, and the
escalation ladder are [`filesystem-intelligence.md`](filesystem-intelligence.md). Read that for
*what* the intelligence is; this doc is *how it is built*.

**It does not re-derive the wall.** *Which* trees `swampd` may open is decided by the sealed
policy plane and compiled to a Landlock allow-set ([`security-model.md`](security-model.md) §5).
This doc consumes that allow-set and enforces itself under it; it does not author policy.

**It does not own agent execution.** How an agent's request reaches the query API, and how a
write it decides on is brokered, live in [`agents.md`](agents.md),
[`grant-protocol.md`](grant-protocol.md), and [`isolation.md`](isolation.md). `swampd` answers
queries; it never runs agent code and never applies a write itself.

## 2. `swampd` — one daemon, not fifty thousand managers

The filesystem tree is the org chart; `swampd` is the single corporate HQ that keeps the org's
records. There is **no per-directory daemon.** One supervised service maintains a hierarchical
map of the whole (allowed) filesystem, using a small pool of workers for the expensive
enrichment stages.

```
swampd (systemd-supervised, User=swamp, Landlocked default-deny — §5)
├── watcher      coalesced fanotify/inotify events → work queue        (cheap, always)
├── mapper       physical + structural map: path, inode, repo, domain  (cheap, always)
├── worker pool  enrichment, tier-gated (§8):
│     · extractor   text extraction → FTS
│     · embedder    embeddings → vector index      (opt tier)
│     · relater     relationship edges             (opt tier)
└── query server  caller-scoped read API over a unix socket            (§9)
```

- **Unprivileged.** Runs as its own `swamp` user with no capabilities beyond file reads inside
  the allow-set. It is not root, not `CAP_DAC_READ_SEARCH` — it reads only what Landlock and
  Unix perms jointly permit.
- **Availability-plane only.** `swampd` is on the fail-open plane (§10): if it dies, the OS is
  fully usable and only enhanced search disappears. It is never on the path of boot, login, the
  VFS, or agent sandbox construction.
- **Workers are enrichment, not authority.** A worker can compute an embedding or a hash; no
  worker widens what the daemon may read. All workers inherit the same allow-set.

## 3. The object record

Every mapped object has one record. The record carries the fields for all tiers, including the
*deferred* living-graph fields — present in the schema now, **inert until that tier ships**
([`filesystem-intelligence.md`](filesystem-intelligence.md) §8), so enabling it later is a
migration of behavior, not of schema.

```
object
  id              stable opaque id (survives rename/move)
  path            current physical path            ── PHYSICAL MAP
  inode, dev      identity across renames
  mount, size, mtime, owner, mode
  ── STRUCTURAL MAP ──
  parent          containing object id
  repo, branch    if under version control
  project         detected project boundary
  domains[]       logical domains this object belongs to (filesystem-intelligence.md §3)
  mime, type
  ── SEMANTIC MAP (opt tiers, §8) ──
  content_hash    for change detection + provenance
  fts_doc         extracted searchable text            (FTS tier)
  embedding       vector id                            (semantic tier)
  classification  advisory content class (tripwire input, §7 architecture.md) — NEVER the wall
  relationships[] edges → other object ids             (relationship tier)
  ── LIVING GRAPH (deferred, inert — filesystem-intelligence.md §8) ──
  edge_weights    reserved
  coaccess_count  reserved
  last_access     reserved
  ── PROVENANCE (architecture.md §8) ──
  provenance[]    append-only: prev_hash→new_hash, actor, model, op, caps, net, ts
```

The `id` is deliberately not the path: rename/move updates `path` and keeps relationships and
provenance intact. `classification` is an *input to the semantic tripwire* only — never
consulted as an access decision; the wall is the allow-set, not a content class
([`security-model.md`](security-model.md) §7).

## 4. The three indexes

One embedded store, three logical indexes, matching the three maps. SQLite is the metadata and
FTS backend (architecture.md §5); the vector index is a separate opt-in store so a light
install carries none of its weight.

| Index | Backs | Engine | Tier |
|-------|-------|--------|------|
| Metadata | physical + structural map | SQLite tables | mandatory core |
| FTS | full-text search | SQLite FTS5 | opt |
| Vector | semantic similarity | separate vector store | opt |

The filesystem stays authoritative. The index is a *cache of derived facts about* the
filesystem; if it and the filesystem disagree, the filesystem wins and the record is repaired
(§6). Nothing in Shrek treats the index as a source of truth for file *contents* — it is a map,
not a mirror.

## 5. Confinement — the spine

`swampd`'s read-scope is a **default-deny Landlock allow-list** generated from the sealed
policy, applied to the daemon itself and inherited by every worker. This is stronger than a
"don't index these paths" exclusion, which can be misconfigured into a leak:

```
GENERATED RULESET (default-deny)
  ALLOW read on:  the explicit set of indexable trees (the union of enabled domains' members)
  DENY  (implicit): EVERYTHING ELSE — including any human-only domain, and any directory
                    created later that is not on the allow-set.

  swampd is not "trusted to skip ~/Vault." swampd is PHYSICALLY INCAPABLE of opening it:
  the open() never succeeds, so the bytes never reach its address space.
```

Consequences that fall out of default-deny, not out of configuration diligence:

- **New secrets are safe instantly.** A freshly-created `~/NewSecrets/` is unreadable to
  `swampd` the moment it exists, with no config change — it is simply not on the allow-set, and
  default-deny covers the gap. A deny-*list* would have to be updated and would leak until it
  was ([`architecture.md`](architecture.md) §5).
- **The allow-set is the union of enabled domains only.** A domain that *names* a tree
  `swampd` may not read (per the most-restrictive-wins composition of
  [`filesystem-intelligence.md`](filesystem-intelligence.md) §3) contributes nothing to the
  allow-set — there is no map of that tree to leak.
- **Static core, counter-anchored additions.** The never-indexable human-only exclusions and
  the allow-set template are sealed static policy baked into the image under dm-verity.
  Per-machine additions go through the counter-anchored grant path, not a writable config file
  ([`security-model.md`](security-model.md) §4–§5, [`grant-protocol.md`](grant-protocol.md)).
- **Confinement is not a tier.** Every point in §8's modular stack — even the bare metadata
  core — runs inside this allow-set. There is no configuration, however minimal, in which
  `swampd` runs unconfined.

## 6. Event pipeline — the map repairs itself

`swampd` does not re-crawl to stay current. It watches coalesced filesystem events and repairs
the affected records incrementally, so the map tracks reality continuously.

```
kernel fs event (coalesced: burst of writes → one job)
        │
        ▼
  watcher → work queue → mapper (physical/structural, always)
                              │
                              ▼  (tier-gated, §8)
                    content_hash recomputed
                    fts_doc re-extracted            (FTS tier)
                    embedding marked stale → re-embed (semantic tier)
                    relationships re-evaluated       (relationship tier)
                    provenance appended (architecture.md §8)
```

- **Coalesced, not per-syscall.** A file being written in a tight loop generates one enrichment
  job after the burst settles, not one per write — the watcher debounces before enqueuing.
- **Cheap map first, expensive enrichment lazily.** The physical/structural update is immediate
  and always runs; embeddings and relationships are recomputed by the worker pool as capacity
  allows and only for enabled tiers. A stale embedding degrades ranking, never correctness (§10).
- **Events outside the allow-set never arrive.** The watcher is registered only for allowed
  trees; a write under a denied domain produces no job because `swampd` cannot watch what it
  cannot read.

## 7. Initial mapping — the crawl

On first run for a user (or when a tree joins an enabled domain), `swampd` builds the map in
phases, cheapest first, so the machine is usable immediately and enrichment fills in behind.

```
Phase 1  (fast, always)     paths · filenames · mime · size · mtime · owner/mode
                            repo/project detection · domain membership
Phase 2  (FTS tier)         extract searchable text → FTS
Phase 3  (core)             content hashes · relationships (structural) · provenance init
Phase 4  (semantic tier)    embeddings · classification · semantic relationships
```

Exclusions, to avoid burning the machine enriching noise — treated as **opaque metadata only**
(present in the physical map, never text-extracted or embedded):

```
node_modules/ · target/ · .cache/ · Steam/ · VM disk images · build output · large binaries
```

These are still *mapped* (they exist in the physical map, so search can find them by path), but
not *enriched*. And the exclusion is a performance choice layered *on top of* the allow-set — it
narrows what is enriched within readable trees; it never widens what is readable.

## 8. Modularity — installable tiers, light by default

The enrichment stack is the tiered, opt-in structure of
[`filesystem-intelligence.md`](filesystem-intelligence.md) §6, realized as independently
installable units. A base install carries the metadata core and nothing else; richer tiers are
separate packages/layers a person adds by need — and can enable per domain.

| Tier | Unit | Adds | Default |
|------|------|------|---------|
| Metadata core | `swampd` | physical+structural map, path/metadata search | installed |
| FTS | extractor + FTS5 | full-text search | opt-in |
| Semantic | embedder + vector store | embeddings, similarity | opt-in |
| Relationships | relater | semantic-map graph | opt-in |
| Living graph | (mycelium engine, adapted) | co-access, decay, reinforcement | **deferred** |

- **Per-domain enablement.** A person can run the semantic tier over `~/Projects` and metadata-
  only over `~/Media`. The tier config is per-domain, so "better in the areas that need it" is a
  supported configuration, not a hack.
- **Graceful degradation.** A missing tier is skipped by the query planner, which falls to the
  best lower tier ([`filesystem-intelligence.md`](filesystem-intelligence.md) §5). Uninstalling
  the semantic tier makes search less clever, never wrong.
- **Living-graph upgrade path.** The deferred tier reuses the existing mycelium FOSS engine,
  adapted to run *inside* `swampd`'s confinement (§5) rather than as a trusted service. The
  record already reserves its fields (§3), so it lands as a behavior migration. Deferred per
  [`filesystem-intelligence.md`](filesystem-intelligence.md) §8 — a self-reinforcing access-
  pattern graph is itself an asset that earns its own threat pass first.
- **Modularity stops at the wall.** Every tier, installed or not, runs under the §5 allow-set.
  What is modular is *which enrichment runs*; that `swampd` is a confined subject is not.

## 9. Query API — caller-scoped, authorize-before-retrieve

`swampd` serves reads over a root-owned unix socket. Every request carries the caller identity
(kernel-attested via `SO_PEERCRED`, the mechanism `gatekeeperd` already uses,
[`grant-protocol.md`](grant-protocol.md)); `swampd` never trusts a caller-supplied identity.

```
request:  { caller, intent: discover|search|read, query, domain_hint? }
             │
             ▼
   resolve caller's AUTHORIZED domains  ← from sealed policy, NOT from the request
             │
             ▼
   escalation ladder (filesystem-intelligence.md §5), SCOPED to those domains from the start
             │
             ▼
   results drawn only from within authorized scope
```

- **Authorize before retrieve.** The authorized domain set is an *input* to the planner, not a
  filter on its output. `swampd` never runs a global search then filters — a post-filter is a
  leak waiting for a bug ([`architecture.md`](architecture.md) §5,
  [`security-model.md`](security-model.md) §5).
- **`discover:false` is honored in the index.** For a domain the caller lacks `discover` on,
  matching objects are absent from results entirely — not returned-and-marked. The caller cannot
  learn the object exists (the deterministic guarantee of architecture.md §6, within `swampd`'s
  scope; the readable-file-that-merely-names-it caveat of §6 still applies).
- **The API reads; it never writes files.** A query can return "here is the object to edit";
  the edit itself is an agent action brokered through `agentd`/`gatekeeperd`
  ([`isolation.md`](isolation.md), [`agents.md`](agents.md)), which then generates the fs event
  that repairs the map (§6). `swampd` is never in the write path.

## 10. Failure behavior — availability plane, fails open

`swampd` sits squarely on the availability plane of the two-plane model
([`architecture.md`](architecture.md) §9, [`security-model.md`](security-model.md) §7):

```
systemctl stop swampd
  → boot, login, desktop, VFS, networking, apps, layers, shell, dev work: ALL STILL WORK.
  → only enhanced capability disappears: semantic search, relationships, auto-embeddings.

This plane fails OPEN by design. It is NOT license for the agent-execution plane to fail open:
swampd being down never grants an agent a byte. A dead index means "search is dumber now,"
which is the SAFE direction — fewer inferences available, never more authority.
```

The failure direction is always toward *less* capability, never *more* authority — a down or
degraded `swampd` can only fail to surface something, never surface something it shouldn't.
That asymmetry is why the whole component is allowed to be best-effort: correctness of the wall
never depends on it running.

## 11. Deferred

- **Living graph** — co-access strengthening, decay, reinforcement; fields reserved in §3,
  engine is the adapted mycelium FOSS core, gated on its own threat pass
  ([`filesystem-intelligence.md`](filesystem-intelligence.md) §8).
- **Richer query IPC** — v1 is a simple line/socket read API. A varlink surface and streaming
  results are a later refinement, mirroring the `gatekeeperd` IPC evolution.
- **Cross-machine domains** — single-machine only at v1 (filesystem-intelligence.md §8).
- **Learned crawl/enrichment prioritization** — Phase-order and exclusions (§7) are fixed
  heuristics; learning which trees to enrich first is a later optimization and must keep the
  fixed order as fallback.
- **erofs/read-only content addressing for the index store** — the index is mutable state on
  the volatile/writable plane at v1; content-addressed sealing is deferred with the broader
  writable-state design.

Every deferral is built, if at all, as a further subject of the §5 confinement — never as an
exception to it.
