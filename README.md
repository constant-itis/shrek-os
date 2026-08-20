# Shrek OS

> Ogres have layers.

A memory-safe, immutable, capability-oriented Linux system built around verified OS
layers, explicit human/agent trust boundaries, semantic filesystem intelligence, and
transactional system updates.

Status: **in-progress implementation.** The repo holds the architecture docs, image build
spikes, and the first Rust control-plane slices.

---

## What Shrek actually is

Strip the theme and the novel part of this project is **not** a distro. Anyone can make a
green Fedora spin. The interesting systems-research question is:

> How does an operating system let autonomous semantic software understand and manipulate
> a user's computer, while ensuring that understanding itself never becomes a
> privilege-escalation mechanism?

The single most important invariant we've identified:

```
semantic authority <= underlying data authority
```

Everything else (immutable base, transactional updates, the layer system) is plumbing we
should **borrow, not build**. The parts worth building are `swampd`, `agentd`,
`gatekeeperd`, and the capability/ACL model that connects them.

## The two-track decision

We do **not** hand-maintain a from-scratch distribution as a product dependency. LFS is
demoted from *foundation* to *laboratory*.

|                   | **Shrek Research Base**        | **Shrek OS (product)**                 |
| ----------------- | ------------------------------ | -------------------------------------- |
| Purpose           | understand the whole stack     | an actually usable, distributable OS   |
| Foundation        | LFS (frozen reference build)   | **Debian Stable** provenance, imaged via **mkosi** |
| Updates           | experimental                   | **systemd-sysupdate** raw A/B (bootc deferred) |
| Layers            | sysext experiments             | signed sysext ("the Onion")            |
| Integrity         | hand-rolled dm-verity          | **dm-verity** sealed (composefs upgrade path) |
| MAC / agent wall  | —                              | **Landlock**-first + AppArmor          |
| Audience          | us                             | real machines / users                  |
| Maintenance       | frozen; port *lessons*, not code | sustainable (Debian owns the substrate) |

The LFS build is a **frozen artifact**: built once, understood, documented, pinned. It is
NOT a living parallel distro and NOT in production CI. If it teaches us something, we port
the lesson to the product image — never the code. This preserves the reason we liked LFS
(bottom-up understanding of what Shrek stands on) without making Sebastian the personal
security-advisory-response-team and package repository for a full distribution forever.

See [`docs/base-selection.md`](docs/base-selection.md) (ADR-001) for why Debian
via `mkosi` won, and [`docs/update-model.md`](docs/update-model.md) for why the
current transport is `systemd-sysupdate` raw A/B with bootc deferred.

## Repository layout

```
shrek-os/
├── image/              ← mkosi image definition + sealed-root overlay
├── layers/             ← sysext/confext Onion fixtures used by the proofs
├── crates/
│   ├── shrekctl/       ← system CLI
│   ├── oniond/         ← layer policy/orchestration around systemd-sysext
│   ├── swampd/         ← filesystem intelligence daemon (metadata → semantic)
│   ├── gatekeeperd/    ← privileged capability broker
│   ├── agentd/         ← agent identity + tier/caps resolver
│   ├── shrek-gate-probe/ ← small probe binary used by proof scripts
│   └── shrek-policy/   ← sealed compiled-in tier + egress policy
├── scripts/            ← build, VM, rollback, Onion, and sandbox proof gates
└── docs/
    ├── architecture.md ← revised core architecture + invariants
    ├── concept-to-code.md ← implementation/proof crosswalk for contributors
    ├── base-selection.md ← ADR-001: chosen immutable base
    └── roadmap.md      ← phased build order
```

## Core invariants (never violate)

1. **Linux remains Linux.** Don't rewrite mature components. `oniond` orchestrates
   `systemd-sysext`; it does not implement layering.
2. **The filesystem stays authoritative.** The database is an intelligence/control plane,
   never storage ground truth.
3. **AI is never in the filesystem fast path.** Kill `swampd` and `agentd` and a fully
   functional Linux system remains.
4. **Semantic authority ≤ data authority.** If a subject can't `read` a file, it cannot
   discover, embed, search, summarize, or infer it. Authorize *before* retrieval.
5. **The wall is deterministic; semantic DLP is only a tripwire.** A classifier false
   negative means a warning failed — never "the agent now has your SSH keys."
6. **Agents get capabilities, not root.** The kernel enforces boundaries, not the prompt.

## Guiding principle

> Shrek OS should not give AI more authority than the user intended merely because AI can
> understand more of the machine. The more semantically aware the OS becomes, the stronger
> its information boundaries must become.
