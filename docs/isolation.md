# Shrek OS — Isolation model (implementation-ready)

> Ogres have layers. So do sandboxes.

This is the implementation spec behind `shrek run --trust=<tier> --caps=<profile>`. Where
[`architecture.md`](architecture.md) §4 *introduces* the four tiers and the two-dial idea,
this document makes them buildable: it fixes the tier definitions, the deterministic
`(trust × caps) → tier` selection matrix, the escalation rule, and the `agentd`/
`gatekeeperd` contract that Phase 5 and Phase 8 are built against.

**Base-agnostic by construction.** Nothing here depends on Debian vs Fedora. Every
primitive named (Landlock, seccomp, namespaces, cgroups v2, gVisor, KVM, virtio-fs,
nftables) is a kernel or userspace-runtime feature, not a distro feature. That is the point:
the isolation runtime is part of the base-agnostic 85% (the control plane), so it never
moves if the base swaps.

---

## 1. Scope & non-goals

**This document owns mechanism selection and construction:** given a trust level and a
capability profile, which isolation *tier* runs the workload, and how the sandbox is
physically assembled (mounts, network, rootfs).

**It does not own policy authoring.** *Which* capabilities a given agent identity is allowed
to request lives in [`agents.md`](agents.md) and [`security-model.md`](security-model.md).
This document assumes a resolved capability profile arrives as input and enforces it; it does
not decide whether `agent: coder` should have `network: [github.com]`.

Restated invariant this doc must never violate:

```
semantic authority ≤ data authority        (system-wide, architecture.md)
   ⇒ here:  a sandbox's blast radius ≤ the requesting agent's granted profile
            and the isolation tier NEVER widens what the profile allows.
```

## 2. The two dials, precisely

Two independent decisions. They answer different questions, are enforced at different points,
by different daemons.

```
shrek run --trust=<tier> --caps=<profile> ./thing
           └── blast WALL ──┘  └── blast RADIUS ──┘
             how strong is       what is reachable
             the box?            inside the box?
           chosen by agentd    built by gatekeeperd
           (§3 tiers, §5 matrix) (§6 construction)
```

- **Trust tier** = the strength of the containment *wall*. It answers: *if the code inside is
  fully hostile and exploits everything it can reach, what stops it from reaching the host?*
  Ranges from "shared kernel, syscall-filtered" (T0) to "separate kernel behind a hardware
  virtualization boundary" (T3).

- **Capability profile** = the *radius* inside that wall. It answers: *which filesystem paths
  are mounted in, and what network egress exists?* This is realized entirely as **what
  gatekeeperd mounts (virtio-fs/bind) and what network it attaches (tap + nftables)** — never
  as a property of the tier.

### The load-bearing claim

> **A microVM with `$HOME` mounted in buys you nothing.**

A Tier-3 hardware-isolated box that has `~/.ssh` bind-mounted read-write has spent maximum
isolation budget to protect *the host kernel* while handing the workload every secret it
wanted. The wall was strong; the radius was catastrophic. Therefore:

**The tier NEVER relaxes the caps.** Caps are enforced identically at every tier — the same
mount-set and the same nftables egress rules are applied whether the workload runs at T0 or
T3. A stronger tier changes *the consequence of a guest escape*, not *what the guest is
handed on a plate.* The two dials are set, and enforced, independently. Always.

## 3. Tier reference

Each tier is specified against a fixed schema so they are directly comparable. "Escape" below
means the guest workload is fully compromised and actively trying to break out.

### Tier 0 — Process sandbox

| | |
|---|---|
| **Mechanism** | Landlock + seccomp-bpf + user/mount/pid/net namespaces + cgroups v2 |
| **Boundary** | Shared host kernel; access restricted per-process by LSM + syscall filter |
| **Startup** | Effectively free (fork/exec + ruleset install), sub-millisecond |
| **Rootfs/kernel supply** | None — runs against host `/usr` (read-only) + mounted caps |
| **On escape** | A kernel LPE defeats it — shared kernel is the whole attack surface |
| **agentd selects when** | First-party/signed code; small tools; the workload is *trusted* and the sandbox is defence-in-depth, not a containment bet |
| **Wrong choice when** | Code provenance is unknown or hostile — a single kernel bug is game over |

Landlock is the **real agent wall** here and everywhere: it is kernel-native, unprivileged,
and distro-agnostic, so the *same* ruleset (`deny ~/Vault/**`) is portable across any base.
seccomp narrows the syscall surface; namespaces + cgroups bound resources and visibility.

### Tier 1 — System container

| | |
|---|---|
| **Mechanism** | systemd-nspawn / LXC / Incus (shared-kernel OS containers) |
| **Boundary** | Shared host kernel; full namespace set + optional per-container Landlock/AppArmor |
| **Startup** | Very low (hundreds of ms); no VM boot |
| **Rootfs/kernel supply** | A container root tree (may be a Shrek sysext-derived image); host kernel |
| **On escape** | Same shared-kernel exposure as T0, but a fuller userland to abuse |
| **agentd selects when** | Dev/build environments, long-lived trusted services, trusted multi-process agents that need a real init + userland |
| **Wrong choice when** | Running untrusted or downloaded code — still one kernel bug from the host |

T1 is T0 with a full OS userland and lifecycle. It raises *convenience and completeness*, not
the *containment strength* — the wall is still the shared kernel.

### Tier 2 — Userspace kernel (gVisor) — **the workhorse**

| | |
|---|---|
| **Mechanism** | gVisor: a userspace kernel (Sentry) services guest syscalls; host syscall surface is drastically reduced and itself seccomp-confined |
| **Boundary** | Guest **never touches the host syscall table directly** — Sentry intercepts and reimplements it in userspace |
| **Startup** | Container-grade (tens of ms) — no hardware-virt boot, no separate guest kernel image |
| **Rootfs/kernel supply** | Container rootfs; **no guest kernel to build/sign** (gVisor *is* the kernel) |
| **On escape** | Must break out of Sentry's userspace kernel *and* its host-side seccomp jail — a far smaller, hardened surface than the full host syscall table |
| **agentd selects when** | Untrusted code that needs container-grade startup: "clone this random repo and run its tests," ephemeral per-task workers, anything unknown-but-not-adversarially-targeted |
| **Wrong choice when** | You genuinely need the VT-x hardware wall (kernel-exploit-class adversary, or multi-tenant hostile) — then T3 |

**Tier 2 is the default answer for "untrusted, but I need it fast," and the original design's
biggest gap.** The naïve model jumps straight from shared-kernel containers (T0/T1) to
hardware-virt microVMs (T3) and skips the tier that gives *untrusted-grade isolation with
container-grade startup and no separate rootfs/kernel to manage.* gVisor fills exactly that
gap. Most untrusted workloads should land here, not at T3.

### Tier 3 — MicroVM

| | |
|---|---|
| **Mechanism** | libkrun *(preferred)* / Firecracker / Kata — a real guest kernel behind a KVM boundary |
| **Boundary** | Hardware virtualization (VT-x/AMD-V); separate guest kernel; host exposure is the VMM + KVM only |
| **Startup** | Slowest: guest kernel boot + rootfs mount (~100 ms class; VMM overhead ~5 MiB for Firecracker) |
| **Rootfs/kernel supply** | Needs a **signed minimal kernel + minimal rootfs per workload class** — real supply-chain work (see §6) |
| **On escape** | Must defeat the guest kernel *and then* escape the VMM/KVM boundary — the strongest wall we offer |
| **agentd selects when** | Untrusted **and** dangerous: autonomous agents that install deps and run generated code, downloaded/AI-generated binaries, unknown plugins, multi-tenant hostile |
| **Wrong choice when** | The workload is merely untrusted-but-fast — you're paying boot + rootfs cost for a wall T2 already provides |

**`libkrun` is the preferred VMM** (it backs `podman --runtime krun` — real, shipping) because
it softens the microVM cold-start/rootfs tax that raw Firecracker leaves entirely to you.
Firecracker/Kata remain options for stricter or multi-tenant profiles.

### Tier summary

```
T0  Landlock+seccomp+ns+cgroups   trusted apps/tools        shared kernel   ~0 startup
T1  nspawn/LXC/Incus              dev/build/trusted svcs     shared kernel   low startup
T2  gVisor (userspace kernel)     UNTRUSTED, fast    ← default  no host syscalls  container startup
T3  libkrun/Firecracker/Kata      UNTRUSTED + dangerous      KVM hw wall     VM startup
```

## 4. The trust axis and the caps axis, defined

The matrix in §5 is indexed by two axes. Both must be discretized to be a lookup table.

### Trust axis — provenance of the *code being executed*

Trust is about the **code**, never about the data it touches (that's caps). Four bands,
increasing risk:

```
T-first    First-party or Shrek-signed code. Provenance verified, authored/audited by us.
T-pinned   Third-party but pinned & vetted: a known dependency at a known hash (distro
           package, pinned crate/toolchain). Trusted supply chain, not our code.
T-untrust  Untrusted: cloned repos, downloaded scripts/binaries, anything off the internet
           whose behavior we have not vetted.
T-hostile  Adversarial-by-assumption: AI-generated code, autonomous-agent-authored code,
           unknown plugins, or anything we must assume is actively trying to escape.
```

### Caps axis — what is inside the blast radius

Caps is about the **mount-set and egress**, discretized by the *worst* thing reachable:

```
C-ro-nosec   Read-only, NO secret domains, NO network. (e.g. summarize provided notes)
C-proj-rw    Read-write to a single project scope; NO secrets; NO network.
C-net        Any of the above PLUS network egress (even if allow-listed).
C-broad      Broad $HOME visibility, or any secret-domain path, or unrestricted egress.
             (This is the profile that must never be granted to low-trust code — see §6.)
```

`C-broad` is deliberately the "danger" column: it is where a compromise hurts most, so the
matrix pushes it to the strongest walls and the floor rule (§5.1) forbids low-trust code from
combining it with a weak tier.

## 5. The selection matrix — the core deliverable

`agentd` resolves `(trust, caps) → tier` **deterministically** from this table. No LLM
judgment, no per-run vibes — the wall selection is as deterministic as the wall itself
(consistent with architecture.md §7: the wall is never an LLM decision).

```
              C-ro-nosec     C-proj-rw      C-net          C-broad
            ┌──────────────┬──────────────┬──────────────┬──────────────┐
 T-first    │ T0           │ T0           │ T1           │ T1           │
 T-pinned   │ T0           │ T1           │ T1           │ T2           │
 T-untrust  │ T2           │ T2           │ T2           │ T3           │
 T-hostile  │ T2           │ T3           │ T3           │ T3           │
            └──────────────┴──────────────┴──────────────┴──────────────┘
              ← weaker caps                          stronger caps →
```

Reading the design of the table:

- **Trust dominates.** Moving *down* a row (less trustworthy code) never lowers the tier and
  usually raises it. The row a workload sits in sets its **floor** (§5.1).
- **Caps modulate within a trust band.** More reachable/dangerous caps push right → stronger
  wall, because a compromise now has more to steal or a way out (network).
- **`T-hostile` + anything that writes or has egress → T3.** AI-generated/autonomous code that
  can write files *or* reach the network is always a microVM. The one concession: `T-hostile`
  + `C-ro-nosec` (read-only, no secrets, no net — e.g. "let a generated parser read this one
  file and print a result") may run at **T2**, because with nothing writable, no secrets, and
  no egress, gVisor's userspace-kernel wall is sufficient and the microVM tax buys little.
- **`T-untrust` + `C-broad` → T3, not T2.** Broad `$HOME`/secret exposure is exactly the case
  where you want the hardware wall even for merely-untrusted code.

### 5.1 The floor rule — hard, downward-forbidden

The row's tier is a **minimum, enforced by agentd and re-checked by gatekeeperd:**

```
floor(trust) =  T-first → T0    T-pinned → T0    T-untrust → T2    T-hostile → T2
                (the leftmost/weakest tier that trust band may EVER run at)
```

- A workload may **never** run below its trust floor, regardless of how narrow its caps look.
  Untrusted code with tiny caps is still untrusted code; a kernel bug still ends T0/T1.
- The floor is a property of **provenance**, which caps cannot buy back. Narrow caps reduce
  *what's stolen on escape*; they do not reduce *the probability of escape* — only the wall
  does. So caps can never lower the wall.

### 5.2 Escalation — upward only

*(Per the locked design decision: matrix as default, explicit escalation, no downward
overrides.)*

- A **policy or operator may request a STRONGER tier** than the matrix cell yields (e.g. run a
  `T-pinned`/`C-net` workload that the table places at T1 in a T2 gVisor sandbox instead).
  This is always permitted — a stronger wall never violates any invariant.
- A **downward override is NEVER permitted.** No policy, operator, flag, or agent request can
  place a workload below `max(matrix_cell, floor(trust))`. gatekeeperd rejects any construction
  request that resolves below that bound; the rejection is an audited event, not a warning.

```
effective_tier = max( matrix[trust][caps],
                      floor(trust),
                      requested_escalation )      # escalation only ever raises
```

`max` over a tier ordering `T0 < T1 < T2 < T3`. There is deliberately no `min` anywhere in
the resolution.

## 6. What the one-liner hides — the real 80%

Choosing the VMM is the *easy* 20%. The following are hard requirements Phase 5 must deliver;
they are also where the **capability profile is actually enforced**, at every tier.

- **File passing IS the cap-enforcement point.** virtio-fs (T3) / bind + Landlock (T0/T1) /
  gofer-mediated FS (T2) realize `--caps`. gatekeeperd computes the exact mount-set from the
  resolved profile and mounts *only* that. `deny ~/Vault/**` is realized as *the bytes are
  never mounted into the sandbox in the first place* — not a filter after the fact. This is
  the isolation-layer face of `semantic authority ≤ data authority`.

- **Networking is per-workload and deny-by-default.** Each sandbox that gets `C-net` gets a
  dedicated tap/veth device and its **allow-list realized as concrete nftables rules**
  (egress to `github.com, crates.io` = actual resolved-IP/SNI rules, default DROP). No caps
  entry ⇒ no interface attached at all. Never "attach host network, filter later."

- **Rootfs/kernel supply for T3.** Each workload class needs a **signed minimal kernel + minimal
  rootfs**. This is real supply-chain work and a synergy point: the Onion/build system
  (roadmap Phase 2) feeds these signed minimal images. T2 sidesteps a *guest kernel* (gVisor is
  the kernel) but still needs a rootfs; T0/T1 need none.

- **Cold-start / pooling.** Ephemeral per-task agents cannot each pay a cold microVM boot.
  Phase 5 maintains a **pre-warmed pool** per workload class (esp. T2/T3) so `shrek run` for a
  short-lived agent is warm-fast. Pool warming runs under cgroup limits and yields to
  interactive work (roadmap benchmark rule: B ≈ A for interactive).

## 7. The agentd ↔ gatekeeperd contract

Clean privilege split — the resolver is unprivileged, the constructor is the broker.

```
agentd (unprivileged resolver)
  input:  agent identity + requested (trust, caps) + optional escalation
  step 1: validate caps ⊆ agent's granted profile        (policy; agents.md)
  step 2: tier = max(matrix[trust][caps], floor(trust), escalation)   (§5)
  output: a SEALED construction request:
          { tier, mount-set, net-set, resource-limits, provenance-id }

gatekeeperd (privileged broker — the ONLY thing that builds sandboxes)
  step 3: RE-CHECK the floor + caps⊆profile independently (never trust agentd blindly)
  step 4: refuse if tier < max(matrix, floor)             → audited rejection, not a warning
  step 5: refuse if mount-set/net-set exceed the granted profile
  step 6: construct: tier runtime + virtio-fs/bind mounts + tap+nftables + limits
  step 7: emit provenance (architecture.md §8): tier, caps, network, actor, model, reason
```

Two independent checks (agentd resolves, gatekeeperd re-verifies) means a bug or compromise in
the unprivileged resolver **cannot** widen a sandbox: gatekeeperd is the wall and refuses
anything exceeding `max(matrix, floor)` or the granted profile. gatekeeperd is small,
privileged, and auditable precisely so this recheck is the single trusted chokepoint.

## 8. Worked examples

The four intent→mechanism lines from architecture.md §4, fully resolved:

```
"summarize my notes"
  code = first-party summarizer (T-first); caps = read the notes, no secrets, no net (C-ro-nosec)
  → matrix[T-first][C-ro-nosec] = T0 ; floor(T-first)=T0 ; no escalation → T0
  mounts: ro bind of ~/Notes/**   net: none

"compile Shrek's kernel"
  code = trusted toolchain (T-pinned); caps = rw build tree, no secrets, no net (C-proj-rw)
  → matrix[T-pinned][C-proj-rw] = T1 ; floor=T0 → T1
  mounts: rw ~/build/**, ro toolchain sysext   net: none

"clone this random repo and run its tests"
  code = untrusted (T-untrust); caps = rw project scope, needs to fetch deps → net (C-net)
  → matrix[T-untrust][C-net] = T2 ; floor(T-untrust)=T2 → T2 (gVisor)
  mounts: rw ~/scratch/repo/**   net: tap + nftables allow {registry hosts}, else DROP

"autonomous agent installs deps + runs generated code"
  code = AI-generated/autonomous (T-hostile); caps = rw + network egress (C-net)
  → matrix[T-hostile][C-net] = T3 ; floor(T-hostile)=T2, escalated by matrix to T3 → T3 (microVM)
  mounts: virtio-fs rw of an ISOLATED workspace only (never $HOME); NEVER ~/.ssh, ~/Vault
  net: dedicated tap + nftables allow-list, default DROP
  rootfs: signed minimal kernel+rootfs from the Onion build; drawn from warm pool
```

Note the last example never mounts `$HOME` even at T3 — the load-bearing claim of §2 in
practice: the strong wall does not excuse a wide radius.

## 9. Open questions

- **Pool warming policy.** How many warm T2/T3 instances per workload class, and the
  eviction/thermal-yield policy so idle isolation never competes with interactive work.
- **gVisor gofer performance on Swamp-class workloads.** gVisor's file path (gofer) is slower
  for metadata-heavy FS access; measure whether `swampd`-adjacent untrusted workloads need a
  T2 tuning profile or fall back to T1/T3.
- **Landlock-inside-T1.** Whether Tier-1 containers should additionally carry a per-container
  Landlock ruleset (belt-and-suspenders) by default, or only on request.
- **Trust-band inference.** How `agentd` derives the trust band for a given invocation (signed
  manifest? provenance DB lookup? explicit `--trust`?) — a policy question owed to agents.md,
  but it gates the matrix, so it must be pinned before Phase 8.
```

