# Shrek OS — glossary

## Agent

An attested subject with `(identity, profile, trust_band)`. The profile is the default-deny capability allow-set; the trust band sets the isolation floor. See [`agents.md`](agents.md).

## Agent-capability model

The rule that an agent receives precise capabilities, not root or a user account. Effective authority is the sealed profile intersected with live grants. The kernel and broker enforce the result.

## Agent legibility

The design property that Shrek's durable state is inspectable as boring, human-readable files rather than opaque daemon/D-Bus/SQLite internals — so a human *or* a coding agent can `cat`/`grep`/`diff` the truth instead of reconstructing it from many abstraction layers. Canonical invariant: every durable Shrek fact has a filesystem-visible canonical representation; a daemon may own the write path for atomicity, but is never the sole source of durable truth, and `shrek … show` is always a pretty-printer of the record, never a divergent computed view. *Debian underneath, file-legible Shrek state above* — not an argument to change the base distro. See [`adr-004-file-legible-canonical-state.md`](adr-004-file-legible-canonical-state.md).

## Agentd

The unprivileged resolver daemon. It binds identity, checks `caps ⊆ profile`, computes the requested tier from sealed policy, and emits a request for `gatekeeperd` to re-check.

## Agent identity slot

The identity-provisioning slot a Workshop hands a dispatched agent: home dir, `CLAUDE.md`, agent-scoped memory. It collapses **inside** a Workshop and is never called a "workspace" (see [`adr-002-environment-vocabulary.md`](adr-002-environment-vocabulary.md)).

## Bench

A **mutable** environment — the mess-with-a-door where you `apt`/`pip`/experiment without touching sealed `/usr`. Classes: scratch, project, personal-dev, untrusted. Instantiated at a **policy-selected tier** (trust is an orthogonal axis, not fixed) with a persistence policy and a default-deny capability profile. Promotes to a Workshop. See [`adr-002-environment-vocabulary.md`](adr-002-environment-vocabulary.md).

## Blast radius

What the workload can reach inside its box: paths, commands, search scope, export scope, and network egress. This is the capability profile.

## Blast wall

How strong the containment boundary is: T0, T1, T2, or T3.

## Capability profile

A default-deny declaration of what a subject may do. It uses verbs such as `discover`, `read`, `write`, `execute`, `search`, `summarize`, `export`, and `network`.

## Constructor

The mechanism that realizes an effective tier: `proc_plane` for T0, nspawn/mount/net planes for T1, `t2_plane` for T2, and a later microVM constructor for T3.

## Data authority

The bytes and metadata a subject can reach through deterministic access control.

## Deployment

A complete bootable Shrek generation: base image + verity identity + UKI + a compatible Onion set, activated via staged `systemd-sysupdate` raw A/B with rollback (see [`adr-001` base-selection](base-selection.md), [`update-model.md`](update-model.md)).

## Gatekeeperd

The privileged broker and wall. It independently re-checks requests against sealed policy, constructs sandboxes, brokers Onion merges, and must fail closed for agent execution.

## Grant

A bounded authority extension approved through the grant protocol. The agent never receives a bearer token; the broker applies the capability directly.

## Job

A short-lived, outcome-oriented execution launched from a Workshop or Application with task-specific grants; an ephemeral sandbox instantiated at the **policy-selected tier** with one-shot grants, torn down after completion. See [`adr-002-environment-vocabulary.md`](adr-002-environment-vocabulary.md).

## Onion

The signed system-layer mechanism: signed, dm-verity-protected sysext/confext DDIs selected by sealed policy and merged by systemd under a fixed image policy. Reserved for functionality that genuinely belongs in the composed OS — **not** the default home for an ordinary user tool.

## Plane

A separated enforcement surface. The main distinction is availability plane vs agent-execution plane: the OS should keep booting when semantic services are down, but agent execution must fail closed without the broker.

## Semantic authority

What a subject can learn through search, embeddings, summaries, relationships, and generated context. In Shrek it must never exceed data authority.

## Semantic filesystem

The metadata and semantic projection over the real filesystem, built by `swampd`. It is not storage ground truth.

## Swampd

The filesystem intelligence daemon. It indexes only what its own Landlock allow-set lets it read, and query results are caller-scoped.

## T0

Process sandbox: Landlock, seccomp, full namespace set, cgroups v2, no rootfs. Shipped for the T0 matrix cells.

## T1

System container: `systemd-nspawn` with a synthetic root, private users, private network, read-only relocated grants, and optional sealed-profile egress.

## T2

Userspace-kernel sandbox using gVisor `runsc`. Shipped for no-network cells; T2 egress is deferred.

## T3

MicroVM sandbox, planned around libkrun/Firecracker/Kata. Designed, not shipped.

## Tainted-origin action

An action triggered by untrusted content the agent read. It cannot silently self-serve, even if the action is otherwise in profile; it is demoted to a proposal or explicit acknowledgement path.

## Trusted path

The human approval surface that the sandbox cannot spoof or capture. The design floor is a gatekeeper-owned console VT triggered through systemd `SecureAttentionKey`; graphical overlay is a later shell refinement.

## Trust band

The provenance band of code being executed: `T-first`, `T-pinned`, `T-untrust`, `T-hostile`. The broker derives it from sealed evidence; unknown provenance fails high.

## User Tool

A self-contained binary/script that runs as a user process (`~/.local/bin`, on `PATH`), optionally through a registered launcher/profile; needs no host libraries and no privilege. Not everything needs a Bench. A **managed** installation retains source, version, digest, installed paths, exported commands, and removal provenance; an unmanaged `curl | sh` result is allowed but **labelled unmanaged**. See [`adr-002-environment-vocabulary.md`](adr-002-environment-vocabulary.md).

## Work

The human-facing management surface for projects, Workshops, Benches, Jobs, agents, and approvals. It is a **UI projection of authoritative state** — not an execution environment and not an authority-owning object. The Quickshell component is the **Work drawer** (read-only until the trusted path lands). Deliberately **not** named "Workspace" (that word collides with Sway/Herdr/IDE/project workspaces). See [`adr-002-environment-vocabulary.md`](adr-002-environment-vocabulary.md).

## Workshop

A named, **reproducible** environment built from a declarative recipe that produces an *environment artifact* (base, packages/versions, exports, rebuild provenance) and a *policy artifact* (max filesystem/network/secret-slot requests, devices, persistence). An **authority template**, not an issuer — a runtime activation compiles it into `shrek run`/Gatekeeper enforcement (`declared maximum ⊇ approved activation ⊇ actual session authority`). Human-authorized and curated; the promote-target of a Bench; may be re-engineered into an Onion via an explicit trust-boundary change. See [`adr-002-environment-vocabulary.md`](adr-002-environment-vocabulary.md).
