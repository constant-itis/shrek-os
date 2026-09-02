# ADR-006 M1 Slice 2 — the /home state model: split store + model-as-data GGUF

Status: BUILT (mechanism). Fuses checkpoint items 2+3 (model-as-data GGUF + split store) — they are one
coherent thing: the mutable AI state on /home. No inference runtime yet (llama.cpp package + real launcher
land with the substrate); this slice delivers the *state model* and the *verify mechanism*, dormant until
a model is delivered.
Parent: docs/adr-006-optional-ai-layer.md (§3 model-as-data, §4 split store). Predecessor: slice 1 (8dc5102).

## 1. Scope

/usr is a sealed read-only dm-verity closure, so all mutable AI state lives on the persistent shrek-data
/home and is (re)delivered every boot (the ADR-005 discipline). This slice stands up that state model and
the model-as-data authentication path:

- **Split store (§4)** — the two-halves permission model that survives the shell running as dev/uid-1000.
- **Model-as-data (§3)** — a per-box GGUF delivered to /home and authenticated against a sealed baked
  digest, so multi-GB model updates decouple from Onion signing ("only the runtime + digest ship sealed").
- **The inference-server unit** — lazy-start + idle-unload, 127.0.0.1-only, dormant until a verified GGUF
  is present.

**Out of scope** (later slices): the deterministic three-body seed that fills the system-seed surface
(slice 4); the llama.cpp-class inference binary + the real `shrek-ai-model` launcher (substrate); the
`shrek ai` front-door that issues the on-demand start (slice 5); the dogfood `AI` stage (slice 6).

## 2. What shipped

**Onion (`layers/shrek-ai/overlay/`) — runtime logic + the sealed digest:**
- `usr/lib/shrek/ai/shrek-ai-store` — materializes the split store on /home (mkdir + ownership/mode,
  idempotent, the `hosts-seed`/`shrek-bench-pool` idiom).
- `usr/lib/shrek/ai/shrek-ai-model-verify` — authenticates the delivered GGUF against the baked digest
  (sha256, the bench `.digest` sidecar discipline). Exit codes: `0` READY, `1` no model configured,
  `2` GGUF not delivered, `3` digest mismatch.
- `usr/share/shrek/ai/model/README` — the sealed digest contract (`<name>.gguf.digest`, line 1
  `<64-hex-sha256>  <name>.gguf`). Ships **no** descriptor — the exact GGUF is a per-box choice (§9c), so
  an unconfigured box has no baked digest and stays dormant.

**Base (`image/overlay/usr/lib/systemd/system/`) — dormant, Condition-gated units:**
- `shrek-ai-store.service` — root oneshot, `After=home.mount`, `WantedBy=local-fs.target`, gated on the AI
  marker + the store helper. Runs as root (it is the privileged deliverer that sets both ownerships).
- `shrek-ai-model.service` — the inference server (§3 below).
- `shrek-ai-mycelium.service` (slice 1) — now `After=/Wants=shrek-ai-store.service`, because its
  `ReadWritePaths=/home/dev/.mycolink` needs the store to have created that dir first.

## 3. The split store (ADR-006 §4) — layout + ownership

| Path | Owner / mode | Role |
|------|--------------|------|
| `/home/.shrek/ai/seed`  | `root:root 0755`, files `0644` | System self-knowledge — dev **reads**, never rewrites. Slice 4 fills it. |
| `/home/.shrek/ai/model` | `root:root 0755` | Model-as-data landing — the per-box GGUF is delivered here (world-readable). |
| `/home/dev/.mycolink/{brain,sessions}` | `dev:dev 0700` | Runtime brain + chat memories — dev-owned, the shell's native layout; the never-clobber namespace (§5b). |

The `/home/.shrek` anchor is `root:root 0755`, shared with (and asserted identically by) the Bench pool —
dev cannot forge records under it. A root deliverer is required precisely because it must set the
root-owned system surface *and* chown the dev-owned brain (§4: the permission model *is* the point).

## 4. The inference server — lazy-start + idle-unload (a new idiom)

No lazy/idle-unload pattern existed in the tree; this slice introduces one without socket activation
(the inference server has no native `LISTEN_FDS` — Fable's pipewire-precedent correction):

- **Lazy-start:** `shrek-ai-model.service` has **no `[Install]`/`WantedBy`** — it is not started at boot.
  The `shrek ai` front-door (slice 5) issues `systemctl start shrek-ai-model.service` on the first
  model-needing turn. A resident multi-GB RSS on modest hardware is a real cost (§3).
- **Idle-unload:** the launcher self-exits after an idle window; `Restart=no` keeps it down until the next
  on-demand start.
- **Model-as-data gate:** `ExecStartPre=shrek-ai-model-verify` — a non-zero pre keeps the server from
  starting and logs *why* (no model / undelivered / mismatch), so an on-demand start fails loudly rather
  than skipping silently.
- **Zero egress, kernel-enforced (§7):** `DynamicUser=yes` (no ambient authority — reads only the
  world-readable GGUF), `--host 127.0.0.1`, and `IPAddressDeny=any` + `IPAddressAllow=localhost` so the
  bind AND the kernel both confine it to loopback. With ADR-003's veth input-drop stopping benches from
  reaching the port, an unauthenticated localhost model server is acceptable.

## 5. Verification (this slice)

- **Digest helper — functionally proven** (fixtures, `SHREK_AI_MODEL_{SEAL,DATA}_DIR` overrides): no
  descriptor → exit 1; delivered + matching → exit 0 READY; GGUF removed → exit 2; corrupted → exit 3.
- **Units** — `systemd-analyze verify` accepts all three (missing launcher / dev-user notices expected for
  target-image units).
- **Split-store script** — mirrors the live `bench-pool`/`hosts-seed` mkdir+chown idiom; exercised for
  real (ownership/mode on /home) by the VM dogfood `AI` stage (slice 6), same as those helpers.
- **Absent-tolerance** — a box without `INCLUDE_AI=1` merges no marker, so `shrek-ai-store.service` and
  `shrek-ai-model.service` both Condition-skip: zero new failure surface.

## 6. Next (slices 3→6, renumbered)

Deterministic three-body seed gen into `/home/.shrek/ai/seed` (seed-version namespace, §5) → `shrek ai`
front-door with **no host-exec surface** (§6, and the on-demand model start) → dogfood `AI` stage (§9).
