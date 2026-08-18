# Shrek OS — Architecture (revised)

> Ogres have layers.

This supersedes the original single-file roadmap. It folds in the base-platform decision
(bootc, not LFS-as-product), the graduated isolation model, and the two enforcement
refinements (swampd confinement, DLP-as-tripwire).

---

## 1. Layer stack

```
┌─────────────────────────────────────────────┐
│ Desktop / UX      Wayland · PipeWire · Portals│
├─────────────────────────────────────────────┤
│ Applications      Flatpak · OCI · trusted native│
├─────────────────────────────────────────────┤
│ AI / Agent layer  agentd · policies           │
├─────────────────────────────────────────────┤
│ Shrek control plane (Rust)                    │
│   swampd  · oniond · gatekeeperd · shrekctl   │
├─────────────────────────────────────────────┤
│ Writable state    /home · /var · /srv         │
├─────────────────────────────────────────────┤
│ The Onion         signed sysext layers        │
│   desktop · graphics · dev · gaming · ai      │
├─────────────────────────────────────────────┤
│ Immutable base    Debian → mkosi → bootc OCI  │
│   dm-verity sealed (composefs upgrade path)   │
├─────────────────────────────────────────────┤
│ Boot trust        Secure Boot(MOK) · UKI · TPM│
├─────────────────────────────────────────────┤
│ Linux kernel      LSM · Landlock · seccomp · BPF│
└─────────────────────────────────────────────┘
```

Everything from "immutable base" down is **borrowed and upstream-maintained**. Everything
in the "Shrek control plane" and "AI/agent" bands is **ours to build**. That boundary is
the whole point of the two-track decision.

## 2. The base — Debian + bootc, not LFS

Decision recorded in [`base-selection.md`](base-selection.md) (ADR-001, committed). Summary:

- **Provenance:** **Debian Stable** supplies the packages (glibc, systemd, kernel, mesa,
  security team, dependency graph). Debian is the *package source*, **not** the runtime
  package manager — the live base is immutable; `apt` lives only inside dev containers.
- **Builder:** **`mkosi`** assembles the image. (Speaks both Debian and Fedora, so a base
  swap is a `distro=` + package-list delta, not a rewrite — keeps Fedora a cheap escape
  hatch.)
- **Transport & updates:** ships as a **bootc OCI image**; transactional A/B with rollback.
  `systemd` is still PID 1 — no container wraps the OS. bootc is distro-independent
  (`ubuntu-bootc` proves the Debian-family path). *Fallback if bootc-on-Debian is janky:*
  `mkosi` + `systemd-sysupdate` raw A/B.
- **Integrity:** **dm-verity** sealed root (via `systemd-repart`) — proven, turnkey on
  Debian today. **composefs** (whole-tree, content-addressed) is a later upgrade, not a
  blocker; it's upstream, so Debian can adopt it.
- **Boot chain:** `UKI (systemd-stub, Shrek key) → sealed root → verified sysext layers`,
  with **MOK enrollment** for Secure Boot now (shim-review only if Shrek is publicly
  distributed later — see base-selection.md). **Custom-compiling the kernel is a late,
  expensive, opt-in decision** — forking it risks stepping off the signed chain.
- **MAC / agent wall:** **Landlock-first** (kernel-native, distro-agnostic — the *real*
  wall, enforced per-process by `agentd`/`gatekeeperd`) + **AppArmor** as the system MAC.
  AppArmor's path model fits the deterministic wall (`deny ~/Vault/**`) more naturally than
  SELinux labels.

LFS lives on only in `reference-lfs/` as a frozen teaching build.

## 3. The Onion — layers via sysext, orchestrated not reimplemented

`systemd-sysext` already does exactly what "the Onion" needs: dynamically extend `/usr`
and `/opt` from stacked, read-only, **Verity-authenticated** raw images.
`systemd-confext` handles the `/etc` slice. `systemd-repart` generates and signs the
Verity data.

So **`oniond` implements no layering.** It is the *policy/orchestration* layer:

```
oniond answers:
  "Which Shrek layers belong on this machine?"
  "Which versions are compatible?"
  "Is this layer signed by a trusted authority?"
  "May this user activate it?"
  "Which layer caused this boot failure? — roll it back."

systemd-sysext / dm-verity / mount / kernel VFS   do the dangerous low-level work.
```

### Delivery routing rule (decide once, never relitigate)

Three mechanisms can put software on the machine. Each has one job:

```
base + security-critical + boot-path      → baked into the bootc IMAGE
optional composable SYSTEM capability      → sysext LAYER   (drivers, toolchains, dev tools)
   (/usr,/opt; /etc via confext)
user-facing APPLICATION                    → FLATPAK
```

`oniond` owns only the middle row.

## 4. Graduated isolation — trust × capability are TWO dials

Agents and ephemeral workloads don't need full QEMU machine emulation. We pick isolation
by **trust tier**, and — critically — the trust tier is *orthogonal* to the **capability
profile**. They answer different questions and are enforced at different points:

- **Trust tier** = *how strong is the blast wall around the workload?*
- **Capability profile** = *what is inside the blast radius?* (which paths mount in, which
  network egress is allowed)

A microVM is a sealed box; it does **not** enforce "read Projects but not `~/.ssh`." That
policy lives entirely in **what you mount in (virtio-fs) and what network you attach.** A
hardware-isolated agent with `$HOME` mounted in has bought you nothing. So both dials are
always set independently.

```
shrek run --trust=<tier> --caps=<profile> ./thing
           └── blast wall ──┘  └── blast radius ──┘
             (agentd picks)     (gatekeeperd builds mounts+net)
```

### The tiers

```
Tier 0 — Process sandbox        Landlock + seccomp + namespaces + cgroups
  trusted apps, small tools · overhead: tiny · shared kernel

Tier 1 — System container       systemd-nspawn / LXC / Incus
  dev/build envs, services, trusted agents · overhead: very low · shared kernel

Tier 2 — Userspace kernel       gVisor                             ← ADDED
  untrusted code, container-grade startup, NO hardware-virt tax
  syscalls serviced in userspace; guest never touches host syscall surface

Tier 3 — MicroVM                libkrun / Firecracker / Kata
  untrusted agents, downloaded/AI-generated code, unknown plugins
  own kernel · KVM hardware-virt boundary · ~5 MiB VMM overhead (Firecracker)
```

**gVisor (Tier 2) is the workhorse, not an afterthought.** The original design jumped
straight from shared-kernel containers to hardware-virt microVMs and skipped the tier that
gives untrusted-grade isolation with container-grade startup and no separate rootfs to
manage. Reach for a microVM only when you need the actual VT-x wall (kernel-exploit-class
untrusted, or multi-tenant).

`libkrun` is the preferred Tier-3 VMM (it's what `podman --runtime krun` uses — real,
shipping) because it softens the microVM cold-start/rootfs tax that raw Firecracker leaves
to you.

### What the one-liner hides

~80% of the engineering behind `shrek run` is *not* choosing the VMM. It's:

- **virtio-fs/9p file passing** — also the capability-enforcement point (§4 above).
- **networking** — each microVM needs a tap device + the "restricted network" realized as
  actual nftables rules.
- **rootfs supply** — Tier 3 needs a tiny signed kernel + minimal rootfs per workload
  class. (Synergy: the Onion/build-system feeds this.)
- **cold start / pooling** — pre-warm a pool for ephemeral per-task agents.

Example intent → mechanism:

```
"summarize my notes"                                  → Tier 0
"compile Shrek's kernel"                              → Tier 1
"clone this random repo and run its tests"            → Tier 2 (gVisor)
"autonomous agent installs deps + runs generated code"→ Tier 3 (microVM), definitely
```

## 5. The Swamp — filesystem intelligence, and the confused-deputy problem

`swampd` observes normal filesystem events (coalesced), builds a metadata index (SQLite,
hashes, MIME, FTS, provenance), and *optionally* a semantic index (embeddings,
relationships, vector retrieval). The filesystem stays authoritative. Search escalates
through tiers — filename/metadata → FTS → semantic → LLM — and **most queries never reach
an LLM.**

### swampd is the most dangerous process in the system

To build an index, `swampd` reads everything. Anything that can query it can therefore
potentially reach what it read — **the index is a side channel around the file wall.**
Guarding `~/Vault/foo.pdf` at the VFS is useless if `swampd` indexed it and an agent can
query the embedding or the extracted summary.

Therefore the invariant `semantic authority ≤ data authority` is enforced **structurally,
by making swampd a *subject* of the deterministic policy, not an exception to it:**

```
Landlock the swampd daemon itself so it CANNOT open the human-only domains.
Not "swampd is trusted to skip ~/Vault" — swampd is physically incapable of reading it.

Consequence: a swampd compromise leaks NOTHING from those domains,
because the protected bytes never entered its address space.
```

This is stronger than a "don't index these paths" config (which can be misconfigured):
swampd's own read scope == the union of what it is allowed to expose, enforced by the
kernel on the daemon. Enforce authorization **before** retrieval — never global-search →
retrieve-secret → filter-after.

## 6. AI capability security — vocabulary beyond rwx

Semantic/AI permissions extend Unix `rwx`:

```
discover · read · write · execute · index · embed · search · summarize · relate · export · network
```

`discover: false` means the agent must not even learn protected material *exists*. Example
agent profile (enforced by kernel primitives via `agentd`, not by prompt):

```yaml
agent: coder
read:    [ ~/Projects/foo/** ]
write:   [ ~/Projects/foo/** ]
network: [ github.com, crates.io ]
commands:[ cargo, git, cmake, ninja ]
denied:  [ ~/.ssh/**, ~/.gnupg/**, ~/Private/**, /boot/**, /etc/**, /usr/** ]
```

## 7. Two-layer security model — deterministic wall + semantic tripwire

Never reverse these:

```
DETERMINISTIC SECURITY  — the WALL
  "Agent can NEVER read this."
  Landlock / SELinux / namespaces / mounts   →  HARD, kernel-enforced
    ~/Identity/**   agents = DENY
    ~/Vault/**      agents = DENY
    ~/.ssh/**       agents = DENY

SEMANTIC SECURITY       — the TRIPWIRE
  "Agent may normally read this, but this content looks risky."
  classifier / rules / DLP                   →  advisory; requires human approval
    upload ~/Projects/report.txt → file access allowed → DLP: "looks like payroll/PII"
      → require human approval
```

Semantic DLP must **never** be the wall. It must remain deterministic enough that critical
restrictions do not depend on an LLM judgment. A classifier false negative = a warning
failed; it must **not** = a secret leaked.

## 8. Provenance

Every agent action is auditable: `artifact`, `previous_hash → new_hash`, `actor`,
`model`, `operation`, `reason`, `capabilities`, `network`, `timestamp`. Surfaced via
`shrek history <path>` and `shrek audit --agent <id>`, including "Protected data accessed:
NO / Secrets accessed: NO / External network: <hosts>".

## 9. Critical failure test (unchanged, load-bearing)

```
systemctl stop swampd
systemctl stop agentd
```

Boot, login, desktop, filesystem, networking, applications, layers, shell, and dev work
must **all still function.** Only enhanced capabilities (semantic search, relationships,
AI assistants, provenance enrichment, auto-embeddings) disappear. This is what prevents
Shrek from becoming an AI appliance pretending to be an OS.

## 10. Development priority (when forced to choose)

```
correctness → security → recoverability → performance → architecture cleanliness → AI features → cosmetics
```

AI is deliberately late. The system must be worth using before the first model loads.
