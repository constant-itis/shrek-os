# Shrek OS — Phase-1 spike plan (the base acceptance test)

> Build the smallest thing that proves the base: a hardened Debian image that boots **sealed**
> under our own key, and rolls back a broken update. Everything else waits.

This is the executable plan for roadmap **Phase 1**. It is a *timeboxed spike*, not the product
build — its job is to answer one question: **does Debian + bootc, sealed with dm-verity and
booted under a MOK-enrolled UKI, hold together cleanly enough to stay on it?**

```
ACCEPTANCE TEST (breaks the base tie — base-selection.md ADR-001):
  clean   → STAY on Debian + bootc.                       (most likely)
  janky   → fall back to mkosi + systemd-sysupdate raw A/B.
  blocked → build a Fedora bootc oracle, finish the hard parts there, port back (cheap via mkosi).
```

---

## 0. Build-environment decision (settled — see host survey 2026-08-17)

**beepboop cannot build this in place**, and we will not mutate it to try. It is Pop!_OS 22.04
on **systemd 249**; `mkosi`, `systemd-repart`, `ukify`, `mokutil`, `bootc`, `mkfs.erofs` are all
absent, and 249 is too old for the repart/ukify sealing workflow (needs ≥ 254). Installing that
stack onto a daily-driver desktop violates the safe-change + minimal-deps rules.

What beepboop *does* have, and what we use instead:

```
cargo/rustc 1.95     → build + compile-check the Rust control plane locally (no container needed)
docker 28.3          → BUILD the image inside a Debian TRIXIE container (mkosi + modern systemd
                       live there; the host stays untouched)  — mkosi's normal mode of operation
/dev/kvm + libvirt   → BOOT the built image + the Fedora oracle in throwaway VMs (virt-install)
452 G free on /home  → ample for image artifacts
```

**Rule for the whole spike: no build tooling is installed on the beepboop host.** mkosi/ukify/
repart run inside the trixie build container; boots happen in disposable VMs.

---

## 1. Sub-steps, in order (each a go/no-go gate)

```
S0  Fedora oracle           study a stock Fedora bootc image in a VM — "what correct looks like".
                            Don't build on it; read its UKI, its sealed-root layout, its bootupd.
S1  Control-plane scaffold  the 5 daemons as empty, DISABLED stubs that compile + install.  ← in-repo now
S2  mkosi trixie base       minimal Debian trixie root via mkosi, in the trixie build container.
S3  Hardening               AppArmor enforcing, sysctl/kernel-cmdline hardening, no apt at runtime.
S4  dm-verity seal          systemd-repart builds a sealed, read-only verity root.
S5  UKI + our key           ukify assembles a UKI (systemd-stub), signed with the Shrek key.
S6  MOK boot                enroll the Shrek key via MOK in the VM; boot the sealed UKI under
                            Secure Boot.  ← the core acceptance gate.
S7  bootc wrap              deliver as a bootc OCI image; do one A/B update.
S8  Rollback proof          deliberately break an update → automatic return to last-good.  ← milestone.
```

**Milestone:** Shrek boots *sealed* on a VM (and, later, bare metal) under MOK; a broken update
rolls back. When S6+S8 pass clean → the ADR-001 tie breaks toward **stay on Debian + bootc**.

---

## 2. The empty control-plane scaffold (S1 — the part that lives in this repo)

The acceptance test requires "an empty Shrek control-plane scaffold" present in the image. It is
a cargo workspace of five binaries, each a **disabled stub** — they install, they have systemd
units, and the units are **not enabled**. This exists so the *shape* is real from day one and so
the §9 critical-failure test is meaningful (stopping them must change nothing yet).

```
crates/
  swampd/       filesystem-intelligence daemon        (Phase 6+) — stub: log + idle, no indexing
  agentd/       agent identity + tier/caps resolver    (Phase 8)  — stub
  gatekeeperd/  privileged broker; the ONLY sandbox builder + policy recheck (isolation.md §7)
  oniond/       sysext layer policy/orchestration      (Phase 2/4)
  shrekctl/     operator CLI (find/history/run/audit)  — stub: prints "scaffold" and exits
```

Each stub: parses no config, opens no sockets, holds no privilege — it logs a single "disabled
scaffold" line and sleeps (daemons) or prints usage and exits (shrekctl). This keeps S1 honest:
nothing here enforces anything yet, and the docs (security-model.md etc.) are the spec the real
implementations will be built to. The **security posture is authored before the code**, on
purpose (roadmap priority: correctness → security → … → AI features).

Units ship **disabled** (`WantedBy` absent / masked in the image preset) so a Phase-1 image boots
a bare hardened Debian with an inert Shrek scaffold — exactly the acceptance-test target.

---

## 3. Reproducible build path (no host mutation)

```
scripts/build-in-container.sh   # runs mkosi inside a debian:trixie container:
                                #   - installs mkosi + systemd-repart + ukify + erofs/verity tools
                                #     INSIDE the container (ephemeral),
                                #   - mounts this repo + an output dir,
                                #   - emits: sealed verity root + signed UKI + bootc OCI image.
scripts/boot-vm.sh              # virt-install/qemu boots the artifact in a KVM VM with OVMF +
                                #   an enrolled Shrek MOK, for S6/S8.
scripts/fedora-oracle.sh        # S0: pull + boot a stock Fedora bootc image read-only, as oracle.
```

Keys: a throwaway **Shrek signing key** is generated into `keys/` (gitignored) for the spike —
MOK-enrolled in the VM, never shipped. Public distribution / shim-review is out of scope for the
spike (base-selection.md).

---

## 4. What the spike deliberately does NOT do

- No composefs (dm-verity is the Phase-1 integrity primitive; composefs is a later upgrade).
- No real swampd/agentd logic, no sandboxing runtime (that is Phases 5–8) — stubs only.
- No sealed policy plane / TPM NV counter implementation yet — those are **specified**
  (security-model.md §4) and get built when the control plane does; Phase 1 only needs the image
  to boot sealed with the scaffold present.
- No bare-metal MOK enrollment until it passes in a VM first.
- No apt at runtime — apt is dev-container-only (base-selection.md).

---

## 5. Offload note (build boilerplate)

The bulk boilerplate this spike generates — `mkosi.conf` + hardening drop-ins, systemd unit
files, further Rust stub expansion — is a good fit for the local coder tier (`gpu-mode coder`
→ `askcoder`), reviewed on the primary model. That flip stops BRENT/OnePlus, so confirm BRENT is
idle and flip back (`gpu-mode brent`) after. The initial scaffold in this repo was authored
directly (small, and must match the architecture precisely); heavier config generation is where
the coder tier earns its keep.
