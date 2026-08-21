# Phase 8 · Slice 1 — Agent Session

Status: **DOR LOCKED** (owner decisions folded in) → boundary/design below. First concrete
product-facing implementation of the already-defined Phase-8 `agentd` control-plane direction.
Not a new architectural phase; the later SAK/VT grant path, real attestation, and richer control
plane are subsequent Phase-8 slices.

```
Phase 8
└─ Slice 1 — Agent Session
   ├─ orchestrate existing authority   (agentd session: resolve → exec gatekeeperd)
   ├─ inspect effective authority      (read-only view of a gatekeeperd-authored record)
   ├─ deterministic workload           (coder, one turn, canned responder; live optional)
   └─ read-only lifecycle              (construct → run → observe → fail-closed teardown)
```

---

## §1 — Definition of Ready (locked)

### Intent
Extend `agentd` from one-shot `resolve` into a **session**: open a project → attest identity
(stand-in) → resolve + construct authority via gatekeeperd → run a real workload that uses the
brokered model path + `shrek find` **inside the wall** → **display the effective authority the
workload actually holds** → clean fail-closed teardown.

### Acceptance line (must prove)
A developer starts one agent session over a granted project; the session exercises brokered model
access + Swamp search entirely inside the constructed wall; the effective-authority view displays
authority **sourced from gatekeeperd's post-construction re-check, never agentd's request**; the
session ends leaving **no residual record** — and every step passes deterministically with no live
inference.

### Owner-locked decisions
1. **agentd = non-authoritative orchestrator/consumer of gatekeeperd.** No new authority root.
   gatekeeperd remains the sole wall that re-checks sealed policy and constructs
   (`invariant:shrek-gatekeeperd-wall`).
2. **Model access = compose the proven brokered path, no new egress primitive.** Reuse the sealed
   one-destination egress profiles + broker seam already shipped
   (`shrek-policy/egress.rs:106–141`: `model-anthropic`→`shrek-model-proxy:8200`, `model-claude-cli`
   →`shrek-claude-cli:8300`, `model-local`→`shrek-model:8100`). agentd forwards the profile **NAME**
   only; gatekeeperd resolves the destination from sealed policy.
3. **Deterministic model for the acceptance gate; live inference optional.** Reuse the canned-
   responder-behind-the-sealed-name pattern (`egress.rs:107`). `--live` is an opt-in smoke, never a
   gate dependency.
4. **Center = visible effective authority + lifecycle**, not agent intelligence or desktop cosmetics.

### Read-only authority boundary (LOCKED, verbatim)
> Phase-8 Slice-1 may **observe and display** effective authority only. It cannot request an
> in-session authority increase, approve one, manufacture one, or turn the authority view into a
> capability. Any authority-increasing UX remains blocked on the future trusted SAK/VT path.

This defers the entire SAK/VT trusted-decision surface (D1/D2 of the grant-protocol spine, mem
#2545) and real attestation (D-identity), with zero loss to the acceptance line. Slice-1 runs
**entirely inside a pre-provisioned granted envelope**.

### Threat bar (LOCKED): confirmation pass — sufficient
The slice adds **one** gatekeeperd-authored **ephemeral runtime record** using an already-proven
ownership/permissions pattern, with: no new authority source · no new egress · no authority
elevation · no transferable token (D3) · no new workload privilege · no interactive grants. The
confirmation pass must specifically prove:

- **C1 — non-forgeable:** the workload/agentd cannot **forge** the effective-view record.
- **C2 — non-wideable:** the workload/agentd cannot **widen** the effective view beyond the
  gatekeeperd re-checked authority.
- **C3 — clean teardown:** destroying the session **removes** the record (no residual).

If implementation reveals a channel beyond C1–C3, escalate **that piece** to a full threat pass —
not the whole slice.

### Attested identity (locked): explicit-input stand-in; defer real attestation
`subject = agentd-attested identity` stays the **contract**; slice-1 uses today's explicit
`--id`/`--profile` stand-in (`agentd/src/main.rs`). Real attestation (integrity-bound subject) is a
later Phase-8 slice.

### Effective-authority view = compose gatekeeperd-authored records (one is new)
| Authority facet | Source | Status |
|---|---|---|
| Filesystem grants (`resource_scope`) | `gatekeeperd/authority_record.rs` (`/run/shrek/authority/<session>`) | exists |
| Egress / network binding | `gatekeeperd/net_binding.rs` (`/run/shrek/net-binding/…`) | exists |
| Effective **tier + caps** + session view model | **NEW ephemeral runtime record** (§2.3) | this slice |

The view **reads gatekeeper-authored records, never agentd's argv** — the same discipline swampd
already uses (*"trusts the RECORD, never the caller"*, `authority_record.rs:6`). The new record is
**ephemeral runtime state under `/run`** — session state, not durable authority history — mirroring
`authority_record`, `net_binding`, and `onion.json`, all of which `--rm` / replace at end of life.

### Ready-criteria — all resolved
- [x] Authority-view source — composes `authority_record` + `net_binding` + one new session-view record.
- [x] agentd→gatekeeperd seam — proven `gatekeeperd sandbox` CLI argv seam (`gatekeeperd/main.rs:391`).
- [x] Attested-identity depth — explicit-input stand-in; defer SAK/VT + real attestation.
- [x] Reconcile with grant-protocol D1–D5 (mem #2545) — slice-1 is a strict **read-only subset**:
      no bearer token (D3), no authority increase (D2), D4 pre-render `semantic≤data` invariant untouched.
- [x] Deterministic gate — canned responder behind the sealed egress NAME + `shrek find` over a fixture corpus.
- [x] Workload — reuse `coder` (already speaks messages-API plaintext to the proxy, `coder/main.rs:65`); one turn.
- [x] Phase/doc-slot — **Phase 8, Slice 1 — Agent Session**.
- [x] Threat bar — confirmation pass (C1–C3).

### In / out
**In:** the `shrek session` front door; the `agentd session` orchestrator; the new session-view
record (write at construct, remove at teardown); the read-only `shrek session status` view; reuse of
`coder` one-turn + one `shrek find`; fail-closed teardown per `invariant:shrek-plane-fail-model`.
**Out (named):** SAK/VT trusted path + any authority *increase* (D1/D2); real attestation; the
deferred sealed socket verb + crypto seal; agent reasoning quality; SWAMP-5; desktop shell/GUI;
write-back (`invariant:shrek-pn1-writeback-deferred`).

### Data-model note (forward reuse)
The session-view record **schema is the prototype for the future Quickshell/Desktop "Work drawer"
data model.** The text view built here is the first consumer; the Desktop track becomes a second
read-only consumer of the same record. Schema is therefore designed as stable structured JSON, not
an ad-hoc print.

---

## §2 — Boundary / Design

### 2.1 Architecture — three thin layers, one privileged engine
```
shrek session <project> [--provider …] [--egress NAME] [--live] -- WORKLOAD…
   │  (thin composer — owns no isolation; mirrors `shrek run`)
   ▼
agentd session …            UNPRIVILEGED orchestrator/resolver
   │  step 1: caps ⊆ granted-profile ceiling            (refuse before any tier)
   │  step 2: effective_tier = max(matrix, floor, escalation)   (deterministic; no LLM)
   │  step 3: attach attested-subject stand-in (--id/--profile)
   │  step 4: exec →
   ▼
gatekeeperd sandbox --tier … --grant … --egress-profile … -- WORKLOAD…
      RE-CHECKS everything against its sealed compiled-in matrix (trusts no input number).
      CONSTRUCT → writes authority_record + net_binding + session-view record (single decision).
      RUN       → workload executes inside the wall (brokered model + shrek find).
      TEARDOWN  → removes all three ephemeral records; wall torn down; fail-closed.
```
agentd never constructs, never holds privilege, and the exec is the **already-proven CLI argv seam**
(`gatekeeperd/main.rs:391`, `if argv.first()==Some("sandbox")`). This is the *only* new mediation:
`shrek run` builds the argv itself; `shrek session` routes through agentd so the session carries a
visible authority identity.

### 2.2 Session lifecycle (read-only authority)
| State | Owner | Action | Failure mode |
|---|---|---|---|
| RESOLVE | agentd (unpriv) | ceiling check + `effective_tier` + attach subject → emit argv | `caps-exceed-profile` ⇒ refuse, nothing constructed (exit 11) |
| CONSTRUCT | gatekeeperd (priv) | re-check; build wall; write 3 records from one decision | PG5 fail-closed; exit propagated **verbatim** (`shrek run` precedent); no partial session |
| RUN | gatekeeperd | workload: one deterministic `coder` turn to canned responder over sealed egress NAME + one `shrek find` over fixture corpus | model provider loss ⇒ degrade-to-lexical (surfaced, session not killed); control-plane loss ⇒ fail closed |
| INSPECT | `shrek session status <id>` (unpriv) | READ the session-view record; render effective authority + lifecycle + semantic availability | missing/malformed record ⇒ fail-closed "no such session" |
| TEARDOWN | gatekeeperd | remove all 3 ephemeral records; tear down wall | idempotent removal; no residual (C3) |

The whole construct→run→teardown is a **single `gatekeeperd sandbox` invocation** (as `shrek run`
already is); the new record's write/remove are two hooks at the seams where `authority_record` /
`net_binding` are already written/removed (`gatekeeperd/t2_plane.rs`).

### 2.3 The session-view record — schema (Work-drawer data-model prototype)
- **Path:** `/run/shrek/session/<id>.json` (override `SHREK_SESSION_DIR` for the no-systemd
  host/container repro, mirroring `SHREK_AUTHORITY_DIR` / `SHREK_SESSION_*` env overrides).
- **Ownership/mode:** root-authored; **root-write-only**, readable by the invoking human view CLI
  (operator-readable audit-record shape, per `onion.json`). Confirm exact mode against `onion.json`
  at build; requirement is: **non-root cannot write** (⇒ C1/C2) and the host user's `shrek` can read.
- **Lifetime:** ephemeral runtime state; written once at CONSTRUCT, removed at TEARDOWN (`--rm`).
- **Not mounted into the sandbox** (`/run/shrek/*` is host-side), so the workload cannot even see,
  let alone write, the record — the structural basis of C1/C2. Even were it visible, it is
  descriptive only and `semantic≤data` (D4) already bounds it.
- **Single write-site:** the `grants` / `egress` fields are *projections* written from the **same
  re-checked decision** that writes `authority_record` / `net_binding`, so they cannot diverge from
  the enforcement truth. `authority_record` (grants) and `net_binding` (egress) remain the
  enforcement authority; this record is the display projection.

Schema `shrek-session/1` (stable JSON; additive-only evolution):
```json
{
  "schema": "shrek-session/1",
  "session": "s0",
  "state": "active",
  "subject": "<attested-stand-in id>",
  "effective": {
    "tier": "T2",
    "trust": "T-first",
    "caps": "cnarrow",
    "profile": "cnarrow",
    "grants": ["/canonical/project/path"],
    "egress_profile": "model-anthropic",
    "egress_dst": "shrek-model-proxy:8200"
  },
  "workload": ["coder", "--provider", "anthropic"],
  "model": { "provider": "anthropic", "path": "brokered", "mode": "deterministic" },
  "semantic": { "available": true, "freshness": "live", "tier": "fts+semantic" }
}
```
Slice-1 `state` domain = `{ "active" }` (the record exists **iff** the session is live; absence ==
ended). No wall-clock timestamp in-record (avoids nondeterminism in the gate); a monotonic/boot-time
start hint is a later additive field if the Work drawer needs it.

### 2.4 Deterministic acceptance gate
- Model: canned responder bound to the sealed egress NAME (`egress.rs:107` — "same seal, only the
  resolution differs"). No LAN/model dependency; `--live` opt-in only.
- Swamp: `shrek find` over a fixed fixture corpus ⇒ deterministic hit set.
- Probe: host-side / `mount-plane-gate`-style assertion emitting `SHREK_GATE: PASS Pn-agentd-session
  …` console lines, matching the existing P62-swamp gate harness (two-boot sealed-VM budget).

### 2.5 Confirmation pass (C1–C3) — proof plan
- **C1 non-forgeable:** a non-root actor (workload uid / a stand-in for agentd) attempts
  `open(O_WRONLY|O_CREAT)` on `/run/shrek/session/<id>.json` ⇒ `EACCES`; record content is unchanged.
- **C2 non-wideable:** compare the rendered view against the gatekeeperd re-checked decision — the
  view's `effective.*` equals the construction decision, never the requested argv; a request that
  exceeds the ceiling never produces a session (RESOLVE refuses first) so no widened record can exist.
- **C3 clean teardown:** after the single `gatekeeperd sandbox` invocation returns, `/run/shrek/
  session/<id>.json` is absent (alongside the already-tested `authority_record` / `net_binding`
  removal); `shrek session status <id>` fails closed.

### 2.6 New vs reused
**New (all small):**
- `agentd session` subcommand — orchestrator wrapper (resolve + exec gatekeeperd). Unprivileged.
- `gatekeeperd/src/session_view.rs` — `write_view` / `remove_view`, pattern-cloned from
  `authority_record.rs` / `net_binding.rs`; called from the `t2_plane` construct + teardown seams.
- `shrek session` front door + `shrek session status <id>` reader (dep-free line-oriented JSON, per
  `shrekctl onion status`).

**Reused (unchanged):** agentd `resolve`; gatekeeperd sandbox construction + all planes;
`authority_record`; `net_binding`; `model-proxy` / canned responder; `coder`; swampd / `shrek find`;
sealed egress profiles.

### 2.7 Invariant compliance
| Invariant | How slice-1 honors it |
|---|---|
| `shrek-gatekeeperd-wall` | agentd requests; gatekeeperd re-checks + authors all records |
| `shrek-trusted-path-no-token` (D3) | no bearer token; slice performs no authority increase |
| `shrek-semantic-authority` / D4 | view is descriptive; `semantic≤data` unchanged; no capability minted |
| `shrek-tier-no-downgrade` / PG2 | `effective = max(matrix, floor, escalation)`; view shows the re-checked tier |
| `shrek-plane-fail-model` | control-plane loss ⇒ session fails closed; semantic degrades on its own plane |
| `shrek-pn1-writeback-deferred` | no write-back introduced |

### 2.8 New authority / trust boundary? — NO (escalation gate)
`agentd session` adds no privilege (execs gatekeeperd, as `shrek run` already does). The session-view
record is authored by gatekeeperd (existing TCB) in an already-proven ownership pattern and is
**read-only display**; its future Work-drawer consumer is a second read-only reader, the same trust
shape as `shrekctl` reading `onion.json`. **No new authority source, no elevation, no new trust
boundary.** Per owner directive, proceed into build without a further approval ceremony; escalate
only a specific piece if C1–C3 implementation surfaces a new channel.

---

## §3 — Build plan (owner-split commits; no Co-Authored-By)
1. **build/feat** `gatekeeperd/src/session_view.rs` + `t2_plane` write/remove hooks.
2. **feat** `agentd session` orchestrator subcommand.
3. **feat** `shrek session` front door + `shrek session status <id>` reader.
4. **test** unit (`--lib/--bins`) on record write/remove + view compose; host oracle for C1–C3 +
   view-matches-construction; sealed-VM `Pn-agentd-session` assertion group (two-boot).
5. **docs** this file + `docs/graph-baselines.md` bump (baseline #8).
Then: push `constant-itis master`; `system-index refresh --scope shrek-os --graphify`.
