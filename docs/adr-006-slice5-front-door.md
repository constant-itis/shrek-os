# ADR-006 M1 Slice 5 — the `shrek ai` front door (hardened mycolink-shell + model launcher)

Status: BUILT + proven end-to-end on host python3. Owner directive 2026-09-02: vendor + harden a
digest-pinned mycolink-shell snapshot; remove every host-exec/escalation/agent-dispatch/process-spawn
primitive from the shipped source tree; add narrow Shrek adapters; retain the loopback model endpoint;
treat the patchset + hardened-source digest as reproducible Onion provenance.
Parent: docs/adr-006-optional-ai-layer.md §3/§6, docs/adr-006-shrek-memory-api.md,
docs/adr-006-shrek-tool-contract.md.

## 1. Scope

The `shrek ai` front door: a capability-reduced derivative of mycolink-shell that talks ONLY to on-box
loopback services (model + memory), loads the Shrek system prompt, and has **no host-exec surface** — plus
the `shrek-ai-model` launcher that makes slice-2's `shrek-ai-model.service` real. This makes slices 1–4
live: Donkey recalls from the seeded brain (slice 3/4) and answers via the on-box model.

## 2. Derive-by-hardening (structural no-host-exec)

The shipped shell is DERIVED from a digest-pinned agent-harness snapshot; the primitives are **physically
removed from the source tree at build time**, not merely unwired (ADR-006 §6: "the shipped shell cannot
spawn a host subprocess").

- **`scripts/harden-mycolink-shell.py`** — deletes every host-exec/escalation/agent-dispatch/process-spawn
  module (10: `escalate.py`, `exec_shell.py`, `escalate_opus.py`, `dispatch_sonnet.py`,
  `oneplus_workflows.py`, `dispatch_tool.py`, `dispatch.py`, `cartridge_lifecycle.py`, `_commit.py`,
  `cookbook/hwscan.py`) and patches the importers/registrations (`agent.py`, `shell/{repl,substrate_tools,
  commands}.py`) so the shell still imports.
- **`scripts/shrek-ai-noexec-check.sh`** — the §6 structural gate: greps the hardened tree for real
  import/call sites (`subprocess`, `os.system/popen/exec/spawn`, `pty`, `shutil.which`). A non-empty match
  FAILS the build.
- **`scripts/shrek-shell-adapters.py`** — adds the two narrow adapters: the **Shrek Memory API** recall
  transport (patches `mycelium_recall.py`'s `_mcp_handshake_and_call`/`_extract_recall_text` to POST to the
  loopback service; keeps the module's public surface so sibling tools still import) and the **file-backed
  system prompt** (`MYCOLINK_SYSTEM_PROMPT_FILE`). The loopback **model endpoint is retained** as-is (the
  existing `AGENT_HARNESS_TIER2_*` env config).
- **`scripts/vendor-agent-harness.sh`** — `git archive <pinned commit>` → harden → adapt → gate → verify
  the hardened-tree digest against the pin → stage into the (git-ignored) Onion overlay. `build-ai-layer.sh`
  calls it before mkosi.
- **`layers/shrek-ai/agent-harness.pin`** — provenance: source commit + the reproducible hardened-tree
  sha256. The vendored tree is a build product (git-ignored); the pin + patchset + gate are the committed
  provenance (ADR-006 §3 — a sealed image cannot float on the sibling's HEAD).

## 3. The front door + model launcher

- `overlay/usr/lib/shrek/ai/shrek-ai-front-door` (+ `usr/bin/shrek-ai`) — points tier2 at the loopback
  llama.cpp (`127.0.0.1:8198`), writes `~/.mycolink/config.json` aiming the recall client at the Shrek
  Memory API (`127.0.0.1:8199`), sets the system-prompt file (operator override on /home wins), starts the
  model on demand + warms the idle marker, then execs the hardened shell (`cli.main(['shell'])`).
- `overlay/usr/lib/shrek/ai/shrek-ai-model` — the llama.cpp launcher slice-2's service waits on:
  verify-gated (`shrek-ai-model-verify`), loopback-only (refuses non-127.0.0.1), runs `llama-server`, and
  idle-unloads via the activity marker the front door touches. The `llama-server` binary is the sealed
  runtime (vendored/pinned in a later build step; the GGUF is model-as-data on /home).

## 4. Verification (this slice)

Proven against the **real** pinned snapshot on host python3:
- **No-exec gate PASS** on the hardened tree — zero host-exec/process-spawn code sites.
- **Shell still imports** (`run_shell` OK); **every** exec/dispatch tool absent from the registry (15 safe
  tools remain: read/write/list files, fold, mycelium_recall/save, slinkd, …).
- **Recall adapter** returns real Shrek self-knowledge from the live Shrek Memory API (loopback).
- **System-prompt hook** loads the file-backed prompt.
- **Reproducible provenance**: vendor pipeline produces the pinned digest, identically across runs.

**VM-gated (deferred, as for slices 1–4):** the `llama-server` binary vendoring + the actual sysext build
(python3 + the vendored shell in the Onion) + boot + a live model turn. The shell/adapters/gate are proven
on host python3; the model runtime is not yet packaged.

## 5. Next (slice 6 — dogfood)

Boot the whole AI layer in a VM and assert the §9 invariants: model answers; seed recall returns real
self-knowledge; brain persists across reboot; all AI listeners bind 127.0.0.1 and egress counters stay
zero; a seeded injection payload produces no host effect; the shipped shell cannot spawn a subprocess.
