# Shrek OS — Agents (identity, profile, lifecycle)

> Donkey is a citizen of the swamp, not its king. He carries papers that say exactly what he may
> touch, the kernel checks them at every door, and when he wants a new door opened a human — not
> Donkey — turns the key.

This document owns **what an agent *is*** and **what it is allowed to do**: the attested identity,
the capability profile, the trust band, and the lifecycle from spawn to termination. It is the hub
that ties the agent-facing docs together — where an agent *runs* is [`isolation.md`](isolation.md),
how it *asks for more authority* is [`grant-protocol.md`](grant-protocol.md), how it *queries the
filesystem* is [`swamp.md`](swamp.md) / [`filesystem-intelligence.md`](filesystem-intelligence.md),
and the capability *vocabulary* it is written in is [`architecture.md`](architecture.md) §6. This
doc references those; it does not re-spec them.

The invariant every agent is a subject of:

```
An agent's EFFECTIVE authority  =  its SEALED profile  ∩  its live GRANTS  —  never more.
  · The profile is a default-deny allow-set: anything not granted is denied (architecture.md §6).
  · Grants can only ADD within the safe envelope; a grant that would violate
    semantic authority ≤ data authority is auto-denied before a human ever sees it
    (grant-protocol.md §D4).
  · Identity is ATTESTED, never agent-asserted. An agent cannot rename itself into more authority.
```

---

## 1. Scope & non-goals

**This document owns the agent as a subject:** its identity and attestation, the profile schema,
the trust band, `agentd`'s role, the lifecycle, and how an agent's actions are routed so it can
never self-serve authority.

**It does not own execution mechanism.** *Which* isolation tier runs a workload and how the sandbox
is physically built (mounts, network, rootfs) is [`isolation.md`](isolation.md). This doc decides
*who the agent is and what it may ask for*; that doc enforces *how strongly it is contained*.

**It does not own the grant ceremony.** The trusted-path approval flow (secure-attention trigger,
the VT surface, no-bearer-token, lifetimes, the CVE-driven auth rules) is
[`grant-protocol.md`](grant-protocol.md). This doc references the *request path* and treats grants
as inputs to effective authority.

**It does not own the capability vocabulary.** The verbs (`discover`, `read`, `write`, `execute`,
`index`, `embed`, `search`, `summarize`, `relate`, `export`, `network`) are defined in
[`architecture.md`](architecture.md) §6. This doc composes them into profiles.

## 2. What an agent is — the attested triple

An agent is not a process that claims a name. It is a kernel-attested identity carrying a sealed
profile and a trust band:

```
agent := (identity, profile, trust_band)

  identity    — an agentd-attested subject, bound to a (pid, start-time) / session tuple,
                NEVER a bare PID (PID-reuse hijack, grant-protocol.md §Identity). Verified by
                gatekeeperd with kernel SO_PEERCRED, never trusted from the caller.
  profile     — the sealed capability profile (§3): the default-deny allow-set of what this
                identity may do, in the architecture.md §6 vocabulary.
  trust_band  — the containment floor this identity is entitled to (§4): trusted / semi / hostile.
```

The three answer three different questions and are enforced at three points:

| Field | Question | Enforced by |
|-------|----------|-------------|
| identity | *which* agent is this, provably? | `agentd` attests, `gatekeeperd` verifies (`SO_PEERCRED`) |
| profile | *what* may it do? | compiled default-deny Landlock ruleset (the blast **radius**) |
| trust_band | *how strongly* must it be boxed? | the isolation tier floor (the blast **wall**, isolation.md) |

**Identity selects; it never gates.** Attested identity chooses *which* sealed profile applies — it
is never itself an authority check. This is the TCC lesson: macOS gates on *who is asking* (the
bundle ID) and is repeatedly bypassed by proxying a request through a more-privileged identity. In
Shrek the object-capability wall is the enforcement point; identity only routes to the profile the
wall then enforces. An agent that spoofs a name gains nothing, because the name is not the gate.

Profile and trust band are **orthogonal** — the two dials of `shrek run --trust=<band> --caps=<profile>`
(isolation.md §2). A high-trust agent can still be given a tiny profile; a tiny profile can still be
run in a hard box.

## 3. The capability profile

A profile is declarative *intent* that **compiles to a default-deny allow-set**. It is authored in
the §6 vocabulary and names exactly what the identity may reach. Anything not named is denied — a
path or host in neither the allow nor the deny list is denied, not allowed.

```yaml
agent: coder
read:    [ ~/Projects/foo/** ]
write:   [ ~/Projects/foo/** ]
search:  [ ~/Projects/foo/** ]         # swamp query scope (swamp.md §9)
network: [ github.com, crates.io ]
execute: [ cargo, git, cmake, ninja ]
denied:  [ ~/.ssh/**, ~/.gnupg/**, ~/Private/**, /boot/**, /etc/**, /usr/** ]
```

- **`denied:` is human-readable intent, not the enforcement.** It compiles to a **default-deny**
  Landlock ruleset (grant `read`/`write`/… on the allow-set, deny everything else), exactly the
  discipline of [`architecture.md`](architecture.md) §6 and [`swamp.md`](swamp.md) §5. The deny
  list documents the *why*; the kernel enforces the *what*.
- **Profiles are sealed static policy.** The profile templates ship baked into the image under the
  dm-verity root ([`security-model.md`](security-model.md) §4) — an agent cannot edit its own
  profile, and a compromised agent cannot rewrite the file to widen itself. Per-machine deviations
  are **grants** (§below), not profile edits.
- **Effective authority = profile ∩ grants.** The sealed profile is the floor of denial; a live
  grant ([`grant-protocol.md`](grant-protocol.md)) can extend it *within* the semantic ≤ data
  envelope, for a bounded lifetime. When the grant expires, effective authority returns to the
  sealed profile with no residue.
- **`discover: false` is honored end-to-end.** A profile that denies `discover` on a domain means
  the agent cannot even learn protected material *exists* — enforced at the VFS **and** in the
  swamp index (objects absent from the agent's projection, [`swamp.md`](swamp.md) §9), subject to
  the readable-file-that-names-it caveat of [`architecture.md`](architecture.md) §6.
- **Enforcement is on file descriptors, not path strings.** The compiled ruleset resolves and pins
  fds (`resolve-beneath`, the mount-TOCTOU discipline of [`security-model.md`](security-model.md))
  rather than matching path *text* — a path-string allow-set is symlink/rename-race-prone (the
  reason Capsicum forces fd-mode), and a TOCTOU walk into an unintended byte would violate
  `semantic ≤ data` without tripping a check.
- **Authority is argument- and context-sensitive, not verb-only.** A capability is not "may
  `execute`" but "may `execute` *these* binaries against *this* scope"; the same verb at a wider
  scope, on more-sensitive data, or with a larger side-effect is a *different* authority (the
  three-tier read-only / sandbox-edit / full-access model, where the tier depends on arguments,
  environment, and data sensitivity — not the tool identity). `network` and `export` in particular
  are **per-destination scoped** (`network: [github.com]`, never `network: true`) — a blanket egress
  capability is an open exfiltration channel (§8).

## 4. Trust band — the containment floor

Where the profile is the blast *radius*, the trust band is the blast *wall*: it sets the **floor**
of the isolation tier the agent runs under ([`isolation.md`](isolation.md) §5). It answers *"if
this agent is fully hostile, how hard is the box?"*, independent of how small its profile is.

```
trusted     first-party, signed, in-tree     → floor may be low (T0/T1); trust the code, box the caps
semi        third-party but reviewed          → mid floor (T2 gVisor, the workhorse)
hostile     unknown / unreviewed / untrusted  → T3 microVM

UNKNOWN band ⇒ treated as HOSTILE (fail-high) — never defaulted to trusted.
(security-model.md; isolation.md "trust-band unknown ⇒ T-hostile".)
```

The effective tier is `max(band floor, matrix, escalation)` — trust never *lowers* the wall below
its floor, and an agent can only escalate *upward* (isolation.md §5). Prompt-injecting an agent
into asking for a weaker box does nothing: the floor is not negotiable downward.

## 5. `agentd` — the identity & resolver daemon

`agentd` is the unprivileged identity plane. It is the only thing that speaks for an agent to the
privileged broker; the agent never reaches `gatekeeperd` directly.

```
agent (sandboxed)  ──proposal──▶  agentd  ──attested request──▶  gatekeeperd (privileged broker)
     narrow, unprivileged        binds identity,                 verifies SO_PEERCRED,
     channel; no broker API       resolves sealed profile,        independently RE-CHECKS the
     in-jail                      normalizes + rate-limits        request, applies within the wall
```

- **`agentd` attests identity and resolves the profile** — it binds *which* agent this is (the
  `(pid, start-time)`/session tuple) and loads the matching sealed profile. It is the *resolver*;
  it decides nothing privileged.
- **`gatekeeperd` is the wall** — the privileged broker re-checks every request against the sealed
  policy and *never trusts the caller's assertion of its own authority* (the independent re-check,
  [`isolation.md`](isolation.md) §agentd↔gatekeeperd contract, and the deployed broker in
  `phase4-gatekeeperd.md`). This two-daemon split is why a compromised `agentd` still cannot mint
  authority: it can *ask*, `gatekeeperd` *decides*.
- **Both are unprivileged-or-least-privileged and on their respective planes** — `agentd` holds no
  merge/mount capability; `gatekeeperd` is scoped to exactly the capabilities its brokered
  operations need, not arbitrary root.

## 6. Lifecycle

```
 spawn ──▶ attest ──▶ resolve profile ──▶ classify trust band ──▶ construct sandbox ──▶ run
   │         │              │                     │                      │              │
 shrek run  agentd binds  agentd loads       band ⇒ tier floor      isolation.md     agent acts (§7)
 / Donkey   (pid,start)   sealed profile     (unknown⇒hostile)      builds the box       │
                                                                                          ▼
                                              provenance appended ◀── every action ──▶ request grant?
                                              (architecture.md §8)                     (grant-protocol.md)
                                                                                          │
                                                                             human approves on trusted path
                                                                             ⇒ gatekeeperd applies, bounded
                                                                                          │
                                                       terminate ──▶ ephemeral grants evaporate; effective
                                                                     authority gone; provenance persists
```

- **Attestation precedes authority.** No profile is resolved and no sandbox is built until `agentd`
  has bound a verifiable identity. An unattested process is not an agent and gets nothing.
- **Construction is delegated to isolation.** Once band and profile are resolved, sandbox assembly
  (tier, mounts, tap+nftables, rootfs) is [`isolation.md`](isolation.md)'s job. This doc hands it a
  resolved `(profile, band)`; it does not build the box.
- **Termination is clean.** Ephemeral grants (`once`/`session`) live in volatile `/var` and are gone
  at session end ([`grant-protocol.md`](grant-protocol.md) §D5); the agent leaves behind only its
  append-only provenance (§9), never lingering authority.

## 7. How an agent acts — three routes, none self-served

Everything an agent does resolves to one of three routes, and **none of them lets the agent grant
itself authority**:

| The agent wants to… | Route | Enforced by |
|---------------------|-------|-------------|
| **find / read** something | swamp query API, caller-scoped, **authorize-before-retrieve** | [`swamp.md`](swamp.md) §9 — per-object intersection, `discover:false` absent from projection |
| **write / execute** within its scope | the sandbox it already runs in — *unless the action's trigger is tainted, then §8 denies this route* | [`isolation.md`](isolation.md) — the default-deny Landlock ruleset |
| **exceed its current profile** (a new mount/host/cap/tier) | a *proposal* → `agentd` → `gatekeeperd` → human | [`grant-protocol.md`](grant-protocol.md) — trusted path, no bearer token |

The load-bearing property: **reads authorize before they retrieve** (never global-search then
filter, [`swamp.md`](swamp.md) §9), and **new authority is never in-band** — the agent emits a
proposal through a narrow channel and receives *nothing* it can carry as authority; `gatekeeperd`
applies the capability directly to the sandbox ([`grant-protocol.md`](grant-protocol.md) §D3). A
pixel-perfect fake grant prompt the agent draws in its own window therefore grants it nothing.

There is a fourth *pseudo*-route, denied by construction: an instruction the agent extracts from
**untrusted content it read** does not get to act on its own authority — it is demoted to a
*proposal* that is specifically **denied route 2** (the self-service in-sandbox write/execute above).
It requires a human: route 3 (the trusted path) if authority-increasing, an explicit human ack if
merely in-profile — because the in-profile case is exactly the confused deputy (§8).

## 8. Untrusted content is not instruction — the in-model confused deputy

The kernel wall sees capability *boundaries*, not *intent*. `semantic ≤ data` bounds what an agent
can **reach**; it says nothing about what untrusted text *inside an authorized read* can make the
agent **decide to do**. That is the one part of the LLM threat model a capability wall structurally
cannot see — and it is where an agent becomes a confused deputy inside its own reasoning: it reads
trusted file A (allowed) and attacker-controlled file B (allowed), and B's text steers an action
against A, entirely within profile, zero grant, zero kernel event.
[`threat-model.md`](threat-model.md) ADV-9 (indirect prompt injection) and
[`security-model.md`](security-model.md) §7 name the threat; this section fixes the agent-side
stance:

```
UNTRUSTED CONTENT IS DATA, NEVER INSTRUCTION.
  Content from below the agent's trust band, or from any object not operator-authored, is tagged
  UNTRUSTED-INSTRUCTION-SOURCE when it enters the agent's context.
  An instruction extracted from tagged content MUST NOT trigger write / execute / network / grant.
  It may only PROPOSE — and a proposal from tainted content is NOT self-served: it is denied the
  self-service route (§7 route 2, in-sandbox write/execute) and requires a HUMAN acknowledgment —
  the trusted path (§7 route 3) for anything authority-increasing, an explicit ack for even an
  in-profile action. The dangerous case IS the in-profile one (the confused deputy), so in-profile
  is exactly what must not auto-execute when the trigger is untrusted text.

  Provenance fails high: a passage whose provenance is unlabeled or unattributable is treated as
  tainted (matching the unknown⇒hostile convention). Agent-authored content STAYS tainted — an
  agent must not launder instructions into a file a later session reads as trusted (the two-hop
  SpAIware write). The cost is taint saturation, cleared only by an operator-endorsement path;
  endorsement is itself an authority-increasing act (trusted-path/grant ceremony), never a casual
  inline ack — or "please endorse this file" becomes the next injection payload.
```

This is the dual-LLM / CaMeL discipline (a planner that never sees untrusted content; a quarantined
parser that returns *data only*; taint metadata so untrusted data can never influence control flow)
applied to Shrek. [`swamp.md`](swamp.md) already does authorize-before-retrieve for data *at rest*;
this extends the same posture to data *in context*. **The wall governs bytes-at-rest; this governs
bytes-in-context.** Three consequences the design must state plainly:

- **The lethal trifecta, priced honestly.** Untrusted-read + *any* network reach = a presumptive
  exfiltration channel — not only for protected data, but for *ordinary* readable-sensitive data
  encoded into permitted request params, hostnames, or timing. A coding task with
  `network: [docs.python.org]` is still a channel. Real-world: the EchoLeak Copilot exfiltration and
  the Supabase MCP leak were both legitimate-read chained to legitimate-egress, zero
  protected-capability abuse. So **`untrusted-read + network` surfaces in the grant UI as a trifecta
  warning** ([`grant-protocol.md`](grant-protocol.md) — *rule pending propagation, security-model §9*)
  — never routine merely because each capability is individually in-policy.
- **Index poisoning is partly live now, not only in SWAMP LIVING.** The deferred living-graph tier
  earns its own threat pass ([`filesystem-intelligence.md`](filesystem-intelligence.md) §8) — but the
  *shipping* semantic tier (embeddings + relationships) is already poisonable: one attacker-authored
  document in an enabled domain can bias Donkey's retrieval and ranking for *unrelated future* tasks
  (the SpAIware pattern — injection that writes memory to steer later sessions). The tag mitigates
  only the *instruction-hijack* consequence — a retrieved passage is data, its provenance travels
  into context. It does **not** fix **ranking/selection integrity**: a poisoned doc that takes the
  retrieval slot or buries the right one steers *which* content is returned with no extracted
  instruction — an open integrity residual ([`filesystem-intelligence.md`](filesystem-intelligence.md) §8).
- **Content fatigue, distinct from volume fatigue.** [`grant-protocol.md`](grant-protocol.md)
  rate-limits and coalesces proposal *volume*; it does not vet proposal *text*. An injected document
  that makes Donkey draft a plausible-sounding grant reason ("fetch the changelog for a compat
  check") is social engineering through a kernel-enforced door — the wall holds, the human opens it.
  The defense is legibility (grant-protocol renders the agent's reason as untrusted text and shows
  the *actual* authority diff, not the persuasive story) — this section is why that rule is
  load-bearing, not cosmetic.

**This is a tripwire-grade guarantee, not a wall-grade one.** Taint-tracking through an LLM's
reasoning is best-effort, exactly like the semantic DLP tripwire ([`architecture.md`](architecture.md)
§7); it must never be relied on where a deterministic capability boundary is available. The
demotion-to-*propose* has two honest halves. A **caught** tag reaches no action without a human
acknowledgment (in-profile: an explicit ack; authority-increasing: the trusted path) — its residual
is *content fatigue*, a human waving through a plausible reason. A **missed** tag (a source
mislabeled trusted, or the model failing to connect an action to its tainted trigger) executes
within the granted profile with no human — degrading to the pre-taint blast radius bounded by the
wall (`caps⊆profile`, no widening), **never a silent wall breach**. The demotion converts a missed
taint from "breached wall" down to "in-profile action the wall already bounds"; it does not, and
cannot, promise a human on every path.

## 9. Donkey — the built-in assistant

Donkey is the OS's built-in general assistant agent — the one that reasons over the swamp on the
human's behalf. It is an agent like any other: an attested identity with a sealed profile and a
trust band, subject to every rule above. It is *not* privileged, *not* exempt from the wall, and
*not* a path around `gatekeeperd`.

- **Default profile: broad discovery, narrow mutation.** Donkey ships able to `search`/`read`/
  `relate` across the user's enabled domains (so "find the thing where we worked out X" works), but
  `write`/`execute`/`export`/`network` are **not** in its default profile — those are per-task
  grants. Reasoning is cheap to allow; acting is not.
- **Scoped by logical domain, per-object.** Donkey's reach is the union of the domains it is
  enabled for, but its authority over each object is the per-object intersection at that object's
  physical home ([`filesystem-intelligence.md`](filesystem-intelligence.md) §3): human-only domains
  (`~/Vault`, `~/Identity`, `~/.ssh`) are simply **invisible** to Donkey, not merely read-denied.
- **Human-only domains are structural, not a Donkey setting.** Donkey cannot see `~/Vault` because
  `swampd` never indexed it (default-deny allow-set, [`swamp.md`](swamp.md) §5) — there is no Donkey
  configuration that can turn that visibility on, only a counter-anchored grant through the trusted
  path.

Ad-hoc agents (`shrek run --trust=… --caps=… ./task`) are the same machinery with a task-specific
profile and an explicit trust band, rather than Donkey's standing assistant profile.

## 10. Provenance & audit

Every agent action is auditable, per [`architecture.md`](architecture.md) §8: `artifact`,
`previous_hash → new_hash`, `actor`, `model`, `operation`, `reason`, `capabilities`, `network`,
`timestamp`. Surfaced via `shrek history <path>` and `shrek audit --agent <id>`, including the
load-bearing summary lines: *"Protected data accessed: NO / Secrets accessed: NO / External
network: <hosts>."* Provenance is append-only and survives the agent that wrote it — an agent's
authority is ephemeral; its record is not.

## 11. Failure behavior — the agent-execution plane fails CLOSED

Agents live on the **agent-execution plane**, which fails **closed** — the opposite of the swamp's
availability plane ([`architecture.md`](architecture.md) §9, [`security-model.md`](security-model.md)
§7):

```
gatekeeperd down / agentd down / policy unreadable  ⇒  agents CANNOT run. No unconfined fallback, ever.
  A grant handshake that cannot complete ⇒ DENIED, never default-to-privileged
  (polkit CVE-2021-3560, grant-protocol.md §Anti-abuse).

Contrast: swampd down ⇒ search gets dumber but the OS runs (availability plane, fails open).
Degrading a feature must NEVER degrade the wall.
```

An agent that cannot be attested, profiled, and boxed does not run in a weaker mode — it does not
run. This is the guarantee that keeps "the AI stack is down" from ever becoming "the AI stack is
unconfined."

## 12. Deferred

- **Multi-agent delegation with attenuation (`child ⊆ parent`).** The effective-authority rule (§0)
  must extend down a delegation chain: a spawned sub-agent's authority is a *narrowing* of its
  parent's, never an amplification. Recommended mechanism (two research traditions converge on it):
  an **attenuating derivation** — seL4's capability-derivation tree, where `Mint` can only
  equal-or-attenuate and revocation cascades to descendants; equivalently a biscuit-style signed
  caveat chain where an appended block can only *add restricting* facts, so amplification is a *type
  error, not a policy bug*. The child still gets a fresh `agentd` attestation for *identity* (binary,
  trust band — cheap, orthogonal), but its *authority rides the derivation*, not a second independent
  grant lookup: two independently-attested agents have no structural containment relation. Root
  signing key stays TPM-resident. Deferred until the derivation tree + Capsicum-style fork/exec
  inheritance are built and threat-passed.
- **Live-grant revocation.** Recommended: extend the single persistent-grant TPM NV monotonic counter
  (§D5, [`grant-protocol.md`](grant-protocol.md)) into **per-subject / per-grant-class epoch
  counters** (TPM 2.0 provides multiple NV counter indices); a grant embeds `epoch_at_mint`,
  revocation is one `NV_Increment`, and enforcement rejects `epoch_at_mint < current` — inheriting the
  same snapshot-rollback resistance as persistent grants. The no-bearer-token design (§7) is the
  enabler: the broker is the permanent indirection point, so revocation is "retract the rule,
  recompute `sealed ∩ grants` on the next mediated call." Two hard parts keep it deferred: (1) a
  compiled Landlock ruleset is an *immutable cache*, so revocation latency = how fast the broker
  rewrites the live ruleset — a *measured security SLA*, not assumed-instant; (2) an epoch bump only
  gates *future* checks, so revocation must **actively tear down live kernel state** (open fds), the
  way Android drops one-time grants on process-*kill*, not backgrounding. This is why the OS-level
  capability table is the enforcement ground truth and tokens/epochs are only the *distribution*
  layer.
- **Build-time capability lint.** A Fuchsia/Scrutiny-style static check that every capability in a
  sealed profile traces to a real resource — catching dangling or typo'd grants before an agent ever
  runs. Cheap and static; applies to the *sealed profile* only (the live-grant path is dynamic by
  nature and cannot be statically routed).
- **Standing vs one-shot Donkey grants at scale.** Donkey's default profile (§8) is a starting
  point; the ergonomics of long-lived per-domain grants vs re-prompting are a Phase-10 UX concern
  once the grant surface ([`grant-protocol.md`](grant-protocol.md)) is built.
- **Cross-machine agent identity.** Identity attestation (§2) is single-machine; an agent whose
  identity spans hosts is out of scope until the single-machine model is proven (mirrors
  [`filesystem-intelligence.md`](filesystem-intelligence.md) §8's cross-machine deferral).
- **Learned trust-band classification.** Bands (§4) are assigned from provenance (signed/in-tree vs
  unknown); a classifier that *learns* trust from behavior is a later addition and must keep
  `unknown ⇒ hostile` as the irreducible fallback.
- **Agent-to-agent audit correlation.** Per-agent provenance (§9) exists; correlating a *causal
  chain* across agents into one audit narrative is a later tooling layer.

Every deferral preserves §0: an agent's effective authority is its sealed profile intersected with
its live grants, attested not asserted — none of these may become a path to widen that.
