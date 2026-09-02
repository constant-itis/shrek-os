# ADR-006 M1 Slice 4 — the on-box mycelium runtime (Shrek Memory API)

Status: BUILT + locally proven end-to-end. This is the slice that makes the slice-1
`shrek-ai-mycelium.service` LIVE and validates the slice-3 seed brain for real.
Parent: docs/adr-006-optional-ai-layer.md §3/§4, docs/adr-006-shrek-memory-api.md.
Owner decision (2026-09-02): minimal stdlib service, out-of-process boundary, no pip closure.

## 1. Scope

Build the on-box memory service the slices 1–3 units wait on: a **stdlib-only implementation of the Shrek
Memory API** (recall + save + seed replace-on-update) bound to `127.0.0.1`, serving the dev-owned runtime
brain + the ingested seed namespace. This closes slice-3's deferred proof — `/recall` returning real
self-knowledge — with an actual running service.

**Out of scope** (slice 5): the `shrek ai` shell front door + the inference-server launcher
(`shrek-ai-model`) + the Shrek Tool Contract verb surface + the agent-harness digest-pin.

## 2. Decision — minimal stdlib service behind a named API

The real mycelium needs the FastMCP/pydantic/uvicorn pip closure and hardcodes `0.0.0.0`. Rather than
vendor that closure into a sealed offline image, M1 ships a purpose-built stdlib service implementing a
**named wire contract** (docs/adr-006-shrek-memory-api.md). Out-of-process on loopback, so the full
mycelium can replace/augment it later without changing Donkey or the front door. See the memory-API doc
for the contract; this slice is the M1 implementation of it.

## 3. What shipped

- `layers/shrek-ai/overlay/usr/lib/shrek/ai/shrek-ai-mycelium` — the service (python3 stdlib: `sqlite3`
  FTS5 + `http.server`). `GET /healthz`, `POST /recall` (FTS5 bm25 + light re-rank, LIKE fallback),
  `POST /save` (runtime namespace; `source_type=seed` refused). Refuses any non-loopback `--host` (§7).
  Ingests the read-only seed set as `source_type='seed'` with manifest-keyed replace-on-update; the
  runtime namespace is never clobbered. Schema is a mycelium-compatible subset.
- **`layers/shrek-ai/mkosi.conf`** — now `Packages= python3` (the first package in the AI Onion). The
  sealed base is deliberately python-free; only `INCLUDE_AI` boxes get python3, via this layer. No pip
  closure.
- **`scripts/build-ai-layer.sh`** — flipped from marker-only to the `--base-tree <sealed base> --overlay`
  delta build (the shrek-dev idiom), since the Onion now installs a package.
- The slice-1 `shrek-ai-mycelium.service` ExecStart (`… --host 127.0.0.1`) already matches; its defaults
  resolve the brain to `~/.mycolink/brain/brain.db` and the seed to `/home/.shrek/ai/seed`.

## 4. Verification (this slice)

Live end-to-end against the **actual 109-record seed** (host python3 3.10, FTS5 present):
- Startup ingests 109 seed records; `/healthz` → `seed_version=1, seed=109, runtime=0`.
- **`/recall "how does Bench work"` returns real Bench self-knowledge** (ADR-002/ADR-003 sections) —
  slice-3's deferred proof, now closed with a running service.
- `/save` a runtime memory (id 110) → `/recall "dark mode preference"` returns it, ranked above seed.
- `source_type=seed` write → **403** (seed namespace is OS-owned).
- Restart → seed reports **`current`** (manifest-keyed replace-on-update is an idempotent no-op).

**VM-gated (deferred, as for slices 1–3):** the actual sysext build (python3 in the Onion via the
base-tree delta) and boot — proven-by-mirroring the shrek-dev package-carrying build, not yet
container-built here. The service logic itself is proven above on stdlib python3.

## 5. Next (slice 5)

The `shrek ai` front door: the mycolink-shell (agent-harness, digest-pinned) with **no host-exec
surface**, loading the system prompt, speaking the Shrek Tool Contract, issuing the on-demand model
start, and talking to this Memory API for `/recall`. Then the dogfood `AI` stage (slice 6) boots the whole
layer in a VM and asserts the §9 invariants (127.0.0.1 + egress zero; seed recall; brain persists;
injection produces no host effect).
