# ADR-006 — The optional on-device AI layer (`shrek-ai`)

**Status:** ✅ **Accepted** — owner sign-off **GO** (2026-09-02). **Mode A (on-box, offline,
zero-egress) ratified as the north-star**; B/C are operator-selectable capabilities gated by §7.
**M1 resident model class fixed: small 3–4B Q4 (~2–3 GB GGUF, ~8 GB-RAM hardware floor)** — the
widest-reach tier so the assistant is a promise on almost any Shrek box (larger models remain a
per-box opt-in via the model-as-data mechanism, §3). All §9 decisions closed. Design frozen for
M1. Nothing is built yet. This ADR locks the *shape* of hooking a language model to the Shrek OS
skeleton before any code lands, per the project rule "design-lock an OS slice before building
it." It reuses two proven patterns — the **writable-`/home` state store beside the sealed
`/usr`** (ADR-005) and the **first-boot seed-once/deliver oneshot** (ADR-005 §6) — with the
corrections in §5/§8 where the reuse is *not* verbatim.

**Fable-5 adversarial design review — GO-WITH-FIXES (2026-09-02, round 1).** All 11 must-fixes
folded into the body below. The review's load-bearing catches: (1) Mode A is only zero-egress if
mycelium runs *on-box* — folded into §3; (2) the brain permission model was broken (a `root:root
0700` store consumed by the uid-1000 shell) — split store, §4, closes §9b; (3) "never drifts" was
a hand-wave — deterministic in-tree generator + seed-version replace-on-update, §5; (4) the
"acts through Bench" safety claim was aspiration — the shell now ships with **no host-exec tool
surface**, §6; (5) AI + a networked bench is an exfil path — AI runs are empty-net-only + per-run
ceremony otherwise, §7; (6) mode is enforced by the **firewall + ceremony**, not dev-writable
config, §7; (7) Mode C needs a host-side **broker**, not in-process redaction, §7; (8) the seed
oneshot ships in a late-merging sysext (the #2904/#2795 timing class) — trigger mechanism named,
§8. Residual owner calls surfaced by the review are in §9.

---

## 1. Context — the gap

Shrek OS is a sealed dm-verity Secure-Boot desktop: read-only `/usr` closure, optional
capabilities as signed "Onion" sysext layers, writable persistent `/home` on a separate data
disk, a Bench sandboxed-compute plane (ADR-003), owner-provisioned identity (ADR-005). It sells
on an auditable "small fixed service set + egress firewall, no user data leaves the box" claim.
It has **no resident assistant**.

`mycolink-shell` (in the sibling repo `agent-harness`, `agent_harness/shell/`, v1.10,
substrate-feature-complete) is a stdlib-Python REPL that already speaks to a local
OpenAI-compatible model endpoint, a frontier CLI subprocess (`claude --print`, subscription
auth, no API keys), mycelium (`/recall`), slinkd (`/tail`), and cartridge tool-loops.

The opportunity: make it the **optional AI layer of the OS** — a model hooked to the skeleton
that *knows the machine it lives on*. "Optional" is load-bearing: a Shrek box must be fully
usable and byte-clean with the layer absent, and the layer must not weaken the sealed-appliance
security story when present. Two things must be *designed*, not just wired: where the model lives
(§2), and the seed brain + behavior (§5).

## 2. Model placement — three configurable modes, one north-star

`mycolink-shell` supports all three transports; the OS selects a **baked default** and lets the
operator change it *through the enforcement path in §7* (not by editing a dev-writable file).

- **Mode A — On-box local (north-star candidate).** A small quantized model runs on the Shrek
  device (an inference server on `localhost`), and — per Fable fix 1 — **an on-box mycelium
  instance** serves the brain from `/home`, both bound to `127.0.0.1`. `mycolink-shell` points at
  both. **Fully offline, zero egress**; the only mode that keeps the appliance claim intact with
  AI on. Constraint: modest hardware ⇒ small resident model; larger models are opt-in for capable
  boxes, never assumed. (Also immune to the fragility hit at design time: EVO-X2 `:8100` was
  unreachable — an on-box model has no such dependency.)
- **Mode B — Endpoint-backed.** The box runs the shell but points at an external
  OpenAI-compatible endpoint provisioned at first-run. Thin on-device; **requires network + a
  reachable server**, so it forfeits the offline property. The endpoint URL (and any key) is
  egress config / a possible secret and lives on its **own tightly-permissioned path**, not in
  plain `config.json` (the ADR-005 wifi-PSK discipline).
- **Mode C — Frontier-CLI escalate.** `/escalate` to a frontier model for heavy reasoning, with A
  or B on cheap turns. Most capable; **needs egress + auth**; mediated only through the §7 broker.

**Recommendation (for owner ratification, §9a):** ship **Mode A as default and north-star** — the
only mode that keeps the sealed/offline story a property of the *box* rather than the network —
with **B and C as operator-selectable capabilities gated by §7**. Mirrors how the OS treats
network/FDE: secure default, capability behind explicit authority-declaring choice.

## 3. Packaging — the `shrek-ai` optional Onion

A **signed sysext Onion**, same build shape as `shrek-dev`/`shrek-bench`/`shrek-browser`
(ADR-003), **absent by default**, gated behind `INCLUDE_AI=1` + onion-policy enable
(listed-but-absent tolerant — a box without it takes no new failure). **Complete Mode-A process
set** (Fable fix 1 — the shell's substrate is network services, so they must be enumerated and
placed on-box):

- `shrek-ai-model.service` — the inference server (candidate: `llama.cpp` server), `127.0.0.1`
  only.
- `shrek-ai-mycelium.service` — an on-box mycelium serving the brain DB from `/home` (§4),
  `127.0.0.1` only. **slinkd is deliberately absent on a Shrek device**; `/tail` degrades to a
  clean "no bus on this box" (see §6 — the M1 claim is *not* that `/tail` works).
- The `shrek ai` / `shrek chat` front-door command (the shell package).
- The baked **seed brain** (§5) as read-only data under `/usr/share/shrek/ai/seed/`.

**Model artifact — decision required (Fable fix 9, closes part of §9c).** A 3–8B Q4 GGUF is
≈2–5 GB — larger than `shrek-apps` (1.2 G); baking it into the Onion means that size in
verity-sign time, sysext staging, and *per-update* download. The ADR **chooses the model-as-data
path** (the proven bench-seed pattern): the GGUF is a **digest-keyed artifact delivered to `/home`
and verified against a baked digest**, loaded on demand — decoupling multi-GB model updates from
Onion signing. Only the runtime + digest ship sealed. The inference server has **no native socket
activation** (unlike pipewire — Fable's precedent correction), so it is a **lazy-start unit with
an idle-unload policy** (a resident multi-GB RSS on modest hardware is a real cost), not a
`LISTEN_FDS` socket unit. `agent-harness` is **pinned by commit/digest** in the Onion build
(vendored snapshot or pinned checkout, digest logged per the `# VERIFY` discipline) — a
reproducible sealed image cannot float on a sibling repo's HEAD (Fable fix 11, closes §9d).

## 4. State model — split store: system-seed read-only, runtime brain dev-owned

`/usr` stays sealed; **all mutable AI state lives on the persistent shrek-data `/home`.** Fable
fix 2 (the permission model was broken — the shell runs as **dev/uid-1000**, so a `root:root
0700` supervisor store can be neither read nor written by it without destroying the
forgery-anchor discipline). The store is therefore **split**, which closes §9b (load-bearing, not
deferrable — it *is* the permission model):

- **System self-knowledge (root-delivered, read-only to dev).** The baked seed (§5) is delivered
  by a root oneshot into a root-owned, group/other-readable surface (the workshop-recipe
  `0755`/group-read shape) — dev can *read* the OS's self-description, never rewrite it.
- **Runtime brain + chat memories (dev-owned).** The assistant's own mycelium brain (what it
  writes as it learns) and session JSONL live under **`~/.mycolink/`** (`brain/`, `sessions/`,
  `config.json`) — dev-owned, on `/home`, the shell's native layout. The on-box mycelium
  (§3) serves this dev-owned brain and mounts the system-seed set read-only.

Binds/DBs on `/home` persist; `/usr` is not writable — so the ADR-005 discipline holds:
materialize once, deliver every boot.

## 5. The seed brain — a self-aware OS, deterministically generated

Three baked seed bodies under `/usr/share/shrek/ai/seed/`, loaded into the brain by a first-boot
oneshot (§8) with content-hash dedup:

- **(i) Infra & architecture self-knowledge** — dense memories describing *this* system (sealed
  base + Onions, Bench, provisioning, the shell, the AI layer) so `/recall "how does Bench work"`
  returns real answers.
- **(ii) Memory-use conventions** — recall-before-you-assert, save discipline, checkpointing —
  the operating doctrine as behavior memories.
- **(iii) Default behavior config (the CLAUDE.md-equivalent)** — a shipped system prompt at
  `/usr/share/shrek/ai/system-prompt.md` (identity, directness, tool posture, safety rails) with
  an operator override on `/home`.

**"Single source of truth, never drifts" — the two hand-waves Fable named, fixed:**

- **(a) Deterministic in-tree generation.** The seed is produced by a **deterministic build step**
  (pinned chunker over the in-tree `docs/`+ADRs, **no model in the build loop** — an LLM distiller
  would make the sealed image non-reproducible and possibly need build-time egress). Output is
  committed / hash-logged per `# VERIFY`. The "single source of truth" claim holds **only for the
  seed namespace**, stated honestly.
- **(b) Replace-on-update for the seed, never-clobber for operator/runtime.** Seed-once with a
  bare `.applied` stamp would mean a later OS update carrying revised ADRs **never** updates the
  seeded brain — drift, the opposite of the claim. Fix: **seed memories are namespaced with a
  seed-version tag**; on version change the oneshot **replaces the seed-tagged set** (retracting
  stale descriptions of the old design) while **preserving all operator/runtime memories**.
  Never-clobber applies to the operator namespace only.

## 6. Surfaces & confinement

- **CLI first (M1):** `shrek ai` drops into `mycolink-shell`. `/recall` works against the on-box
  brain; `/cartridge`, `/escalate` (via §7 broker) work; **`/tail` degrades cleanly** (no on-box
  slinkd — Fable §6/§8 correction; the M1 claim is *not* "`/tail` works").
- **Confinement is structural, not aspirational (Fable fix 4).** The brain is an **injection
  channel** (seeded/recalled content can carry instruction-shaped payloads), and the REPL runs as
  dev with full ambient authority (`~/.ssh`, `config.json`, all of `/home/dev`). So M1 ships the
  shell with **no host-exec tool surface**: tool/code execution routes **exclusively through
  `shrek bench run`** against a designated AI bench; direct host exec is not a cartridge the OS
  build includes. **Invariant + dogfood negative:** the shipped shell cannot spawn a host
  subprocess, and a seeded memory containing an instruction-shaped payload produces no host-side
  effect.
- **Quickshell (later slice):** an AI panel consuming the same `~/.mycolink` session store, so CLI
  and GUI share history.

## 7. Security posture

- **Mode A:** every AI-layer listener binds `127.0.0.1` only; egress stays zero with AI on. The
  auditable service set becomes **N+2** (model + on-box mycelium) — restated honestly, not hidden
  (Fable fix 10). ADR-003's veth `input`-drop already blocks benches from reaching the localhost
  model port, which is *why* an unauthenticated localhost model server is acceptable — cited
  because it is load-bearing.
- **AI + Bench is a structural exfil path — closed (Fable fix 5).** ADR-003's sealed-egress
  invariant (a bench holding an internet profile must never receive a secret) is today enforced by
  a grant denylist + human habit; but the assistant could pipe recalled memory into a networked
  bench's argv/stdin. Fix: **AI-initiated bench runs are restricted to empty-net benches**;
  running an AI-supplied workload in a bench that holds any egress profile requires the **console
  consent ceremony per invocation** (not per-grant — ADR-003's per-`run` execution inside an
  existing grant is otherwise ceremony-free).
- **Mode selection is enforced by the firewall + ceremony, not `config.json` (Fable fix 6).**
  `~/.mycolink/config.json` is dev-writable ⇒ assistant-writable. Flipping A→B/C is a **no-op
  without the nft allowlist widening**, and that widening is an **authority-declaring operation
  gated by the gatekeeperd HIGH-authority typed-code console ceremony** on a surface the model
  cannot paint. Config records intent only and **fails closed** against the sealed policy.
- **Mode C = broker-mediated only (Fable fix 7).** In-process PII redaction is bypassable when the
  same process is prompt-injected. Fix: a **host-side escalate broker** (the `model-proxy`/
  `swamp-broker` shape ADR-003 already names) is the **sole holder of the frontier credential**
  (not dev-readable, dot-path-denylisted from benches), applies redaction on the trusted side of
  the process boundary, and is the **only principal on the egress allowlist** for the pinned
  frontier hosts. The frontier CLI's OAuth token, telemetry endpoints, and **default self-update**
  are all incoherent on a sealed appliance ⇒ auto-update disabled/pinned, hosts enumerated; the
  shell **never invokes `claude` directly**.
- The AI layer **never holds its own credential authority**: it authenticates as the operator's
  session and acts through Bench, so it cannot exceed the human's granted scope — now true by
  construction (§6 no-host-exec + this section), not by assertion.

## 8. First-boot trigger — not verbatim ADR-005 (Fable fix 8)

ADR-005's oneshots are **base-baked**; `shrek-ai-seed.service` lives in the **Onion**, and a unit
inside a late-merging sysext may be invisible when systemd computes the boot transaction (the
#2904/#2795 timing class ADR-005 round-3 closed). **Decision:** the base image carries a generic
dormant applier that, post-merge, runs any `/usr/share/shrek/*/seed` present (reusable mechanism,
not AI-specific), OR oniond kicks sysext-shipped oneshots post-merge. Either way this is
**deliberate, bounded base↔layer coupling** — recorded as the answer to §9e, not hidden behind
"reuses the pattern verbatim" (which was false on this axis).

## 9. Scope & open decisions

**M1:** Mode A, CLI-only (`shrek ai`), model-as-data GGUF, on-box mycelium, the three-body seed
brain (deterministic gen, seed-version namespace), default system prompt, `/recall` against the
seeded brain, **no host-exec surface**. Dogfood `AI` stage: model answers; seed recall returns
real self-knowledge; brain persists across reboot; **all AI listeners bind 127.0.0.1 and egress
counters stay zero**; a seeded injection payload produces no host effect.

**All owner decisions closed (2026-09-02):**
- **(a) Model-placement default** — ✅ **Mode A (on-box) ratified** as north-star, §2.
- **(c) Model class + hardware floor** — ✅ **small 3–4B Q4 (~2–3 GB), ~8 GB-RAM floor**; the
  exact GGUF is selected per-box via the model-as-data digest (§3), so the ADR stays
  size-mechanism-neutral while M1 targets the small tier.
- **(b) permission model** → split store, §4. **(d) vendor-vs-depend** → pin `agent-harness` by
  digest, §3. **(e) base coupling** → the generic post-merge applier, §8.

## Relationships

- **ADR-003** — the AI layer runs tools *through* Bench (a user-authority sibling, never a T2
  extension); its sealed-egress invariant and per-`run` aperture are what §7 hardens.
- **ADR-005** — reuses the writable-`/home` store + seed-once/deliver oneshot, with the §4
  permission split and §8 trigger correction where the reuse is not verbatim.
- **`mycolink_shell` / `cartridge_profile_design`** (mycelium) — the substrate being hooked, and
  the deferred profile work that later lets the assistant pick cartridges by purpose.
