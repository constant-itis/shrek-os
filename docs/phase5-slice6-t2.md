# Phase-5 slice-6 — the T2 (gVisor) sandbox constructor

> **Boundary draft — for plan review, not yet implemented.** No code until this boundary is
> approved. Numbering: **slice-5 stays reserved for the crypto-seal + socket transport** (as the
> earlier slice docs record); the T2 constructor is **slice-6**.

Slice-4 landed the genuine **T0** constructor; slices 1–3 built the **T1** mount + egress planes and
the `(trust × caps) → tier` decision plane. But the decision plane's **floor** pins
`T-untrust → T2` and `T-hostile → T2` (`isolation.md §5.1`), and **T2 has no constructor** — so today
every `T-untrust` and `T-hostile` workload **fails closed and cannot run at all.** Two entire trust
bands are unreachable. The `gate: refuse if effective ≥ T2` line in `sandbox.rs recheck()` is the
standing placeholder.

Slice-6 removes that gate for T2. It builds the **T2 constructor: a gVisor (`runsc`) userspace-kernel
sandbox**, dispatched from the same decision plane, satisfying the same caps property (granted path
present, ungranted + secret paths **absent**) and the same fail-closed invariants as T0/T1 — now with
gVisor's Sentry as the kernel the workload sees, rather than the host kernel behind Landlock/seccomp.

This is the isolation-technology half of the phase-5 milestone ("same workload runs at any tier by
flag"), specifically the **floor** the two least-trusted bands stand on.

## What this slice is (and is NOT)

**In scope:**
- The T2 constructor in `gatekeeperd` — orchestrating `runsc` from a generated **OCI bundle**
  (`config.json` + a pinned, verity-sealed minimal rootfs), across the `create → start → wait →
  delete` lifecycle, with deterministic teardown/cleanup on every exit path.
- **Platform selection with a real usability probe** (`kvm` vs `systrap`) — see §Platform.
- Caps realization at T2 via the bundle mount-set (grant = mount, absence = omit) and
  **`--network=none`** (loopback-only) for the no-egress posture.
- cgroup-v2 placement under the delegated scope (reusing slice-4's `Delegate=yes` base).
- `sandbox.rs` dispatch: `effective == T2 → t2_plane`; the `≥ T2 ⇒ refuse` gate flips to
  `T2 ⇒ construct`, while `≥ T3 ⇒ refuse` **stays** (T3 is the next slice's floor concern).
- Shipping the **pinned `runsc` release** verity-sealed in `/usr` (with the pinned rootfs). The exact
  packaging — one binary vs binary + required runtime sidecar(s) — is a **pre-code verification item**
  against the chosen release (§Shipping), not an assumption: seal whatever that release actually
  requires at runtime.
- Oracle proof + a new sealed-VM gate section (**S5**).

**Out of scope (deferred, by explicit decision — these are boundaries, not bugs):**
- **Net-cap egress under gVisor.** gVisor uses its own userspace **netstack**; wiring the slice-3
  deny-by-default egress-profile table through a host tap ↔ Sentry netstack is a slice's worth of
  work. Slice-6 serves **`C-ro-nosec` and `C-proj-rw` only**; the existing
  `gate: refuse if caps need egress` keeps firing for `C-net` at T2. (Mirrors how slice-2 deferred
  egress and slice-3 added it for T1.)
- **The general signed-rootfs *supply pipeline* + pooled cold-start** (`isolation.md §6`). Slice-6
  ships **one** pinned, sealed minimal rootfs; the per-workload-class rootfs supply and the warm pool
  are later work.
- **Write realization** — grants mount read-only in slice-6, matching T0/T1's current `bind-ro`
  posture. `C-proj-rw` genuine write is still a cross-tier follow-up. (For `C-proj-rw` the *project*
  mount is where write would land; until the cross-tier write slice, it mounts `ro` like the rest.)
- **T3** (libkrun/Kata microVM) constructor; trust-band integrity-sourcing (OPEN B1).

## The dispatch is unchanged; the constructor is new

The `agentd resolve → gatekeeperd re-check` contract (`isolation.md §7`) is **untouched**. agentd
still resolves `effective = max(matrix, floor, escalation)`; gatekeeperd still recomputes the bound
from the **compiled-in, verity-sealed** policy table and refuses anything below it or exceeding the
granted profile. Slice-6 only supplies the **constructor** that the re-check dispatches to when the
(independently re-verified) result is exactly T2.

```
effective == T2  →  gatekeeperd t2_plane:
   1. re-check bound + caps ⊆ profile         (sealed table — unchanged from slice-2)
   2. refuse if caps need egress (C-net)       (deferred; unchanged gate)
   3. platform = select_platform()             (probe — §Platform; fail-closed, never falls to T1)
   4. build OCI bundle from the SEALED grant set + compiled-in policy  (§Bundle)
   5. place under delegated cgroup-v2 leaf (mem.max/pids.max)          (reuse slice-4 base)
   6. runsc --root=<state> create/start/wait/delete  --network=none --platform=<sel>
   7. emit provenance (tier=T2, platform, caps, net=none, actor, reason)
```

## Platform — the usability probe (the one refinement over "check /dev/kvm")

`runsc` supports `systrap` (default), `kvm`, and `ptrace` (deprecated) via `--platform=`. It does
**no** auto-fallback: whatever platform we name, if construction fails, boot fails — the constructor
owns the decision. `runsc platforms` only lists *compiled-in* names, it does **not** probe usability.

**KVM must be probed for genuine usability, not mere presence.** The device node can exist on a
nested host while VM creation fails. The decisive probe, from the privileged daemon:

```
open("/dev/kvm", O_RDWR)                         # device access
→ ioctl(fd, KVM_GET_API_VERSION) == 12           # subsystem present (refuse if != 12)
→ ioctl(fd, KVM_CREATE_VM, 0)  succeeds           # DECISIVE — a returned VM fd proves construction
→ (optional) ioctl(vmfd, KVM_CREATE_VCPU, 0)      # full fidelity: matches runsc's own path
```

This mirrors exactly what `runsc` does internally (`pkg/sentry/platform/kvm`): a single
`KVM_CREATE_VM`, then `KVM_CREATE_VCPU`. `stat(/dev/kvm)` is insufficient; so is API-version alone.

**Selection policy — DECIDED (Option B).** gVisor's **own** guidance is to prefer **systrap inside
nested VMs** — nested KVM is slower **and** adds hypervisor attack surface (CVE history). Shrek runs
inside a **sealed guest VM**, and T2 is the wall for the **least-trusted** code, so by the roadmap's
`security > performance` order the policy is:

> **Prefer `systrap` on nested/VM hosts; use `kvm` only on bare-metal** where the probe passes *and*
> it is a net win. Same genuine T2 wall either way.

Virtualization detection is **authoritative via `systemd-detect-virt`** (multi-signal: DMI, CPUID,
container cgroups) — NOT the CPUID `hypervisor` flag alone, which a VM can hide (`-cpu
host,-hypervisor`) and so could misclassify a VM as bare metal and wrongly pick KVM. If the detector
is unavailable, the selection defaults to **`systrap`** (the safe default — KVM is only a bare-metal
perf win; systrap is always a valid T2 wall, so we never risk KVM without confirming bare metal).
`KVM_CREATE_VM` remains the usability probe; `/proc/cpuinfo` `vmx`/`svm` is kept as **diagnostic
only**, not a decision input (the KVM probe already subsumes it). (The rejected alternative — "prefer
KVM whenever usable" — would put the most dangerous workloads on nested KVM, backwards for this
target.)

Either way the **fail-closed rule holds**: a `kvm ⇄ systrap` choice is made **at clean preflight
only** — both are genuine T2, so switching between them is *not* a tier change (analogous to slice-4's
"fall-up only at clean preflight"). If the *selected* platform fails once construction has begun, the
build **fails closed**. And under no circumstance does T2 fall **down** to T1 — T1 is below the
`T-untrust`/`T-hostile` floor.

## Bundle — caps realization (absence model, like T1)

The sandbox runs in an **empty mount namespace**: only `root.path` and explicit `mounts[]` entries
exist inside. This gives the caps property for free and matches T1's **absence** semantics (ungranted
path = *not present* / ENOENT), *not* T0's Landlock **deny** (present-but-EACCES).

- **rootfs:** `root.path` → the pinned, verity-sealed minimal rootfs; `root.readonly = true`. Ephemeral
  scratch (e.g. `/tmp`) via a discarded tmpfs overlay (`--overlay2`), not the sealed rootfs.
- **grant → mount:** each granted subtree is a `mounts[]` entry, `options: ["rbind","ro"]`
  (read-only per the deferred-write decision). The vault/secret paths and ungranted siblings are
  **omitted entirely** — invisible, nothing to "deny."
- **file-access:** default `--file-access=shared` (the gofer revalidates against the live host) is the
  safe default; `exclusive`/EROFS are perf options for a later supply-pipeline slice.
- **the deny property is gVisor's Sentry, not host seccomp.** A sandboxed `mount(2)` (or any
  disallowed op) is answered by gVisor's userspace-kernel implementation (`EPERM`/`ENOSYS` by gVisor
  policy) because the Sentry *is* the only kernel the workload can reach. The host's seccomp only
  hardens the **Sentry itself** toward the host (defense-in-depth) — a separate mechanism. The oracle
  asserts the *property*; the *enforcer* is gVisor, and this distinction is stated so the proof reads
  correctly (T0 proved `mount`=our-seccomp-EPERM; T2 proves `mount`=Sentry-denied).
- **no new authority source.** `config.json` is generated deterministically from the sealed grant set
  + compiled-in policy and consumed immediately; it is the one ephemeral writable artifact (like T1's
  generated nspawn args) — gatekeeperd *writes* it from sealed inputs, never *reads it back* as
  authority. The rootfs and the `runsc` binary are verity-sealed.

## Lifecycle & privilege (new responsibility vs T0)

- **Host-root, non-negotiable.** Rootless `runsc` disables both the detached `create` lifecycle **and**
  netstack — we need both — so the T2 constructor runs in the privileged broker, consistent with
  gatekeeperd's existing posture.
- **create → start → wait → delete**, each with an explicit per-workload `--root=<state-dir>` and
  container id. gatekeeperd owns teardown: a failed `create` never reaches `start`; **every** exit
  path (success, failure, signal, abort) runs `kill`+`delete` and removes the state dir and the
  generated bundle. Fail-closed = no orphan sandbox, no leaked state.
- **cgroups — DECIDED: `--ignore-cgroups` + manual delegated leaf.** Reuse slice-4's
  `systemd-run --scope -p Delegate=yes` base; gatekeeperd creates the leaf, sets `memory.max`/
  `pids.max`, and places `runsc` into it before `start`. This matches slice-4's manual cgroup
  management and avoids runsc's EXPERIMENTAL `--systemd-cgroup` driver (the rejected alternative).
- **fd hygiene:** pass only the bundle; all gatekeeperd fds `O_CLOEXEC` so nothing host-side leaks into
  the `runsc` child. (Less load-bearing than T0's scrub — the workload shares no host kernel view —
  but still enforced.)

## Fail-closed invariants (adapted from slice-4)

1. **No unconfined fallback, ever, and no fall-down.** If platform preflight yields nothing usable, or
   bundle/rootfs assembly fails, or `runsc create/start` fails, construction aborts with **no workload
   run**. T2 **never** falls to T1 (that is below the floor). Unlike T0 (which may fall *up* to T1),
   T2 has nothing stronger to fall to short of T3, so failure = closed.
2. **Platform chosen at clean preflight only.** `kvm ⇄ systrap` is decided before construction; both
   are genuine T2 (not a tier change). Any failure *after* construct starts fails closed — no
   mid-build platform switch.
3. **Rules-before-usable.** gVisor's confinement is live from sandbox boot; the workload never executes
   outside the Sentry. There is no unconfined window.
4. **Same caps property as T0/T1.** Granted path readable; ungranted sibling + vault path absent — here
   by empty-mount-ns (gofer-mediated), Sentry-enforced.
5. **No new authority source.** Bundle derived from the sealed grant set + compiled-in policy; rootfs +
   `runsc` verity-sealed; nothing new to seal.

## Gates

- **Oracle** (`scripts/t2-construct-proof.sh`, privileged `debian:trixie`, the real release binary +
  the pinned rootfs; pure decision/probe logic covered by unit tests):
  - **G1** a T2 cell (e.g. `T-untrust / C-ro-nosec`) constructs at genuine T2 — decision says
    `construct-at=T2`; inside the sandbox the grant is readable, the vault is **absent** (ENOENT),
    `mount` is **Sentry-denied**, and `--network=none` yields loopback-only / **no egress**.
  - **G2** a T1 cell (e.g. `T-first / C-net`) still routes to the nspawn constructor; a T0 cell still
    routes to `proc_plane` — the T2 plane doesn't capture them.
  - **G3** the deferred-Net gate: a T2-resolving cell that needs egress (`C-net`) still **fails
    closed** (no egress plane under gVisor yet).
  - **G4** the platform probe: KVM-usability is decided by `KVM_CREATE_VM`, not `stat` — on a host
    where `/dev/kvm` exists but VM creation fails, the probe selects `systrap` (or fails closed per
    the chosen policy), never silently mis-selects.
- **Sealed VM** (`.../mount-plane-gate`, new section **S5**): re-assert G1's properties on the shipped
  kernel + dm-verity `/usr`. The sealed VM is itself nested, so this is also the real test of the
  platform policy — the probe should land on **systrap** there, and S5 asserts `construct-at=T2` on
  `systrap`. If a regression disabled gVisor confinement, S5's in-sandbox assertions (vault-absent /
  mount-denied / no-egress) fail loudly rather than hiding.

Method unchanged: host/container oracle before the ~35-min VM cycle; empirical VM gate before commit;
no unconfined fallback anywhere; no fall-down from the floor.

## Shipping the runsc release — VERIFIED, pinned

The pre-code gate ran on 2026-08-18 in the oracle. Result, recorded in
[`image/supply/gvisor.pin`](../image/supply/gvisor.pin):

- **Pinned release: `release-20260810.0`** (OCI spec 1.2.1, x86_64), `runsc` sha256
  `670bcd3c…3c068e`. Production builds fetch **this exact release** and verify the sha256; the
  `latest` channel is used **only** for bring-up, never in a sealed build.
- **Required runtime artifact set = `runsc` alone — EMPIRICALLY PROVEN.** A minimal OCI bundle ran
  via `runsc run` with **no containerd shim, no sidecar, and no runtime auto-download**, under the
  decided flags `--ignore-cgroups --network=none --platform=systrap`. The gofer is the same binary
  re-executed. `containerd-shim-runsc-v1` exists in the release but is containerd-only — **not** our
  path; it is inventoried in the pin but not fetched or sealed.
- **Seal boundary = `{runsc}`** into `/usr` under dm-verity. Nothing else.
- **Integrity: sha256 (pinned here) + upstream sha512-over-TLS.** The raw release bucket publishes
  **no detached PGP** signature (`runsc.asc` 404s); PGP signing exists only on the apt channel. So
  the seal-time check is the recorded hash, not a signature verify.
- Re-check the pinned release's `flags.go` defaults (`--platform`, `--directfs`) if the pin is ever
  bumped, since those shifted historically.

## Decisions (settled) — recorded for the implementation

1. **Platform policy — Option B:** prefer `systrap` on nested/VM hosts (our sealed-VM target); `kvm`
   only on bare-metal where the probe passes and it's a net win. Both genuine T2. (§Platform)
2. **cgroups — `--ignore-cgroups` + manual delegated leaf** under slice-4's `Delegate=yes` base;
   not the EXPERIMENTAL `--systemd-cgroup` driver. (§Lifecycle)
3. **T2 never falls down to T1** — below the `T-untrust`/`T-hostile` floor; any construction failure
   is fail-closed. (§Fail-closed)
