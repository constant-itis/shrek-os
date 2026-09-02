# ADR-006 M1 Slice 3 — the deterministic seed brain

Status: BUILT. The self-knowledge seed is generated + delivered; the on-box mycelium that *serves* it
is a later slice, so like slices 1–2 the delivery is wired and dormant-tolerant until that runtime lands.
Parent: docs/adr-006-optional-ai-layer.md §5 (seed brain), §8 (first-boot trigger). Predecessor:
model ratification 014c2d3.

## 1. Scope

Make the assistant self-aware: generate a seed brain describing *this* system, deterministically and with
**no model in the build loop** (§5a), and deliver it to /home with **seed-version replace-on-update** so a
later OS update refreshes the self-knowledge without clobbering the operator's own memories (§5b). This is
what makes `/recall "how does Bench work"` return real answers.

**Out of scope** (later slices): the on-box mycelium runtime that indexes/serves the seed set read-only
alongside the dev brain; the `shrek ai` front-door that loads the system prompt (slice 5); the dogfood
`AI` stage that asserts recall against the seeded brain (slice 6).

## 2. What shipped

**Build inputs (`layers/shrek-ai/seed-src/`) — the reproducible source:**
- `sources.list` — the pinned, ordered self-knowledge doc set (11 in-tree docs; excludes the
  parallel-session `adr-004`). Explicit list, not a glob, so the seed is reproducible.
- `conventions.md` — the authored memory-use doctrine (§5 (ii)): recall-before-assert, propose-don't-
  execute, offline-first, save-what's-durable, checkpoint. One H2 = one behavior memory.
- `SEED_VERSION` — the seed namespace version (`1`). Bump on any doc/convention change + regenerate.

**Generator (`scripts/gen-ai-seed.py`) — stdlib, deterministic:**
- Chunks the pinned docs + conventions by H2 section into seed memory records
  (`{id: sha256(content)[:16], content, mtype, source, seed_version, tags}`); infra → `reference`,
  conventions → `lesson`; content-hash dedup.
- Emits `usr/share/shrek/ai/seed/memories.jsonl` (109 records, v1) + `SEED-MANIFEST`
  (`SEED_VERSION` + `RECORDS` + `MEMORIES_SHA256`, the `# VERIFY` hash log).
- **Determinism is load-bearing** and enforced: explicit ordered inputs, version-from-file (never a
  timestamp), content-hash ids, `sort_keys`, LF endings. `--check` mode fails if the committed seed is
  stale (a pre-commit/CI guard). Proven: two runs → identical sha256.

**Shipped seed (`layers/shrek-ai/overlay/usr/share/shrek/ai/`):**
- `seed/memories.jsonl` + `seed/SEED-MANIFEST` — the generated, committed seed set.
- `system-prompt.md` — the authored default identity/posture (§5 (iii)): "Donkey", direct, propose-only
  (no host-exec), untrusted-proposer authority model, recall-first. Read from sealed /usr; an operator
  override at `/home/dev/.mycolink/system-prompt.md` takes precedence (never-clobber).

**Delivery (base-baked, Condition-gated):**
- `usr/lib/shrek/ai/shrek-ai-seed` — the applier: delivers the baked seed to `/home/.shrek/ai/seed`
  (root:root 0644, dev reads/never rewrites — §4), replacing the seed set when the baked manifest differs
  (version bump *or* same-version content change), atomic temp-then-move, **never touching the dev-owned
  `~/.mycolink` brain**. Same-manifest boots are a no-op (safe every boot).
- `image/overlay/.../shrek-ai-seed.service` — the base-baked oneshot (§8 option (1): a base-carried
  post-merge applier — the unit is in the boot transaction, the Onion-shipped logic runs when merged).
  `After=shrek-ai-store.service`. Triple Condition (marker + applier + baked seed) → clean skip when absent.
- `shrek-ai-mycelium.service` (slice 1) now also orders `After=shrek-ai-seed.service` — it mounts the
  system-seed set read-only, so the seed must be delivered first.

## 3. The two namespaces (why replace-on-update is safe)

The mycelium brain the assistant queries is the union of two stores on /home, kept apart on purpose (§4):

| Store | Path | Writable? | On seed update |
|-------|------|-----------|----------------|
| System-seed self-knowledge | `/home/.shrek/ai/seed` | root-owned, **read-only to dev** | **replaced wholesale** (retracts stale design descriptions) |
| Runtime brain + chat | `/home/dev/.mycolink/{brain,sessions}` | dev-owned | **never clobbered** |

Because the seed set is root-owned and dev-unwritable, replacing it wholesale is safe — there are no user
edits to lose. Never-clobber applies only to the operator/runtime namespace, which lives at a different
path the applier never touches. This is the honest reading of "single source of truth, never drifts": the
claim holds **for the seed namespace only** (§5a), and drift is prevented by manifest-diff replacement
rather than a seed-once stamp.

## 4. Verification (this slice)

- **Determinism** — two generator runs produce identical `memories.jsonl` sha256; `--check` guard green.
- **Generator content** — 109 records; 14 carry Bench self-knowledge; conventions present as `lesson`
  memories; source-attributed (`docs/…#section`).
- **Applier** — fixtures prove deliver (empty dest) / no-op (same manifest) / replace (v1→v2), mode 0644,
  and a sibling dev-brain path left intact.
- **Units** — `systemd-analyze verify` clean; triple-Condition dormancy on a non-AI box.
- **Deferred to the dogfood `AI` stage (slice 6)** — the on-box mycelium actually serving the seed and
  `/recall` returning real self-knowledge; that needs the mycelium runtime (a later slice).

## 5. Next (slices 4→6)

The `shrek ai` front-door + on-box mycelium runtime (loads the system prompt, serves the seed set +
dev brain, issues the on-demand model start, no host-exec surface, agent-harness digest-pin) → dogfood
`AI` stage (model answers; seed recall returns real self-knowledge; brain persists across reboot; all
listeners 127.0.0.1 + egress zero; seeded injection produces no host effect).
