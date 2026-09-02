# The Shrek Memory API — the stable boundary to the on-box brain (ADR-006 refinement)

Status: DESIGN + BUILT (M1 implementation). Owner decision 2026-09-02.
Parent: docs/adr-006-optional-ai-layer.md §3/§4. Implemented by
`layers/shrek-ai/overlay/usr/lib/shrek/ai/shrek-ai-mycelium`.

## 1. Why a named API (not "just run mycelium")

The real mycelium needs the FastMCP/pydantic/uvicorn pip closure and hardcodes a `0.0.0.0` bind — both
wrong for a sealed, offline appliance (§7 requires loopback-only; the image prizes a small fixed service
set with no pip closure). So M1 ships a **minimal, stdlib-only implementation** of a **named wire
contract** — the Shrek Memory API — rather than the full engine.

The contract is the load-bearing thing. Because Donkey and the `shrek ai` front door speak *only* this API
over loopback (an **out-of-process** boundary), the implementation behind it is swappable: the full
mycelium (connections, semantic recall, decay) can **replace or augment** the M1 service later **without
changing Donkey or the front door**. M1 buys offline + light + auditable; the boundary buys the upgrade
path.

## 2. Transport

- HTTP/JSON over **`127.0.0.1` only** (§7 — every AI-layer listener binds loopback; egress stays zero).
  The M1 service refuses any non-loopback `--host`.
- Default port `8199` (loopback). No auth: ADR-003's veth `input`-drop keeps benches off the localhost
  port, and nothing else on the box has a route to it — the same reasoning that makes the localhost model
  server acceptable (§7).
- `api` field on every response carries the contract version (`shrek-memory/1`).

## 3. Endpoints (v1)

| Method + path | Request | Response |
|---------------|---------|----------|
| `GET /healthz` | — | `{ok, api, seed_version, counts:{total, seed, runtime}}` |
| `POST /recall` | `{query, limit?=5, project?, mtype?}` | `{api, query, memories:[{id, content, mtype, project, source_type, score}]}` |
| `POST /save` | `{content, mtype?, project?, source_type?, pinned?, confidence?}` | `{api, id}` |

- **`/recall`** — FTS5 (`bm25`) + a light re-rank (token coverage + recency + access + pin/confidence),
  LIKE fallback. Returns the union of the runtime brain and the seed namespace, ranked together.
- **`/save`** — writes to the runtime (dev-owned) namespace. `content` ≤ 8000 chars.
  `source_type="seed"` is **rejected (403)** — the seed namespace is owned by the OS seed pipeline, not
  writable through the API.

## 4. The two namespaces on one brain DB

The real mycelium is a single writable store and cannot "mount" a read-only seed set; the M1 service is
too. So §4's split is realized by **`source_type`**, in one SQLite DB (`~/.mycolink/brain/brain.db`,
dev-owned):

- **`source_type='seed'`** — the OS self-knowledge, **ingested** from the root-owned read-only seed set
  (`/home/.shrek/ai/seed`) and **replaced wholesale** when the seed manifest changes (§5b), preserving all
  runtime rows. Dev cannot edit the authoritative source; the ingested copy is refreshed from it.
- **everything else** — the runtime brain the assistant writes as it learns (the never-clobber namespace).

Seed replacement keys on the delivered `SEED-MANIFEST` (version + content sha): unchanged → no-op (safe
every start); changed → `DELETE WHERE source_type='seed'` then re-ingest. The schema is a
**mycelium-compatible subset** (`memories` + `memories_fts`), so a future real-mycelium swap reads the
same shape.

## 5. What M1 deliberately omits (the full engine can add behind this API)

Connection propagation, semantic/vector recall, decay/consolidation, agent-access tracking, the richer
mtype taxonomy behaviors. None are M1-critical for a seed brain whose job is FTS recall over ~100
read-only records + a small runtime brain. Adding them is an implementation change behind an unchanged
`/recall` + `/save`.
