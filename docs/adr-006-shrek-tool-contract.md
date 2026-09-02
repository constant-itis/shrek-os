# The Shrek Tool Contract — the single authority-bearing surface (ADR-006 refinement)

Status: DESIGN NOTE (owner-ratified 2026-09-02). Input for the `shrek ai` front-door slice; not built.
Parent: docs/adr-006-optional-ai-layer.md §6/§7. Reaffirms §3 (slinkd absent by default on a Shrek box).

## 1. The property this protects

> **Removing Slinkd must not make ShrekOS less functional as a personal computer.**

A stock Shrek box — Granite 4.2 3B + on-box Mycelium + Bench/services — must locally understand
"install VLC", "why was this network request denied?", "find yesterday's PDF", "give this Workshop
access to Documents", "what's using my battery?", "remember I prefer dark mode". None of that may
depend on an optional coordination bus.

## 2. The chain (shipped local box)

```
User → Shrek AI front door → Granite 4.2 3B  ⇄  Mycelium retrieval
                                   │
                          typed tool proposal
                                   │
                     Bench + ShrekOS service APIs
                                   │
                       gatekeeperd / policy / consent
                                   │
                                 effect
```

The model is an **untrusted intent/tool-call proposer**. It emits a *typed proposal* against the Tool
Contract; it holds no credential authority and has **no host-exec surface** (§6). `{"tool":"network.grant"}`
is a request, not an effect — Bench executes and gatekeeperd/consent decides. Occasional model
stupidity degrades to "invalid proposal rejected", never "machine altered".

## 3. One chokepoint, every principal

Optional Slinkd (configured only when the user wants agent harnesses / multi-agent / remote
coordination / long-running workflows) speaks the **same** Tool Contract — it does **not** become a
second tool API and gets **no** privileged path around Bench/gatekeeperd:

```
Granite ───┐
           │
Slinkd ────┼──→  Shrek Tool Contract  →  Bench/services  →  policy/consent  →  effect
           │
future ────┘
agents
```

**One schema. One audit vocabulary. One authority model.** Consequences: the model is swappable
without redesigning the OS (each verb is self-describing via its schema); every actor is audited
through the same vocabulary; and the authority boundary is enforced in exactly one place.

## 4. The M1 verb vocabulary (candidate — the front-door slice finalizes it)

Generic, capability-shaped verbs; the model learns each from its JSON schema, Bench/services map it to
the substrate, gatekeeperd decides legality, Mycelium supplies user context:

| Verb | Maps to |
|------|---------|
| `system.status` / `system.explain_denial` | desktopd / gatekeeperd audit record ("why was X denied?") |
| `apps.search` / `apps.install` / `apps.launch` | the application plane (ADR-003 Onions) |
| `files.search` | indexed file query (read-only) |
| `settings.read` / `settings.change` | settings surface (change → consent) |
| `network.status` | egress/nft state (read-only) |
| `workshop.create` | Bench/Workshop provisioning (→ consent ceremony) |
| `memory.search` / `memory.remember` | the on-box Mycelium brain |

Naming rule: it is the **Shrek Tool Contract**, never "Slinkd tools" — the abstraction is load-bearing
and outlives any one client. Authority-raising verbs (grants, installs, settings writes) route to the
§7 consent/ceremony path; read-only verbs do not.

## 5. Dispatch tiers — and why Bench is not sudo (owner refinement 2026-09-02)

The Tool Contract is not one executor — it **dispatches each verb to the narrow service that owns that
capability**. The model chooses **intent** (which verb); it never chooses the execution path or the
privilege mechanism. The OS decides which of three tiers carries the intent:

1. **Read-only service verbs** (`system.status`, `system.explain_denial`, `files.search`,
   `network.status`, `memory.search`) — the owning service answers; no privilege, no side effect.
2. **Trusted OS operations** (`apps.install`/Onion install, authenticated model-data update,
   `settings.change`) — handled by the **narrow, signed-input service that owns the capability**
   (oniond/gatekeeperd for Onions; the model-as-data verify+deliver path — `shrek-ai-model-verify` — for
   model updates), operating **only on signed / digest-verified inputs**, with the §7 consent ceremony
   where authority-raising. **These do NOT go through Bench.**
3. **Arbitrary / untrusted code + dependency execution** ("run this workload", pull arbitrary crates) —
   **Bench** + the ADR-003 egress plane: sandboxed, empty-net by default, per-invocation consent for any
   egress.

**Bench only ever runs untrusted compute.** If trusted OS operations were "routed through Bench", Bench
would need OS privilege and would become a general escalation path — sudo with extra steps. Keeping
privileged ops in purpose-built signed-input services keeps Bench a pure sandbox, and keeps the model
choosing *intent* while the OS owns *mechanism and privilege*. The AI's own runtime deps are sealed in
the Onion (no broker, no egress); only its *proposed actions* dispatch through these tiers.
