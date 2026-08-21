# Phase-6 Swamp slice-3 — live watcher + persistent index (design-of-record)

Status: IN PROGRESS. Supersedes the slice-1/-2 "point-in-time snapshot rebuilt at start" model
(`docs/phase6-swamp-slice1-query-gate.md`, `docs/phase6-swamp-slice2-broker-routed-find.md`).

## 1. What slice-3 changes — and the one thing it must not

Before: `swampd` deleted `index.db` on every start, crawled once into `/run/swamp` (tmpfs), and served
a **frozen** snapshot. The map went stale the instant the coder (or anything) touched the tree.

Slice-3 makes the map **live, persistent, and self-reconciling** — without touching the query
**authorization gate**. `index::Index::query`'s scope predicate
(`matches ∩ session-grants ∩ domain-ceiling`, built-before-retrieve) is **byte-identical** to slice-2.
Slice-3 only changes *which rows exist* and *when*; never *who may see a row*.

Guarantees (owner-scoped):

- create / modify / delete / rename reflected automatically
- persistence survives `swampd` restart AND reboot
- index stays subordinate to the sealed indexable allow-set
- events outside allowed domains cannot inject data into the DB
- atomic / crash-safe updates
- startup reconciliation catches events missed while `swampd` was down
- deletion really removes searchable content (metadata AND FTS)
- existing query authorization is completely unchanged
- a degraded watcher/DB is **never** served as authoritative-current — freshness is explicit

## 2. Adjudicated forks (owner-decided this slice)

1. **Watcher = inotify, not fanotify.** `swamp.md` §6 said "fanotify", but fanotify create/delete/
   rename-with-names needs `CAP_SYS_ADMIN`, which contradicts swampd's unprivileged-Landlocked spine
   (§5). inotify is unprivileged, works inside the Landlock allow-set, and delivers
   create/modify/delete/moved_from/moved_to + `IN_Q_OVERFLOW`. §6 amended to record inotify as the
   unprivileged watcher and why. Hand-rolled raw syscalls in `linux_uapi.rs` (no `notify` crate —
   minimal-deps).
2. **Persistence = a dedicated `/var/lib/swamp` partition.** The sealed image mounts `/var` as a fresh
   tmpfs each boot (`systemd.volatile=state`, `image/mkosi.conf.d/25-writable-var.conf`) — a documented
   S7 deferral — so a bare path under `/var` would NOT survive reboot. Owner decision (fork re-opened
   when the first VM cycle surfaced this): add a **dedicated persistent partition** (GPT label
   `shrek-swamp`, `image/mkosi.repart/30-swamp-state.conf`) mounted at `/var/lib/swamp` by an explicit
   `var-lib-swamp.mount`, leaving `systemd.volatile=state` and the rest of `/var` untouched. Only the
   swamp store survives reboot; the reboot-persistence guarantee is delivered for real, not weakened.
   The index is reclassified **durable *derived* state, never authority** (a wipe just forces a
   reconcile). §11 amended. `/var/lib/swamp` is separately Landlock-granted internal state and is
   **never** part of the semantic allow-set (the index never indexes itself). The partition is mounted
   `noexec,nosuid,nodev` — a data store must never be an execution home.
3. **Freshness end-to-end** through swampd → swamp-broker → coder. The broker does not reinterpret it;
   coder surfaces it explicitly to the model. STALE means results may still be useful but
   completeness / non-existence claims are unsafe until reconciliation restores FRESH.
4. **Unit correct-but-disabled.** `swampd.service` fixed for durable state (`StateDirectory=swamp`,
   `Restart=on-failure`, `ProtectHome` relaxed only enough not to preempt the allow-set roots — Landlock
   remains the real indexing boundary). NOT enabled by default; proven by explicit start/restart in the
   sealed-VM gate.

## 3. Freshness state machine

`Freshness ∈ { FRESH, STALE }` (in-memory runtime state; not persisted — a fresh start reconciles).

- Boot: STALE until the initial full reconcile completes **and** every allow-set root's watch is armed.
- → FRESH only when BOTH a successful reconcile AND watcher arm have completed.
- → STALE on any of: `IN_Q_OVERFLOW`, `inotify_add_watch` exhaustion/failure (`ENOSPC`), a DB write
  error while applying an event. Returning to FRESH then requires a **fresh successful reconcile +
  watch re-arm** (per fork 1).

Freshness is index-global. It carries no per-session or per-object information, so exposing it is not
an existence oracle.

## 4. Wire protocol — additive, backward-compatible

`swampd` query response gains one header line after `RESULT <n>`, before the hits:

```
RESULT <n>
freshness fresh|stale
hit <path>\t<snippet>
...
END
```

Both existing consumers (`shrek find`, coder `format_swamp_result`) already ignore unrecognized lines
between `RESULT` and `END`, so old binaries keep working; new ones surface freshness. The
`RESULT`/`hit`/`END` **structural core is unchanged**, so slice-2's probe-resistance (a denied query is
indistinguishable from a legitimate zero-hit in that core) is preserved.

Broker: relays swampd's response **verbatim** (freshness passes through untouched — no
reinterpretation). On the broker's own fail-closed paths (deny / swampd unreachable) it emits
`freshness unknown` — the honest state (it did not reach a healthy index) and, being index-global,
not an existence oracle. `unknown` and `stale` alike bar completeness claims.

Coder: `format_swamp_result` reads the freshness header and, when not `fresh`, prefixes the tool
result with an explicit caution so the model does not treat absence as proof of non-existence.

## 5. Reactor & coalescing

Single-thread `poll()` reactor over `[query-listener fd, inotify fd]` — one `rusqlite::Connection`,
no locks, matching the existing synchronous design. Coalescing is via `IN_CLOSE_WRITE` (one enrich per
file-close, not per write syscall); a timerfd debounce for pathological never-closing writers is a
noted follow, not slice-3.

Event application (all re-run the SAME `is_never_indexable` / `PRUNE_DIRS` / `domain_for` userspace
mirror the crawl uses, so an event can never introduce an out-of-domain or never-indexable row):

- `IN_CREATE|IN_ISDIR`, `IN_MOVED_TO|IN_ISDIR`: **arm watch on the new dir, THEN reconcile that
  subtree** (closes the recursive-watch race — children created between mkdir and add_watch are caught
  by the immediate subtree reconcile).
- `IN_CREATE` (file), `IN_CLOSE_WRITE`, `IN_MOVED_TO` (file): upsert metadata + (re)extract FTS.
- `IN_DELETE`, `IN_MOVED_FROM`, `IN_DELETE_SELF`, `IN_MOVE_SELF`: delete the path's record and, if a
  dir, its whole subtree (metadata AND FTS) + drop its watch.
- `IN_Q_OVERFLOW` (wd == -1): mark STALE and schedule a full reconcile.

Rename is handled as delete-old + add-new by path (cookie pairing is an unnecessary optimization; the
resulting DB state is identical).

## 6. Persistence & startup reconciliation

- State dir moves `/run/swamp` (tmpfs) → `/var/lib/swamp`, a **dedicated persistent partition** on the
  sealed image (the rest of `/var` stays a volatile tmpfs); `swampd` no longer deletes the DB on start.
  `swampd.service` `Requires=`/`RequiresMountsFor=` the mount, so it never writes onto the volatile
  fallback. Off-image (dev host) the path is any persistent directory.
- SQLite WAL already gives atomic, crash-safe commits (`PRAGMA journal_mode=WAL`).
- `meta(k,v)` table carries `schema_version`. On open, a missing/mismatched version → wipe + rebuild
  (reconcile from scratch) — invalid state fails toward a clean rebuild, never a corrupt serve.
- Startup **full reconcile** (replaces the one-shot crawl): walk the allow-set, upsert every live
  object (re-extract FTS when size/mtime changed or FTS absent), then **prune** every DB row whose
  path is no longer live or no longer in the allow-set (+ its FTS). This is what catches
  creates/edits/deletes that happened while swampd was down.

## 7. Out of scope (unchanged deferrals)

No embeddings, no graph, no LLM summaries, no content-hash tier. Query auth gate untouched. Living
graph still deferred (§11). timerfd write-debounce deferred (§5 above).

## 8. Ship path

boundary → design-review (this doc) → build → unit → oracle (`swamp-broker-find-proof.sh` extended:
live create/modify/delete/rename reflected; restart-persistence; reboot-sim reconcile catches
offline changes; overflow→STALE; deletion-really-gone; freshness end-to-end) → **sealed-VM re-seal**
(swampd is a `default-member`, on-image) with a **reboot-based** persistence gate (P62-swamp3: seed on
one boot, `systemctl reboot`, verify on the next that the swamp partition survived AND ordinary `/var`
was wiped) → owner-split commit → dual-gh → graph baseline #6.
