# Phase-6 Swamp slice-4 — pluggable embedding-provider abstraction + local semantic tier (threat model + design-of-record)

Status: **BOUNDARY / THREAT-MODEL + DESIGN. No code yet.** Brings the threat pass and the DOR
*before* any implementation, per `swamp.md` §8/§11 and `filesystem-intelligence.md` §8 (embeddings +
an external provider channel are a self-reinforcing asset that earns its own threat pass first).

Predecessor: slice-3 live+persistent (`docs/phase6-swamp-slice3-live-persistent.md`), HEAD `cffcea7`.
Builds on the **frozen** slice-1/-2/-3 query-authorization gate (`crates/swampd/src/index.rs::query`)
and does **not** alter it by one bit.

> The headline deliverable is the **abstraction + graceful degradation**, not any one model. Semantic
> retrieval is *optional*; lexical (FTS) is the *mandatory floor* and is always available. A provider is
> nothing more than `(sealed provider-profile NAME, local wire-adapter)`; authority stays
> `matches ∩ session-grants ∩ domain-ceiling`, and the provider **never widens scope — it only scores.**

---

## 0. The one property everything below is subordinate to

```
SEMANTIC AUTHORITY ≤ DATA AUTHORITY  (architecture.md §5, filesystem-intelligence.md §0)
  Adding embeddings + a similarity index must not let a caller reach — by inference — any byte it
  could not already reach directly. The candidate set for similarity is built from
  matches ∩ session-grants ∩ domain-ceiling BEFORE any vector is scored; unauthorized objects'
  vectors are ABSENT from the computation, never ranked-then-filtered. Provider loss degrades to
  lexical FTS; it never disables Swamp and never widens authority.
```

Three failure asymmetries this slice must preserve, all in the SAFE direction:

| Signal | Degraded meaning | Never |
|--------|------------------|-------|
| `freshness stale` (slice-3) | map may be behind reality; a miss ≠ non-existence | never more authority |
| `semantic unavailable` (this slice) | no vector scoring today; FTS floor serves | never unavailable Swamp |
| stale/pending vector | ranking less clever for that chunk; FTS still finds it | never wrong, never more authority |

---

## 1. Threat model — the provider channel + the vector index earn their own pass

### 1.0 Assets, principals, boundaries

**New assets this slice creates:**
- **A1 — the vector store.** Per-chunk embeddings of *indexed content* on the durable
  `/var/lib/swamp` partition. A vector is a lossy but real derivative of file content; possessing the
  vectors of `~/Vault` would be a content-leak by inference. (It never contains Vault vectors — see T1 —
  but the store is an asset an adversary would want.)
- **A2 — the content-egress channel.** Indexing-time embedding ships *indexed chunk content* off the box
  to an external provider to be vectorized. Object content leaves swampd's address space. This is the
  first time any swampd-adjacent component sends file content outbound.

**Principals / trust:**
- `swampd` — the most dangerous process in the system (`swamp.md` intro): reads everything on its
  allow-set, unprivileged, Landlocked, availability-plane, **currently makes zero outbound network
  connections** (`confine.rs` `handled_access_net: 0`; query socket is unix-domain).
- The **embedding provider** (a LAN host) — **untrusted for authority.** It scores; it never widens
  scope. It is reached over a **gated egress plane**, never trusted to hold or return authority.
- A **caller** (coder T2 session, `shrek find`) — reaches the query API only through the broker with a
  kernel-attested session; never supplies its own identity or scope.

**Trust boundaries crossed by new code:**
```
  (indexed file content) → chunk → [BOUNDARY: content leaves the box] → provider → vector → /var/lib/swamp
  caller query → broker → swampd → [BOUNDARY: authority gate] → scoped candidate set → similarity → hits
```

### 1.1 (a) Authority-before-vector — THE CRUX (preserve the query-gate structure exactly)

**Threat T1 — global ANN leaks across authority.** A naive design builds one global vector index
(HNSW/IVF over *all* chunk vectors), runs k-NN for the query vector, then filters the top-k by the
caller's grants. That is *retrieve-then-filter* — the exact leak `index.rs` was written to avoid
(its module header: "an object outside the caller's authority is never a candidate row"). A ranking
bug, a k too small, or a similarity tie leaks the existence — or a snippet — of an out-of-scope object.

**Mitigation (load-bearing, non-negotiable):** the candidate set is **constructed, not filtered.** The
same scope predicate `index.rs::query` already compiles — `(o.path = grant OR substr-prefix under
grant) AND o.domain IN (verb-domains)` — selects the candidate chunk-vectors *before* any similarity is
computed. Similarity is evaluated **only over vectors whose object row satisfies the scope predicate.**
Unauthorized vectors are never loaded into the ranking. At v1 scale (a personal box) this is a
**scope-first brute-force cosine scan** over the joined candidate set — correct, simple, and leak-proof
by construction. **No global ANN at v1** (Fork F2). If an ANN index is ever added for scale, it must be
*scope-partitioned* — never a global index filtered after.

**Threat T2 — score/count side channels around the wall.** `index.rs` deliberately exposes **no raw
bm25 score and no global match count**, because FTS5 bm25 mixes corpus-wide document-frequency stats
that span out-of-scope docs. A per-hit similarity score or a "N candidates considered" count would be
the same thin side channel for the vector tier.

**Mitigation:** the semantic projection is **byte-identical in shape to the FTS projection** — in-scope
`path` + in-scope `snippet` only. **No similarity score, no candidate/global count crosses the wire.**
(Cosine similarity between the query and one in-scope chunk is self-contained — it carries no
corpus-wide IDF like bm25 — but we still refuse to emit it, to keep the projection discipline of
`index.rs` exact and to leave no scalar an attacker could difference across queries.) The `RESULT n /
freshness / semantic / hit / END` structural core is unchanged, so slice-2 probe-resistance (a denied
query is indistinguishable from a legitimate zero-hit) is preserved.

**Threat T3 — fail-open on the semantic path.** A bug that makes the empty-scope case *skip* the scope
predicate for the vector query (e.g. "no FTS candidates, fall through to global similarity") is a
catastrophic leak.

**Mitigation:** `grants.is_empty() || verb_domains.is_empty() || terms.is_empty() ⇒ empty result`
is enforced **before** the vector path exactly as `index.rs::query` does today (`index.rs:238`). The
semantic query is a *second query kind that reuses the same guard and the same scope-building code*, not
a parallel path with its own guard. Fail-closed = empty projection, unchanged. Unit + oracle both assert
the Vault-token-by-vector case (§ proof) the way slice-1 asserts it by FTS (`index.rs:367`).

### 1.2 (b) Provider channel + content egress

**Threat T4 — swampd becomes an exfiltration deputy.** If swampd dials the provider *directly*, the
most-dangerous, reads-everything-allowed process gains an **outbound network capability** and an HTTP
client in the sealed base. A swampd compromise could then ship any byte it can read to any reachable
destination — reopening exactly the side channel §5 confinement exists to close.

**Mitigation (Fork F1):** swampd **never dials the network.** It speaks a tiny plaintext framing to a
**local unix socket** (`/run/swamp-embed.sock`) named by a **sealed provider-profile**. An **off-image,
broker-side** `swamp-embed-proxy` (new crate, excluded from workspace `default-members` — mirrors
`crates/model-proxy`) is the only party that reaches the LAN provider, and only the sealed
provider-profile destination (`<embedding-host>:8102`), pinned by the same **deny-by-default egress plane** as
slice-1b (`docs/phase6-slice1b-egress.md`: named-egress, fail-closed A-record pin, drop-everything-else).
swampd's `handled_access_net` stays `0`. The base links **no HTTP client, no TLS, no DNS egress.**

**Threat T5 — content egress exceeds indexable authority.** The embedding request carries chunk
*content* out of the box. It must never carry content from a tree swampd may not index, and never to a
destination other than the sealed provider.

**Mitigation:** embedding is driven **only** from the same enrichment pipeline that already gates on
`is_never_indexable` / `PRUNE_DIRS` / `domain_for` / the 512 KiB `MAX_FTS_BYTES` cap
(`crawl.rs:29,91,104,125`) — content that never enters the FTS floor never enters the embedder. Egress
is bounded to the one sealed provider destination; no secret is needed (LAN, no auth), and **no secret
lives in the base** regardless. The proxy is the natural chokepoint for a future content-egress
tripwire/DLP (deferred, noted).

**Threat T6 — a hostile/spoofed provider returns poisoned vectors, or widens scope.** A compromised
provider could return vectors crafted to rank an attacker-chosen chunk first, or attempt to return
anything other than a fixed-width numeric vector.

**Mitigation:** the provider is **untrusted for authority by construction** — it only produces a score
input for objects *already in the caller's authorized candidate set*. A poisoned vector can at worst
**reorder in-scope results** (a ranking-quality issue, never an authority escalation — semantic
authority ≤ data authority holds regardless of vector values). The proxy validates the reply shape
(fixed `dim`, finite floats) and fails **closed to lexical** on any malformed/oversized/erroring reply
(T7). The provider can never inject a *path* or *object* into results — it only ever returns numbers
keyed to chunks swampd already authorized and sent.

**Threat T7 — provider loss disables Swamp.** If a missing/unreachable/500ing provider made a query
error out, an adversary who can DoS `:8102` could deny search entirely — availability regression.

**Mitigation (§ failure behavior):** missing / unconfigured / unreachable / erroring / timing-out
provider ⇒ **semantic & hybrid retrieval fall back to lexical FTS**, `semantic unavailable` is
reported, and the query still succeeds. Same asymmetry as slice-3 freshness: less clever, never wrong,
never unavailable, never more authority. `swampd` stays on the availability plane (`swamp.md` §10).

### 1.3 (c) Sealed-base surface — quantify what the base newly links

**Threat T8 — the semantic tier drags weights / inference / a heavy dependency tree into the sealed,
dm-verity base.** Model weights or an inference runtime in the base bloat the image, expand the audited
attack surface, and violate "light by default."

**Mitigation (Fork F3, and the answer to (c)):** the sealed base ships **only the backend interface +
the FTS floor + the availability signal + the vector-store tables.** Quantified, the base links
**nothing new**:
- **No model weights, no inference deps** in the base. Any local embedding runtime is the *provider*,
  which is off-image (the LAN service) — never in the sealed image (`swamp.md` §8 "confinement is not a
  tier", but the *runtime* is an optional off-image extension).
- **No HTTP client / no TLS / no ANN library / no new serde:** the swampd↔proxy protocol is a compact
  **length-prefixed binary framing** (request: chunk texts; reply: `dim`×f32 blobs) over a std unix
  socket — no JSON dep in-base. Cosine similarity is **pure `std` f32 arithmetic**. Vectors are stored
  as **BLOBs in the already-bundled `rusqlite`** (the `fts5` build) — no separate vector engine.
- New in-base surface = a `mod embed` (the `EmbeddingBackend` trait + a `SocketBackend` +
  a `NullBackend`), two new SQLite tables, one similarity function, one wire header. Zero new crates.

The **provider runtime/model is off-image / optional-extension**; the **backend interface + FTS floor +
availability signal are in-base** (swampd is a `default-member` → on-image → VM re-seal). Hold that
split firmly.

### 1.4 Residual / out-of-threat-scope this slice
- Content-egress DLP/tripwire at the proxy (chokepoint identified; not built — mirrors model-proxy §6).
- ANN/scale index (deferred; if built, scope-partitioned, never global-filter-after).
- Rename-surviving opaque object id (still deferred from slice-3; chunks key to the stable `objects.id`
  row, which upsert preserves across content edits — sufficient for v1).
- Timing side channels from similarity-scan duration scaling with candidate-set size (a coarse
  size oracle already implicit in FTS scan time; not newly introduced; noted).

---

## 2. What slice-4 changes — and the one thing it must not

Slice-4 adds an **opt-in, per-domain-enablable SWAMP SEMANTIC tier** layered on the mandatory core,
**without touching the query authorization gate.** `index.rs::query`'s scope predicate
(`matches ∩ session-grants ∩ domain-ceiling`, built-before-retrieve) is preserved exactly; the semantic
query is a *second query kind that reuses the same guard and the same scope construction.* Slice-4
changes *which ranking is available* and *what capability signal rides with the result*; never *who may
see a row*.

Guarantees (owner-scoped):
- pluggable embedding-provider abstraction (versioned: `provider_id + model_id + dim + version`) ships
- FTS is the mandatory floor — with no provider, Swamp still serves lexical/metadata search and reports
  `semantic unavailable`
- semantic/hybrid retrieval is strictly additive; provider loss degrades to lexical, never disables
- deterministic chunking: stable, reproducible boundaries + stable chunk IDs tied to the object record;
  re-embedding is idempotent and incremental
- persistent vectors on `/var/lib/swamp`; crash-safe (WAL, same as slice-3)
- rebuild-on-provider/model/schema-change via a `semantic_version` bump (wipe degrades ranking, never
  correctness)
- freshness AND semantic-availability propagate end-to-end (swampd → broker verbatim → coder/`shrek find`)
- **existing query authorization is completely unchanged**
- no model weights and no inference deps enter the sealed base

## 3. Forks — ADJUDICATED (owner-decided 2026-08-21, build-go)

Owner adjudication: **boundary approved — proceed to build.** F3 = **SQLite BLOB tables** (amends
`swamp.md` §4). F4 = **distinct `semantic available|unavailable` header**. F1/F2/F5/F6/F7 stand at the
recommendations below.

| # | Fork | Options | Recommended | Amends |
|---|------|---------|-------------|--------|
| **F1** | Provider channel topology | (a) swampd dials `<embedding-host>:8102` directly; (b) swampd → local unix socket → **off-image `swamp-embed-proxy`** → provider over the gated egress plane | **(b)** — base stays network-free & HTTP-dep-free; mirrors `model-proxy`; T4 mitigation | — |
| **F2** | Similarity search structure | (a) global ANN (HNSW/IVF) filtered-after; (b) **scope-first brute-force cosine** over the candidate set | **(b)** — authority-before-vector by construction (T1); ANN deferred, scope-partitioned if ever | — |
| **F3** | Vector store | (a) separate vector DB (Chroma/FAISS — new heavy dep + separate store); (b) **extend the `/var/lib/swamp` SQLite** with `chunks`/`vectors` BLOB tables | **(b)** — zero new crates; light-by-default (empty tables on a core install) | **`swamp.md` §4** ("separate vector store" → separate *logical tables* in the same SQLite; a metadata/FTS install carries none of their weight) |
| **F4** | Availability signal | (a) overload `freshness`; (b) **distinct additive `semantic available\|unavailable` header** | **(b)** — orthogonal to freshness (a FRESH index can be semantic-unavailable); both index-global, no existence oracle | wire proto §4 |
| **F5** | Chunk identity | (a) content-hash id; (b) **`(object_id, ordinal)` + stored `text_hash`** for idempotent skip | **(b)** — stable across edits; ties to the object record; incremental re-embed | — |
| **F6** | Hybrid fusion | (a) equal-weight RRF; (b) **semantic-led, lexical as exact-match booster** | **(b)** — prior art: a retrieval bakeoff proved equal-weight RRF drags semantic down (hybrid 62% < semantic 90%) | — |
| **F7** | First backend | (a) stand up a new endpoint; (b) **consume an already-live `<embedding-host>:8102` EmbeddingGemma-300M** | **(b)** — verified live this session: OpenAI-compatible `/v1/embeddings`, **dim=768**, permanent `embed-gemma.service`, reboot-durable, up in both GPU modes | — |

## 4. The backend interface (in-base, versioned)

```
BackendIdentity { provider_id: &str, model_id: &str, dim: u32, version: u32 }

trait EmbeddingBackend {
    fn identity(&self) -> BackendIdentity;         // → the semantic_version key
    fn embed(&self, chunks: &[&str]) -> Result<Vec<Vec<f32>>>;  // fixed dim, order-preserving
}

SocketBackend  — frames chunks to /run/swamp-embed.sock (the sealed provider-profile), parses dim×f32.
                 Any connect/frame/timeout/shape error ⇒ Err ⇒ caller degrades to FTS (T7).
NullBackend    — no provider configured ⇒ identity absent ⇒ semantic=unavailable, FTS floor only.
```

The **first real backend** is `provider_id=local-lan`, `model_id=embeddinggemma-300m`, `dim=768`,
reached through `swamp-embed-proxy` (off-image) over the gated egress plane. The interface is *two
concrete implementations forcing the seam*, **not a plugin framework** (same discipline as the coder's
provider seam, `phase6-slice3-provider-abstraction.md` §6).

## 5. Schema — additive tables on `/var/lib/swamp` (Fork F3)

`index.rs` `SCHEMA_VERSION` bumps `3 → 4`; the reconcile already wipes+rebuilds on mismatch, which
*creates the new tables and re-derives* — invalid/older durable state fails toward a clean rebuild
(`index.rs:104`). New tables (empty on a core/FTS-only install — light by default):

```
chunks(
  id         INTEGER PRIMARY KEY,
  object_id  INTEGER NOT NULL REFERENCES objects(id) ON DELETE CASCADE,
  ordinal    INTEGER NOT NULL,          -- deterministic order within the object
  byte_start INTEGER, byte_end INTEGER, -- reproducible boundaries
  text_hash  TEXT NOT NULL,             -- idempotent-skip / incremental re-embed (F5)
  UNIQUE(object_id, ordinal)
)
vectors(
  chunk_id         INTEGER PRIMARY KEY REFERENCES chunks(id) ON DELETE CASCADE,
  vec              BLOB NOT NULL,        -- dim×f32, little-endian
  semantic_version TEXT NOT NULL         -- provider_id|model_id|dim|schema
)
meta: 'semantic_version' = provider_id|model_id|dim|schema  → on mismatch, wipe chunks+vectors & re-embed
```

- **Deletion cascades** — an object delete/subtree-delete/prune (`index.rs` `delete_path` /
  `delete_subtree` / `prune_absent`) drops its chunks AND vectors, so "deletion really removes content"
  extends to the semantic tier (slice-3 guarantee, preserved).
- **Deterministic chunking:** fixed char/token window (target ≤ ~1024 tokens to stay safely under
  EmbeddingGemma's 2048-token cap — a retrieval bakeoff saw ~11% of *dense* docs exceed 2048 at 1.28
  tok/char) with fixed overlap; boundaries are a pure function of the object's bytes, so re-chunking is
  reproducible and chunk IDs are stable.
- The scoped semantic query JOINs `vectors → chunks → objects` under the **same scope+domain predicate**
  `index.rs::query` builds today; cosine is computed in-Rust over the returned candidate blobs.
- `/var/lib/swamp` remains internal state, `noexec,nosuid,nodev`, **never** part of the semantic
  allow-set (the index never indexes itself) — unchanged from slice-3.

## 6. Wire protocol — additive, backward-compatible (Fork F4)

One new header line after `freshness`, before the hits:

```
RESULT <n>
freshness fresh|stale
semantic available|unavailable        ← NEW (this slice)
hit <path>\t<snippet>
...
END
```

- Both consumers already ignore unrecognized lines between `RESULT` and `END`
  (`phase6-swamp-slice3-live-persistent.md` §4), so old binaries keep working; new ones surface it.
- **Broker** relays swampd's response **verbatim** (no reinterpretation, exactly as freshness); on its
  own fail-closed paths (`EMPTY_RESULT`) it emits `semantic unavailable` alongside `freshness unknown`.
- **Coder / `shrek find`** surface `semantic unavailable` the way they surface `freshness stale` — the
  model is told semantic ranking was not applied, so it must not read absence-of-semantic-hit as
  semantic non-existence.
- `semantic` is **index-global** (a capability signal): it discloses nothing about which sessions or
  objects exist — no existence oracle, so it does not weaken §1's projection guarantees.

## 7. Event/enrichment integration (reuses slice-3 plumbing, adds one lazy stage)

The embedder is a **tier-gated enrichment stage after FTS extraction**, hooking exactly where
`crawl.rs:130` writes `index.set_fts(id, &text)`:

```
mapper (always) → extract_text (FTS tier) → [semantic tier, if enabled for this domain]:
                    chunk(text) → text_hash unchanged? skip : embed(chunks) via SocketBackend → store vectors
```

- **Availability, not correctness, gates it.** If the backend is `NullBackend` or `embed()` errs, the
  object is still fully FTS-indexed; only its vectors are absent → `semantic unavailable` (global) /
  pending (per-chunk). A stale/pending vector degrades ranking, never correctness (`swamp.md` §10).
- **Live watcher** (`watch.rs:211` `index_object`) re-runs the same stage on `IN_CLOSE_WRITE`, so an fs
  edit → re-chunk → re-embed automatically; `text_hash` skips unchanged chunks (incremental).
- **Per-domain enablement** (`swamp.md` §8): the semantic tier is enabled per indexable domain in
  `shrek-policy::swamp` (e.g. semantic over `projects`, FTS-only over `documents`) — a supported config,
  not a hack. Enablement narrows/adds *enrichment*; it never touches the wall.
- **Single-thread reactor unchanged** (`server.rs:48`): embedding is a synchronous call to the local
  proxy socket inside the same poll loop; a slow provider is bounded by a timeout and degrades to FTS
  rather than stalling queries. (A worker-thread offload for embedding latency is a noted follow, not
  this slice — v1 keeps the one-connection, no-locks design.)

## 8. Failure behavior — availability plane, fails to lexical

```
no provider configured / proxy down / provider 5xx / timeout / malformed reply
  → semantic & hybrid retrieval fall back to lexical FTS
  → query STILL SUCCEEDS; response carries `semantic unavailable`
  → boot, login, VFS, FTS search, metadata search: ALL STILL WORK

This plane fails toward LESS capability, never MORE authority (swamp.md §10). A dead or hostile
provider makes ranking dumber; it can never surface an object the caller lacks authority for, and can
never make Swamp unavailable.
```

## 9. Out of scope (explicit deferrals)

- **No cloud embedding provider** — LAN/gated only.
- **No model weights / no inference deps in the sealed base** — local runtimes are optional off-image
  extensions; the base links zero new crates (§1.3).
- **No reranker.**
- **No relationship / living graph** (SWAMP-5) — this slice is retrieval, not relationships.
- **No LLM summaries.**
- **No global ANN / scale index** — scope-first brute-force at v1; ANN scope-partitioned if ever (F2).
- **No content-egress DLP tripwire** — chokepoint identified at the proxy; deferred.
- **No per-domain model selection** (`filesystem-intelligence.md` §8) — one backend at v1.

## 10. Ship path (unchanged method) — STATUS

boundary + this threat-model/DOR ✅ → **owner design-review + fork adjudication (F1–F7)** ✅ (2026-08-21,
build-go) → build ✅ → unit ✅ (index crux + embed + crawl-enrichment + policy + broker + coder; whole
workspace green) → **oracle ✅** (`swamp-broker-find-proof.sh` extended, section E — **46/46 gates, 0
fail**: E1 `semantic available`; E2 FTS floor preserved with semantic ON; E3 authority-before-vector
(out-of-scope token/project ABSENT with semantic ON); E4 NO score/count on the wire; E5 base wires the
provider channel network-free; E6 provider-down → `semantic unavailable` + FTS floor still serves; E7
broker fail-closed → `semantic unavailable`; all slice-1/2/3 gates still green) → **sealed-VM re-seal**
◻ NEXT — **gate PREPPED** (owner-run, needs `mkosi`): the P6-swamp4 section is written into
`image/overlay/usr/lib/shrek/mount-plane-gate` (asserts the sealed swampd carries the semantic surface +
provider-channel wiring; the sealed coder carries the `semantic=` note; the off-image proxy is ABSENT
from the base; and the behavioural fail-closed — a no-provider sealed swampd `reconcile` builds the FTS
floor AND reports `semantic=unavailable`), and the slice-4 DOR + `swampd.service` `Documentation=` are
placed in the overlay. swampd is a `default-member` (on-image); the backend interface + FTS floor +
`semantic` header + chunks/vectors tables are in-base; the proxy + provider are off-image and never in
the VM. The re-seal must rebuild the default-member swampd AND re-run `seal-t2-artifacts` (the coder
note), else the gate flags a stale seal. → owner-split commit ◻ → dual-gh ◻ → graph baseline #7 ◻.

Also proven this build: a **LIVE round-trip** of the real `swamp-embed-proxy` against the production
the `<embedding-host>:8102` EmbeddingGemma-300M returned 2×768-dim vectors through swampd's exact binary framing
(non-gating smoke, confirms the first real backend end-to-end).

The in-base / off-image split, restated (hold it firmly):
```
IN-BASE   (default-member → on-image → VM re-seal):  EmbeddingBackend trait, SocketBackend/NullBackend,
          chunks+vectors tables, cosine, `semantic` header, FTS floor. Zero new crates.
OFF-IMAGE (optional extension, excluded from default-members): swamp-embed-proxy (HTTP client to the
          gated provider), the LAN EmbeddingGemma runtime. Never in the sealed image.
```
