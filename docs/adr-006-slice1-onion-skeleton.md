# ADR-006 M1 Slice 1 — the `shrek-ai` Onion skeleton + on-box mycelium wiring

Status: BUILT (skeleton). No runtime yet — the Mode-A process set arrives in slices 2–6.
Parent: docs/adr-006-optional-ai-layer.md (Accepted 2026-09-02, Mode A north-star).
Predecessor state: installer-0 @ f84c1a7 (Installer M1 complete).

## 1. Scope

Lock the **packaging + enablement wiring** of the optional on-device AI layer before any
model/shell/seed code lands, and stand up the one runtime surface this slice owns: the
**on-box mycelium bound to `127.0.0.1`**. Nothing here loads a model or answers a `/recall`;
this slice proves the *shape* is correct and **absent-tolerant** — a box built without
`INCLUDE_AI=1` takes no new failure, exactly like `shrek-dev`/`shrek-bench`.

Explicitly **out of scope** (later slices, per ADR-006 §3/§9): the llama.cpp-class inference
server (`shrek-ai-model.service`), the model-as-data GGUF delivery + split store, the
deterministic three-body seed brain, the `shrek ai` shell front-door (no host-exec surface),
and the dogfood `AI` stage.

## 2. What shipped

1. **`layers/shrek-ai/mkosi.conf`** — a signed dm-verity sysext Onion, `ImageId=shrek-ai`,
   **marker-only** (no `Packages=`, the `shrek-hello` shape). No base-tree overlay delta is
   needed until the layer carries real packages.
2. **`layers/shrek-ai/overlay/usr/lib/shrek/layers/ai`** — the Onion identity marker (the
   `shrek-dev`/`shrek-hello` convention): present on `/usr` **iff** the layer merged.
3. **`scripts/build-ai-layer.sh`** — builds the signed DDI in an ephemeral `debian:trixie`
   container (the `build-dev-layer.sh` idiom, minus the base-tree dance since there are no
   packages yet).
4. **`scripts/build-layers.sh`** — new `INCLUDE_AI` gate (default `0`) + a `desktop`-mode
   staging block that copies `out/layers/shrek-ai*.raw` into the store **only when built**,
   mirroring `INCLUDE_BENCH`/`INCLUDE_BROWSER`/`INCLUDE_APPS`.
5. **`image/overlay/usr/lib/shrek/onion-policy`** — `enable shrek-ai`, sealed under dm-verity,
   with the same listed-but-absent tolerance comment as its siblings.
6. **`image/overlay/usr/lib/systemd/system/shrek-ai-mycelium.service`** — the on-box mycelium
   unit (§3 below).

## 3. The on-box mycelium unit — decisions

- **Base-baked, not shipped inside the Onion.** A unit living inside a late-merging sysext is
  invisible when systemd computes the boot transaction (the #2904/#2795 timing class; ADR-006
  §8). Baking the *unit file* into the sealed base — the proven `shrek-desktop-polkit.service`
  precedent — puts it in the transaction at sysinit. This is ADR-006 §8 **option (1)** (a
  base-carried dormant applier) and the concrete answer to §9e's bounded base↔layer coupling.
- **Dormant until real via two `ConditionPathExists`:** the AI marker
  (`/usr/lib/shrek/layers/ai` — did the Onion merge?) **and** the runtime launcher
  (`/usr/lib/shrek/ai/shrek-ai-mycelium` — delivered by a later slice). Either absent ⇒ clean
  condition-skip, no failure. In slice 1 the launcher does not exist yet, so the unit is wired
  but never activates — the honest skeleton claim.
- **`User=dev` (uid 1000).** The runtime brain is dev-owned under `~/.mycolink` (ADR-006 §4
  split store); a root-owned mycelium could not serve it without breaking the permission model.
- **`--host 127.0.0.1` is the load-bearing invariant** this unit exists to lock (ADR-006 §7:
  every AI-layer listener binds loopback only; egress stays zero with AI on). The launcher's
  remaining contract (port, brain path, read-only system-seed mount) is finalized with the
  runtime slice.

## 4. Absent-tolerance (the slice-1 proof)

- **No `INCLUDE_AI=1`:** `build-ai-layer.sh` is never run, nothing stages into the store,
  `enable shrek-ai` in the sealed policy resolves to a listed-but-absent layer oniond simply
  does not merge (no error) — and `shrek-ai-mycelium.service` condition-skips on the missing
  marker. Zero new failure surface. Identical behavior to `shrek-dev` when its store omits it.
- **`INCLUDE_AI=1` (this slice):** the marker Onion merges; the mycelium unit condition-skips
  on the still-absent launcher. The layer is present and inert — the intended skeleton state.

## 5. Verification (this slice)

- Static: `build-ai-layer.sh` produces a signed `out/layers/shrek-ai*.raw`;
  `systemd-analyze verify` accepts `shrek-ai-mycelium.service`; `onion-policy` lists
  `shrek-ai`. (VM-gated build/boot runs with the next DOGFOOD iteration.)
- The runtime `127.0.0.1`-bind + egress-zero + seed-recall assertions belong to the dogfood
  `AI` stage (ADR-006 §9, slice 6) once the model + launcher land.

## 6. Next (slices 2–6)

Model-as-data GGUF + split store → deterministic three-body seed gen (seed-version namespace)
→ `shrek ai` front-door with **no host-exec surface** → dogfood `AI` stage.
