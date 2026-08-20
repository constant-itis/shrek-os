# Phase-6 (Swamp track) slice-1 — authority-filtered indexing + `shrek find`

> **Track note.** This is the **Swamp / semantic-filesystem** Phase-6 track (`swampd`), distinct from
> the completed **coding-agent enablement** Phase-6 track (slices 1a/1b/1c/2/3, `shrek run`). Both are
> "Phase 6"; they are different subsystems. This slice is the first vertical slice of the memory/
> knowledge side: the smallest end-to-end query path, and nothing more.

## What shipped

The first executable form of the system-wide invariant **semantic authority ≤ data authority**
(architecture.md §5, filesystem-intelligence.md §0, security-model.md §5): a caller can reach a byte
by *inference* — an index hit, an FTS token — only where it could reach that byte *directly*. Proven
the hard way: **prove the negative** — an ungranted tree's files AND their FTS-derived data are
undiscoverable, not filtered-after.

Two walls, both enforced and both gate-proven:

1. **Confinement (swamp.md §5).** `swampd` Landlocks *itself* default-deny to the sealed indexable
   allow-set before it reads one byte of user data. A protected tree's bytes never enter its address
   space — the confused-deputy defence is a kernel fact, not a config discipline.
2. **Query gate (swamp.md §9).** Every `shrek find` is caller-scoped, **authorize-before-retrieve**:
   the candidate set is built from `session-grants ∩ sealed-allow-set ∩ per-object-domain-ceiling`
   *before* FTS retrieval. Out-of-authority objects are absent from the projection — never returned,
   never counted, never inferable.

Explicitly OUT of this slice (not built, not scaffolded): embeddings/semantic tier, LLM summaries, the
relationship/living graph, the taint plane, any UI, and the live self-repairing fanotify watcher (§6 —
v1 is a one-shot crawl at start).

## The authority model (the crux, as decided)

A `shrek find` query carries a **session HANDLE**, never grants. Authority is resolved from privileged
state, independently — the "resolve from sealed policy, NOT from the request" of swamp.md §9, realized
against the shipped per-session anchor+grant model rather than a (non-existent) uid→domain table:

```
gatekeeperd (session construction)                swampd (query)
──────────────────────────────────                ──────────────
`authority-record` writes the session's           SO_PEERCRED authenticates the peer UID
CANONICAL grants to a root-owned record:            → IDENTITY only, NEVER authority
  /run/shrek/authority/<session>                   load_grants(<handle>) from the ROOT-OWNED record
  root:swamp 0640, in a root:swamp 0750 dir          → trusts the RECORD, never the caller's claim
  (untrusted workload cannot forge or widen)       candidate = grants ∩ sealed-allow-set ∩ ceiling
                                                    optional --scope may only NARROW the grants
                                                    FTS retrieval runs INSIDE the candidate
```

- **SO_PEERCRED = identity, the record = authority.** The socket peer's UID authenticates *who* asks
  (coarse connect allow-list); *what* they may reach is the independently-resolved session record.
- **The scope selector can only narrow.** `--scope P` is honoured iff `P` is component-wise beneath a
  session grant; otherwise the effective set is empty. A selector never widens authority.
- **Fail-closed = empty, not error.** A missing/malformed record, an unknown handle, empty grants, or
  a domain whose ceiling denies the verb all yield an empty projection — indistinguishable from "no
  match", so a caller cannot probe which sessions exist.
- **Minimal + ephemeral.** The record is set once at construction and removed at teardown (`--rm`). No
  general grant protocol — that is a later slice.

## Components

| Piece | Where | Role |
|-------|-------|------|
| Sealed indexable allow-set | `shrek-policy/src/swamp.rs` | `INDEXABLE_DOMAINS` (name → `$HOME`-relative members + `DomainCeiling`) + `NEVER_INDEXABLE` markers. Pure policy DATA, compiled-in + dm-verity-sealed, resolved independently. Mirrors `egress.rs`. |
| Authority record writer | `gatekeeperd/src/authority_record.rs` | Privileged writer of the root-owned session grant record. New `gatekeeperd authority-record` subcommand — does NOT touch the frozen sandbox constructor or the merge path. |
| Landlock self-confinement | `swampd/src/confine.rs` (+ `linux_uapi.rs`) | Builds one default-deny ruleset from the allow-set + operational dirs and `landlock_restrict_self`. Reuses gatekeeperd's proven Landlock ABI (minimal local mirror, no refactor of the frozen privileged crate). |
| Index store | `swampd/src/index.rs` | rusqlite (bundled SQLite + FTS5). Metadata/structural tables + FTS5. `Index::query` is the authorize-before-retrieve spine. |
| Crawl | `swampd/src/crawl.rs` | One-shot Phase-1 metadata + Phase-2 FTS over the allow-set member trees; prunes `NEVER_INDEXABLE` + build/cache dirs; no symlink following. |
| Authority reader | `swampd/src/authority.rs` | Loads a session's canonical grants from the root-owned record; fail-closed to empty. |
| Query server | `swampd/src/server.rs` | Root-owned unix socket, SO_PEERCRED, dep-free line-text wire protocol; applies scope-narrow + domain-ceiling; returns the projection. |
| `shrek find` | `shrek/src/main.rs` | User/agent front door (sibling of `shrek run`). Carries handle+query, prints the projection. std-only. |
| Oracle | `scripts/swamp-find-proof.sh` | The end-to-end negative proof (below). |

## Decisions (the three forks, as adjudicated)

1. **Authority = session-bound handle** (not request-supplied anchor/grants, not uid→global-domain).
   gatekeeperd/`shrek run` records canonical grants; swampd resolves independently; scope narrows only.
2. **Storage = rusqlite + bundled SQLite + FTS5**, vendored in-tree for a hermetic offline build,
   scoped EXCLUSIVELY to swampd (the availability-plane daemon, off the sealed image's wall/egress
   path). No hand-rolled index. The deliberate scoped exception, like tinyjson (coder) / rustls (proxy).
3. **Confinement now = kernel Landlock**, not deferred. swampd Landlocks itself to the allow-set before
   crawl; the query gate is the retrieval boundary. Acceptance proves BOTH the kernel-boundary open()
   denial and that Vault-derived metadata/FTS never enter the index or results. Reused the existing
   Landlock primitives; built no generalized new sandbox framework.

## Proof — `scripts/swamp-find-proof.sh` (privileged debian:trixie oracle, default bridge)

Seeds two indexable projects (`app-a`, `app-b`, both crawled) + a `~/Vault` (never a member), all
world-readable so DAC is *not* the denier. **20 gates, 0 fail:**

- **Confinement (kernel):** `confine-probe` enforces the real allow-set, then `open(~/Vault/passwords)`
  → `DENIED 13` (EACCES) at the kernel boundary though DAC would allow it; `$HOME` root denied
  (not a member); an allow-set member + `/etc/passwd` open OK.
- **Query gate:** session A (grant = app-a) sees only app-a; app-b (indexed, out of scope) and Vault
  (never indexed) are absent. Session B symmetric. FTS token isolation: `BBSECRET` (app-b, out of
  scope) and `VAULTSECRET`/`hunter2` (Vault, never indexed) are undiscoverable from A. Scope cannot
  widen to app-b but narrows within the grant. Unknown session sees nothing. `discover` (path) intent
  scoped identically. The root-owned record is unreadable by the untrusted `tester` uid.

Unit coverage: `shrek-policy` (12 swamp-table tests incl. component-wise membership + never-indexable
override), `swampd` (index scope/FTS isolation, authority parse, scope-narrow, Landlock ABI struct
sizes), `gatekeeperd` (authority-record roundtrip + session-id traversal guard). Full workspace green.

VM-confirm is not required this slice: swampd is availability-plane (fail-open §10), off the sealed
boot path; the Landlock boundary is a runtime kernel property the oracle proves directly.

## Honest scope / residuals

- **Reference leakage (unchanged, security-model §5 A5-disc).** The byte-wall + query gate stop A1
  bytes and their tokens; a *readable* in-scope file that merely *names* an out-of-scope path is still
  readable. The wall protects bytes, not references to them held elsewhere.
- **v1 index is a point-in-time snapshot** rebuilt at swampd start (volatile `/run`), no live watcher
  (§6) and no persistence — deferred, matching swamp.md §11.
- **Object `id` = `(dev,ino)`** for v1; the opaque rename-surviving id is deferred (§3).
- **In-sandbox querying deferred.** Slice-1 `shrek find` runs host-side with a session handle; a query
  from *inside* a gVisor T2 wall needs the broker-routed IPC (the parked Path-2), out of this slice.
- **`linux_uapi` duplication.** swampd carries a minimal mirror of gatekeeperd's proven Landlock/
  peercred ABI rather than a shared crate, to keep blast radius off the frozen privileged broker.
  Factoring a shared `shrek-sys` crate is a follow (it would touch gatekeeperd).
- **§7 exclusions** are pruned entirely at v1 (mapped-but-not-enriched nuance deferred); FTS extraction
  is a UTF-8/no-NUL heuristic capped at 512 KiB.

## Next candidates (Swamp track, none in-flight)

SWAMP SEARCH as a separately-installable unit (§8 per-domain enablement); the live fanotify event
pipeline + index persistence (§6); the semantic/embedding tier; broker-routed in-sandbox `shrek find`;
per-machine counter-anchored allow-set additions (§5 grant path); the relationship/living graph (its
own threat pass first, §8).
