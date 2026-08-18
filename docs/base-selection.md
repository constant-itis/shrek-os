# ADR-001 — Immutable base platform

**Status:** Accepted (design-time)
**Date:** 2026-08-17
**Context:** Shrek OS needs a production foundation. The original plan built everything on
LFS. That makes Sebastian the permanent maintainer of a full distribution's boring
substrate — kernel packaging, glibc, openssl, mesa, firmware, dependency resolution,
security backports — which is effort stolen from the only novel parts (`swampd`,
`agentd`, `gatekeeperd`, the capability/ACL model).

## Decision

**Prototype and ship the product on a `bootc` immutable base derived from Fedora Atomic;
keep a path to CentOS Stream image mode for the stable/distributable channel. Use NixOS as
a reproducible-build lab for layer *contents*, not as the base. Keep LFS as a frozen
research reference only.**

## Why bootc (and not the alternatives)

Requirements traced to Shrek invariants, and how each candidate scores:

| Requirement (Shrek §)                         | **bootc / Fedora Atomic** | openSUSE MicroOS/Aeon | NixOS            | LFS (as product) |
| --------------------------------------------- | ------------------------- | --------------------- | ---------------- | ---------------- |
| OCI-image transactional update + rollback (§8)| ✅ native (that's the model)| ⚠️ btrfs snapshots, not OCI | ⚠️ generations, not OCI | ❌ build it yourself |
| Sealed whole-tree integrity (§3, §9)          | ✅ **sealed composefs + fs-verity** (landing now) | ⚠️ btrfs, no sealed-image model | ⚠️ not the same story | ❌ hand-roll dm-verity |
| Verified boot chain UKI→root→layers (§9)      | ✅ being built upstream    | ⚠️ partial            | ⚠️ DIY           | ❌ DIY |
| Signed Secure Boot shim out of the box (§9)   | ✅ Fedora's signed shim+kernel | ✅ (SUSE)          | ❌ MOK-enroll pain | ❌ own the whole chain |
| sysext "Onion" is idiomatic (§7)              | ✅ systemd-native          | ✅ systemd-native      | ⚠️ fights the Nix model | ✅ but you build it |
| SELinux-native MAC (§5)                        | ✅ first-class             | ❌ AppArmor            | ❌ painful       | ⚠️ DIY |
| Sustainable substrate (the whole point)       | ✅ Red Hat/Fedora own it   | ✅ SUSE owns it        | ✅ community      | ❌ **you own it forever** |
| Desktop hardware enablement (§11)             | ✅ leads immutable desktop | ✅ good                | ⚠️ variable      | ❌ DIY |

### The clincher: integrity is being handed to us

As of mid-2026 the exact chain Shrek §9 specified — `UKI → systemd-boot → sealed composefs
root → fs-verity` — is being built **upstream on the Fedora Atomic bootc base**. Sealed
bootable-container test images shipped on Fedora 44 (built with Rust `composefs-rs` over
overlayfs/EROFS/fs-verity). composefs sealing is **stronger than the original EROFS +
dm-verity plan**: one digest authenticates the whole tree — file *contents and* metadata
(permissions, ownership, xattrs, symlinks) — with no fixed-partition requirement.

Translation: the single hardest, most security-critical piece of Shrek's base — a fully
verified boot chain — arrives as upstream plumbing on the platform we'd pick anyway.
Building it on LFS would mean re-implementing all of it by hand and owning the Secure Boot
signing bureaucracy on top.

### Why not the others as the base

- **openSUSE MicroOS/Aeon** — solid transactional model, but btrfs-snapshot-based, *not*
  OCI-image; AppArmor not SELinux; no sealed-image integrity story matching composefs. Good
  system, wrong shape for the Onion + capability model.
- **NixOS** — best-in-class *reproducibility*, but a different paradigm: no OCI image
  transport, sysext is un-idiomatic, SELinux is weak. **Reassigned role:** use Nix/Guix as
  the reproducible-build lab that produces the *contents* of Shrek sysext layers with
  pinned hashes — the "verified tarball → build recipe → artifact → manifest" pipeline —
  without being the OS.
- **Universal Blue (Bluefin/Bazzite/Aurora)** — these are *downstream examples* of doing a
  bootc desktop well. **Study and borrow** their desktop polish and image-build ergonomics;
  don't adopt one as the base — Shrek builds its own image from the Fedora bootc base.
- **Ubuntu bootc** — experimental (composefs-native still an experiment); the Red Hat-led
  bootc ecosystem is well ahead. Skip for now.

## Product vs stable channel

- **Now / prototype:** Fedora bootc (Atomic) — newest bootc, composefs, sysext, best
  hardware + desktop enablement. Fastest path to a running `swampd`/`agentd` demo.
- **Later / distributable stable:** evaluate **CentOS Stream image mode** (the same bootc
  model, RHEL-adjacent, slower/longer-supported) for the channel real users install. The
  bootc image definition should stay base-agnostic enough to retarget.

## Interesting symmetry

Both hand-rolled substrates get demoted to *labs*, each keeping its genuine value:

```
LFS   → reference-lfs/   : bottom-up understanding of what Shrek stands on   (frozen)
NixOS → build lab        : reproducible, hash-pinned layer contents          (tooling)
```

Neither is the product. The product base is bootc.

## Consequences

- Shrek's owned/trusted code shrinks to: `swampd · agentd · gatekeeperd · shrekctl ·
  oniond` + policies + image definition + security profiles + semantic ACL + provenance +
  UI. That is a *much* better project boundary.
- We inherit Fedora's decisions (SELinux policy, kernel config, release cadence). The one
  place this bites is a custom hardened kernel — treat that as late/expensive/opt-in and
  ride the signed stock kernel as long as possible (see architecture.md §2).
- First implementation milestone becomes "a hardened Fedora bootc image that boots sealed,
  with an empty Shrek control-plane scaffold" — not "bring up LFS."

## Open questions

- Exact bootc base tag/version to pin the prototype to, and cadence for rebasing.
- When (if ever) a custom kernel config is worth stepping off Fedora's signed chain.
- Whether the stable channel is CentOS Stream image mode or a pinned Fedora — revisit once
  the control plane exists.

## Sources

- [bootc — Introduction](https://bootc.dev/) / [composefs backend (experimental)](https://bootc.dev/bootc/experimental-composefs.html)
- [Image sealing with composefs — Giuseppe Scrivano](https://scrivano.org/posts/2026-06-05-sealing-with-composefs/)
- [Sealed Fedora Atomic Desktop bootable container images — Fedora Magazine](https://fedoramagazine.org/sealed-atomic-desktops-test-images/)
- [Fedora Sealed Bootable Container Images — a "Fully Verified Boot Chain" — Privacy Guides](https://www.privacyguides.org/news/2026/05/04/fedora-sealed-bootable-container-images-possibly-opening-the-door-to-a-fully-verified-boot-chain/)
- [UKI, composefs and remote attestation for Bootable Containers — All Systems Go! 2025](https://cfp.all-systems-go.io/all-systems-go-2025/talk/TNKPQS/)
- [systemd-sysext(8) — freedesktop.org](https://www.freedesktop.org/software/systemd/man/systemd-sysext.html)
- [NixOS — declarative builds and deployments](https://nixos.org/)
