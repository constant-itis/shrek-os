# ADR-002 — Environment & execution vocabulary

**Status:** ✅ Accepted (2026-08-24). Settles the terminology before the Bench model
and the `shrek` CLI verbs encode it. **Work** is confirmed as a UI projection of
authoritative state, not an authority-owning object.
**Context:** Three overlapping drafts each used the words *Workshop* and *Workspace*
for different things, and the codebase never defined them:

- The **Software/Work/Agent Workflow Guidelines** (product-direction doc, 2026-08-24)
  define **Workshop** = a named, reproducible environment built from a declarative
  recipe (the promote-target of a Bench).
- The **workshops/workspaces brainstorm** (parked idea) defines **Workshop** = a
  human-owned *tool space* (the woodshop: tools, jigs, materials, safety rules,
  process) and adds **Workspace** = the human management plane *over* workshops.
- An older agent-identity note calls an **agent** "a workspace" (home dir +
  `CLAUDE.md` + agent-scoped memory = identity).

Left unreconciled, `shrek workshop …` would encode the wrong mental model and the UI
would name two different things "workspace." This ADR picks one meaning per word.

## Decision

Shrek OS has an **execution & placement model** for *where software lives and runs*,
and a separate human-facing **Work surface** for *how a person governs it*. The two
never share a noun. The objects do **not** form a linear ladder.

### The execution & placement model (relationship graph, not a ladder)

```text
Bench ──promote──▶ Workshop ──launch──▶ Job
                                           ▲
User Tool / Application ────────────────────┘

Workshop ──re-engineer (explicit trust change)──▶ Onion
Onions + base + UKI ─────────────────────────────▶ Deployment
```

A Job is **not** a more mature Workshop; a Deployment is **not** a promoted Job. These
are distinct object kinds with distinct relationships:

| Term | Kind | What it is |
|---|---|---|
| **User Tool** | persistent user process | a self-contained binary/script on `PATH` (`~/.local/bin`); needs no host libs, no privilege. Not everything needs a Bench. |
| **Application** | sandboxed app | a normal GUI/user app; packaging format is not the user's concern. Can launch Jobs. |
| **Bench** | **mutable** environment | the mess-with-a-door: `apt`/`pip`/experiment without touching sealed `/usr`. Classes: scratch, project, personal-dev, untrusted. Promotes to a Workshop. |
| **Workshop** | **reproducible** environment + authority template | a named environment from a declarative recipe (see §4). Human-authorized and curated; the promote-target of a Bench. Launches Jobs. May be re-engineered into an Onion (explicit trust-boundary change). |
| **Job** | ephemeral execution | a short-lived, outcome-oriented run launched from a Workshop or Application with task-specific grants; torn down after completion. |
| **Onion** | sealed / signed layer | a signed, dm-verity-protected sysext/confext for functionality that genuinely belongs in the composed OS. **Not** the default home for an ordinary user tool. |
| **Deployment** | sealed generation | a complete bootable Shrek generation (base + verity identity + UKI + compatible Onion set) with staged A/B activation and rollback (ADR-001). |

**The two "Workshop" drafts describe the same object.** The Guidelines' *recipe*
already contains `network policy + secret requests + persistence` — that **is** the
brainstorm's *authority envelope*. "Woodshop full of tools" and "declarative
reproducible recipe" are the tooling-side and authority-side views of one noun. There
is one Workshop.

**Bench → Workshop is the mess → recipe transition.** A Bench is the *pre-reproducible*
form; promotion inspects it, separates declared packages from incidental debris, and
emits a reviewable recipe (Guidelines core law #5: *promotion captures intent, not
filesystem debris*). Workshops accrete bottom-up from Bench work, then get named and
reified — but reification into a real authority boundary requires an explicit **human
promote** (propose-vs-promote; the build-provenance invariant holds).

### Tier is an orthogonal axis — do not bind it

Bench/Workshop/Job describe **lifecycle and reproducibility**. T0/T1/T2/T3 describe
**trust and execution constraint**. They are independent. A Bench or Job receives a
**policy-selected tier** and capability profile from its inputs, trust band, and
requested capability — it is *not* inherently "T1/T2" or "T2." `shrek run --trust=X
--caps=Y` is exactly the two-axis product (blast wall × blast radius).

### The Work surface (how a human governs it)

**Work** is the human-facing management surface for projects, Workshops, Benches, Jobs,
agents, and approvals. It is a **UI projection of authoritative state** — *not* an
execution environment and *not* an authority-owning object. The Quickshell component is
the **Work drawer** (today read-only; authority-mutation UX is gated on the trusted
path — see below).

There is deliberately **no** capitalized architectural object called "Workspace."
The word collides with Sway virtual workspaces, Herdr workspaces, IDE/project
workspaces, and general Linux desktop vocabulary. We do not give it a seventh meaning.
(A `shrek work …` verb, if any, would drive that UI projection — nothing is forbidden
here; there is simply no Workspace *object* to own state.)

### Killed collision: "agent as workspace"

The older "an agent is a workspace" phrasing is **retired**. That thing — home dir +
`CLAUDE.md` + agent-scoped memory — is the **agent identity slot** (a.k.a. *agent
home*): the identity-provisioning slot a Workshop hands a *dispatched* agent, alongside
its tools and caps. It collapses **inside** a Workshop. Agents own and govern
**nothing**; the entire ownership + management plane is human = the trusted path. An
agent is a constrained principal *dispatched into* a Workshop to operate its tools.

## §4 — What a Workshop recipe compiles to

A Workshop recipe is **not** merely a set of `shrek run` flags. It produces two related
artifacts, and a *runtime activation* compiles those into enforcement:

```text
Environment artifact                 Policy artifact
├── Base / rootfs identity           ├── Maximum filesystem requests
├── Packages and versions            ├── Network profile
├── Entrypoints / exported commands  ├── Secret-slot requests
└── Rebuild provenance               ├── Devices and resources
                                     └── Persistence policy

        └──────────── runtime activation ────────────┘
                            │
                   shrek run / Gatekeeper enforcement
```

The recipe **creates the actual tool environment** (the Environment artifact), not just
a description of command-line knobs.

## §5 — A Workshop requests authority; it does not grant it

A Workshop is an **authority template**. Gatekeeper remains the sole authority issuer.
Three distinct things must never be conflated:

- **Declared / requested maximum authority** — what the recipe asks for, e.g.
  ```yaml
  secrets:
    requests:
      - github-token
  ```
- **Human-approved activation authority** — what a human approves when installing or
  launching, ≤ the declared maximum.
- **Actual session authority** — what Gatekeeper enforces for a given run, ≤ the
  approved activation.

Installing a Workshop that *requests* `github-token` does **not** permanently grant the
token. `Declared maximum ⊇ approved activation ⊇ actual session authority` (sets of
permissions, not numeric levels), and mutation of any of these happens only on the
trusted path.

## Consequences

- `shrek` CLI verbs map to object kinds without overlap: `shrek bench …` (mutable),
  `shrek workshop …` (recipe → environment + policy artifacts), `shrek job …`
  (ephemeral run), `shrek deployment …` (generation). "Work" is the **UI** projection,
  not a state-owning object.
- **Workshop is where the authority *template* is declared** — the compile target for
  the existing capability model (capability profile = blast radius; trust band = blast
  wall; egress profile; brokered secret *requests*). Workshops are a declarative
  front-end to `shrek run`, not a parallel security system, and they issue nothing
  (`declared maximum ⊇ approved activation ⊇ actual session authority`).
- **Bench is net-new, mutable engineering** — distinct from the Onion/sysext plane,
  which is immutable-by-construction. Today's `shrek-dev` stays a *signed sysext
  toolchain Onion*, **not** retroactively a Bench. The mutable Bench plane (scratch
  `apt`/`pip`, per-project, promotable) is the next major build after INSTALL-0 and the
  hardest unbuilt piece.
- **User Tools carry provenance.** A managed installation retains source, version,
  digest, installed paths, exported commands, and removal provenance, and may run
  through a registered launcher/profile. Shrek still *allows* an unmanaged
  `curl | sh`, but it **labels the result unmanaged** rather than pretending nothing
  happened — no user-space archaeology under `~/.local/bin`.
- **Authority-mutation UX stays gated.** Any Work surface that *grants*, *stops*, or
  *promotes* is authority-mutation and must run on the **trusted path** (gatekeeper-
  owned console VT via SecureAttentionKey; graphical overlay is a later refinement —
  see glossary: Trusted path). Until that lands, the Work drawer is **read-only /
  observe-only**. This ADR names the surfaces; it does not unblock the trusted path.
- Glossary updated: User Tool, Bench, Workshop, Job, Deployment, Agent identity slot,
  Work (replacing the retired "Workspace").

## Open questions

- **`curl | sh` effect-preview** (Guidelines "installer-script mediation"): showing a
  script's intended writes from arbitrary shell is undecidable in general. Treat the
  effect-preview as best-effort; the robust path is *offer to run it in a Bench*
  (sandbox-and-observe), and *label unmanaged* whatever runs outside a managed install.
- Whether **Personal-Dev Bench** and a long-lived **project Workshop** should be one
  spectrum or two verbs (mutability is the real axis; naming may still confuse).
- **Applications** are acknowledged but not fully resolved: how they launch Jobs and
  how their promote/removal provenance aligns with the User Tool / Workshop stories.
