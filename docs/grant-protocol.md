# Grant protocol — the trusted-path capability approval (policy/agent-UI)

> Design-of-record for the one Shrek-native shell role with no freedesktop standard: the **grant
> prompt**. When a sandboxed agent asks for authority beyond its current profile (a mount, a network
> reach, a capability, a higher trust tier), a **human approves on a surface the sandbox cannot spoof,
> overlay, or capture** — and the approval carries the `semantic ≤ data authority` invariant into the
> UI. This doc specs the protocol and the trusted path; it does **not** re-spec the broker daemon.

## Scope / lane

This is the **UI + protocol** design. The privileged broker that realizes grants — the socket, peer
auth, the merge/mount privilege — is `gatekeeperd`, specced as a build slice in
[`phase4-gatekeeperd.md`](phase4-gatekeeperd.md). The grant protocol **reuses that skeleton verbatim**
(root-owned socket, `SO_PEERCRED`, two-plane fail model, structured audit) and adds a verb family + the
human trusted path on top. Where this doc and the broker slice disagree, the broker slice wins on daemon
internals; this doc owns the *approval semantics* and the *rendered surface*.

## What a grant is

```
grant := (subject, caps[], trust_tier, resource_scope[], lifetime, constraints)
```

- **subject** — an `agentd`-**attested** identity, never agent-asserted (§Identity). Bound to a
  `(pid, start-time)` / session tuple, not a bare PID.
- **caps** — e.g. `vault-read`, `net-egress:api.example.com`, `fs-write:/project`.
- **trust_tier** — T0–T3 (isolation.md); the blast *wall*, orthogonal to caps (the blast *radius*).
- **resource_scope** — the exact paths/hosts that mount in (virtio-fs) or open (nftables).
- **lifetime** — `once` | `session` | `until-expiry` | `persistent` (§Lifetime).
- **constraints** — optional (time window, read-only, byte cap).

## Request path (the agent cannot self-serve)

```
agent (sandboxed)  ──proposal──▶  agentd  ──attested request──▶  gatekeeperd  ──▶  [human]  ──▶ apply
        │                           │                                │
   narrow submit only          binds identity,               invariant pre-check,
   (no grant API in-jail)       normalizes, rate-limits       trusted-path render, apply
```

Three non-negotiables, each a known failure mode elsewhere:

1. **The sandbox has no grant API in-jail.** It emits a *proposal* through a narrow, unprivileged
   channel; `agentd` is what actually speaks to `gatekeeperd`. A compromised agent can ask; it cannot
   reach the broker directly.
2. **Identity is attested, not asserted.** `agentd` binds *which* agent this is; `gatekeeperd` verifies
   the peer with kernel `SO_PEERCRED`. (polkit CVE-2018-1116 was trusting a caller-supplied UID.)
3. **The agent never self-approves and never carries the result as authority** (§D3).

## D1 — The trusted path (the load-bearing decision)

**Principle (every prior-art system shares it):** the prompt is rendered by a process the requester has
**no code path to reach**, and the requester's own surface is walled off from the trusted surface's
z-order and input. Qubes (dom0 GUI daemon draws the border a VM can't paint), Flatpak portals
(out-of-sandbox backend), Android (SystemUI-owned dialog) all work this way. None rely on the sandbox
behaving.

**Trigger (unified across rungs):** systemd 257's **`SecureAttentionKey`** logind D-Bus signal
(Ctrl+Alt+Shift+Esc). This is the modern replacement for the dead classic SAK (`do_SAK()` is
console-only, over-kills `/dev/console` holders, absent under Wayland). The signal fires in **both**
console and graphical sessions — one trigger, every rung.

**Renderer (per rung):**

| Rung | Trusted surface | Trust is enforced by |
| --- | --- | --- |
| 0 / 1 (console) | a `gatekeeperd`-owned VT via logind `TakeControl`/`SwitchTo` | **kernel** — `VT_ACTIVATE` needs `CAP_SYS_TTY_CONFIG` or the controlling tty; no device node in the sandbox mount-ns ⇒ no fd ⇒ no ioctl |
| 2 (Wayland) | a compositor-launched client with `ext-session-lock-v1`-style exclusivity | **compositor policy** — the interface is restricted to that one trusted binary; `wp_security_context_v1` strips sandboxed clients of capture/privileged binds |

**Decision:** the **console VT is the primary anchor and the security floor** — kernel-enforced beats
compositor-policy-enforced, works at every rung including bare Rung 0, and is one surface to audit. The
in-session **graphical overlay is a Phase-10 UX refinement, not a correctness requirement** (it also
needs a per-compositor screencopy-exclusion audit — capture-exclusion of the trusted surface is
compositor policy, not wire-guaranteed). This ordering is the roadmap's own: *correctness → security →
… → cosmetics*. Until the overlay ships, the shell layers only **notify** ("grant pending — press
⟨SecureAttentionKey⟩ to review"); the decision always lands on the VT.

## D2 — Risk-tiered ceremony

The deterministic wall is always enforced; the *ceremony* scales with risk along **two** axes — the
grant's authority delta, and whether the action's **trigger is tainted** (extracted from untrusted
content the agent read; agents.md §8, security-model.md §8b).

- **Authority-increasing** grants (new caps, wider scope, higher tier) → the full SecureAttentionKey →
  trusted-surface path (the security-model **TP1** gate).
- **Non-escalating / informational** acknowledgements → may be handled inline in the shell.
- **Tainted-origin actions** (the trigger is untrusted content) → **cannot self-serve and cannot be
  silently inlined**, even when in-profile. An in-profile action would otherwise execute inside the
  sandbox untouched by `gatekeeperd` (Landlock permits it), so demote-to-propose here means **`agentd`
  HOLDS the action** and forwards an **action-descriptor proposal** — a verb carrying the action plus its
  tainted provenance with an *empty capability delta* — over the same transport/fail/audit machinery as a
  real grant. A tainted-origin *in-profile* action then requires an **explicit human ack** (an inline
  surface is acceptable; silent auto-apply is not); a tainted-origin *authority-increasing* action takes
  the full TP1 path; the answer timeout ⇒ denied (Anti-abuse). The dangerous case is the in-profile
  confused deputy, so in-profile is exactly what must not auto-apply when the trigger is untrusted.
  *Where the runtime cannot hold an action for ack, this degrades to security-model.md §8b's tripwire — the missed-path
  in-profile baseline, bounded by the wall, never a silent breach.*
- **Tag granularity (deterministic floor + named residual).** `agentd` tags proposals at **session
  granularity**: once it has mediated any untrusted-classified ingest for an agent, that agent's
  subsequent proposals are tagged `tainted-origin` — coarse (it over-taints) but **unspoofable and not
  dependent on in-model action↔trigger linking**. Ingest `agentd` did **not** mediate (a direct
  in-sandbox read of an untrusted file) is the **tripwire residual** (security-model.md §8b's two failure
  halves); an unattributable trigger is treated as tainted (fail-high). So the ceremony is tripwire-grade,
  not wall-grade — the deterministic part is the tagging of *mediated* ingest, not a guarantee the trigger
  was ever seen.
- **Content endorsement** (clearing the taint on a file so later sessions may read it as trusted) is
  itself an **authority-increasing act** → TP1, never a casual inline ack — a casual ack would make
  "please endorse this file" the next injection payload. It is the *only* tainted→trusted flip
  (security-model.md §8b), and it is inherently **persistent** (it changes how *future* sessions read the
  file), so it is a **`persistent` grant class** (D5): its apply target is the **provenance store**, not a
  sandbox cap (it therefore falls outside D3's apply-to-sandbox model); it **bumps the TPM NV
  monotonic-counter** and is checked every load, so an offline rollback cannot resurrect an endorsement of
  a since-re-tainted file; **revocation = re-taint** (also bumps the counter). Endorsement is **never
  pre-bakeable** into a Rung-0 cap-set (see Rung-0).

## D3 — No bearer token, ever

The human tells **`gatekeeperd`** "yes"; `gatekeeperd` then **applies the capability directly to the
sandbox** (adds the virtio-fs mount / nftables rule / cap) via the isolation runtime. The agent never
receives or carries an approval token — there is nothing to steal, replay, or leak. The request carries
a `gatekeeperd`-issued **nonce** so a stale "approved" cannot be fabricated.

For a tainted-origin **action-descriptor** (D2, empty cap-delta) there is nothing to apply to the
sandbox: "approve" means `gatekeeperd`'s result returns to `agentd` over the same attested channel and
`agentd` **releases** the held action (deny/timeout ⇒ discards it). The agent still receives nothing it
can carry — the no-bearer property holds identically whether the outcome is a new capability or the
release of a held in-profile action.

*Corollary (why this matters):* because no capability flows *through* the UI, a pixel-perfect **fake**
prompt the agent draws in its own window grants it **nothing** (the Flatpak-portal property). The
residual threat is therefore **social** — a fake prompt to trick the human, or suppressing/obscuring the
real one — which is exactly what the D1 anchor (can't draw where the real prompt lives) defends.

## D4 — The invariant is enforced before the human is asked

`gatekeeperd` **auto-denies** anything that would violate `semantic authority ≤ data authority`
(fail-high) **before** rendering — the human never sees it. The trusted path is only for *discretion
within the safe envelope*. **Human error cannot breach the wall**, because the wall is deterministic and
upstream of the prompt. (This mirrors the broker's "the caller's request is never an authority" re-check
in `phase4-gatekeeperd.md` §independent-re-check.)

## D5 — Lifetime and the TPM

- `once` / `session` / `until-expiry` → **ephemeral**: live in volatile `/var`, gone on
  reboot/session-end. No persistent state, no TPM.
- `persistent` → written to the **mutable-policy plane** with fs-verity + a **TPM NV monotonic-counter**
  bump (security-model.md §4), checked every load, so an offline snapshot-rollback cannot resurrect a
  revoked or superseded grant. **Revocation bumps the counter too.**

The grant protocol is the feature that **introduces the first mutable grants** — so it is the slice that
must land the NV-counter machinery the broker slice explicitly defers.

## Legibility (what renders on the trusted surface)

A **diff**, not a wall of text: *current* authority vs *requested*, the exact scope, tier, and lifetime.
The agent-supplied **reason** is rendered as **untrusted text** — control/escape sequences stripped — so
it can neither forge the chrome nor inject a fake prompt. This defeats the prompt-injection and
terminal-escape chrome-spoof in one move.

**Lethal-trifecta warning.** When the resulting profile would compose `untrusted-read + network egress`
— the agent already holds (or is here requesting) an untrusted read **and** any network reach — the
surface renders an explicit **trifecta warning**: that pairing is a presumptive exfiltration channel
(EchoLeak / Supabase-MCP class — legitimate-read chained to legitimate-egress), even if each capability
is individually in-policy (security-model.md §8b). It is never rendered as routine. (The warning is
surface-bound, so a pre-baked Rung-0 cap-set that composes the pair renders nothing at runtime — that
composition is caught by a policy-authoring lint, not this warning.)

**Tainted-origin ack surface.** A tainted-origin in-profile ack has an *empty* authority diff (the
action is already in-profile), so it must **not** render as a content-free "OK?" — that trains the
fatigue-click security-model.md §8b warns about. It renders the **action descriptor**, the **tainted trigger's provenance**
(as untrusted text, escapes stripped, so it can't spoof the chrome), and a **why-asked banner** ("an
untrusted source triggered this — confirm you intended it"). This is the difference between
authenticating *intent* (the tainted ack) and extending *authority* (the D4 discretion) — two different
questions on the same trusted surface.

## Anti-abuse

- **Fail-closed, two-plane** (security-model §7): `gatekeeperd` unreachable or the human doesn't answer
  within timeout ⇒ **denied** (agent-execution plane fails closed). Box availability is unaffected
  (availability plane fails open). *Never default-to-privileged on an incomplete handshake* — polkit
  CVE-2021-3560 was a D-Bus race falling back to UID 0.
- **Anti-flood:** the agent→`agentd` channel is rate-limited and **coalesced per (agent, cap-family)**;
  a pending-grant storm **auto-denies** rather than letting an agent DoS the human into fatigue-clicking.
  A flood of tainted-origin acks triggered by one poisoned document therefore auto-denies (fail-closed) —
  which can also deny the agent's *legitimate* grants in that cap-family; that is intended fail-closed
  behavior, not a regression (untrusted content must not be able to *force* an approval, only lose one).

## Identity & auth (the CVE-driven rules)

- **`SO_PEERCRED`, never caller-asserted identity** (CVE-2018-1116). Resolves the broker slice's open
  `SO_PEERCRED`-vs-`SocketMode` item in favor of kernel-verified peer creds.
- **Bind to `(pid, start-time)` / session tuple, never a bare PID** (CVE-2019-6133 — PID-reuse hijack).
- **The enforcement binary is memory-safe (Rust)** (CVE-2021-4034 / PwnKit). The grant UI is authored in
  the same cargo workspace as `gatekeeperd`, so the daemon that authenticates a grant and the UI that
  renders it share types with no serialization seam between them.

## Sandbox prerequisites (a Phase-5 dependency, called out here)

The VT anchor is only sound if the sandbox construction (Phase 5) **strips input/console device nodes**:

- No `/dev/console`, `/dev/tty0` in the sandbox (CVE-2025-52565 is a live runc console-node escape).
- **Strip `evdev` (`/dev/input/event*`)** — raw evdev reads bypass VT routing entirely; a sandbox with
  evdev can snoop or inject keystrokes regardless of which VT is active. This is a **sharper edge than
  `VT_ACTIVATE`** and is equally load-bearing.

## Relationship to the broker skeleton

Reused **unchanged** from `phase4-gatekeeperd.md`: the root-owned unix socket, `SO_PEERCRED` peer auth,
`Restart=always` + socket activation, the two-plane fail model, and the structured audit sink. The grant
protocol adds a **`grant` verb family**. The one place it outgrows the broker's line-oriented
`<VERB> [name…]` wire format is the request **payload** — a grant tuple (caps, scope, lifetime, nonce,
attested subject) is not a bare layer name — so the grant verbs carry a structured payload while keeping
the same transport, auth, fail, and audit machinery. The tainted-origin **action-descriptor** proposal
(D2) reuses that same payload channel with an *empty capability delta*: it asks the human to authenticate
an in-profile action's intent, not to extend authority, but rides identical transport/fail/audit so the
ack and its timeout are enforced by the broker, not left to the agent's compliance.

## Deferred / open

- **Graphical trusted overlay** (Rung-2 in-session prompt) — Phase-10 UX; requires the per-compositor
  screencopy-exclusion audit before it can be a *primary* anchor.
- **Rung-0 headless policy (settled):** an unattended box grants **only pre-baked cap-sets** in the
  sealed policy; interactive escalation **requires a human at the console, full stop**. No silent
  auto-grant. **Tainted-origin exclusion (security-model.md §8b):** a `tainted-origin` proposal — authority-increasing OR
  in-profile — is **never** auto-satisfied from a pre-baked cap-set; it fails to *denied-pending-human*
  (headless, that resolves to denied via the answer timeout). Pre-baked auto-grant is for the agent's
  own intent, never for an action a piece of untrusted content asked for. And **content endorsement is
  never pre-bakeable** — a headless box can never flip tainted→trusted without a human, even for an
  agent's own-intent endorsement (which may carry no taint tag).
- **Coalescing granularity (settled):** coalesce per `(agent, cap-family)` — finer than per-agent (so a
  broad ask doesn't smuggle unrelated caps under one approval), coarser than per-cap (so a legitimate
  multi-cap task isn't death-by-a-thousand-prompts). **The key also carries the taint bit**
  (security-model.md §8b): a `tainted-origin` proposal is **never** coalesced with an untainted one — so a
  coalesced group is uniformly tainted or clean, and renders accordingly. Else a tainted ask rides a clean
  approval under the same cap-family, exactly the smuggling this finer key exists to prevent, reopened on
  the taint axis.
- **Grant-UI front-end toolkit for Rung 2** (slint vs iced vs gtk-rs) — decide when the overlay is built;
  the core (gatekeeperd client + the `caps × trust × semantic≤data` model) is toolkit-independent.
- **TPM NV-counter mechanics** — shares the security-model §4 machinery; concrete NV-index layout and the
  greenboot-healthy commit gate are specced with the first persistent-grant implementation.
- **Policy-authoring lint for the lethal trifecta** — the runtime trifecta warning (Legibility) is
  surface-bound, so a pre-baked Rung-0 cap-set that statically composes `untrusted-read + network egress`
  renders nothing. A build-time lint over sealed cap-sets is what catches that composition; specced with
  the sealed-policy authoring tooling (security-model.md §4).
