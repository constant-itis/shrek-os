# ADR-001 — Immutable base platform

**Status:** ✅ Accepted / committed (2026-08-17). Supersedes the earlier Fedora-leaning draft.
**Context:** Shrek OS needs a production foundation. LFS-as-product = becoming a full
distribution's maintainer forever, effort stolen from the only novel parts (`swampd`,
`agentd`, `gatekeeperd`, the capability/ACL model). The base is ~15% of Shrek; the control
plane (the hard 85%) is base-agnostic.

## Decision

**Build Shrek on Debian Stable as package provenance, authored with `mkosi` and delivered as
an immutable sealed image.** The original transport preference was `bootc`; the Phase-1 spike
resolved that fork to `systemd-sysupdate` raw A/B, with bootc/composefs deferred. Agent wall is
Landlock-first (portable), with AppArmor as the system MAC. dm-verity sealing is current.
Secure Boot uses direct UEFI db enrollment in the current spike; shim-review only matters if
Shrek ever ships pre-signed public binaries.

Debian is **package provenance, not the runtime package manager**. The live base is
immutable; `apt` lives only inside dev containers where mutability is desirable.

```
Debian Stable (glibc, systemd, kernel, mesa, openssl, security team, dep graph)
      ↓  mkosi
minimal Debian root image
      ↓  systemd-repart
dm-verity sealed  (→ composefs later)
      ↓  systemd-stub
signed UKI  (Shrek key; enrolled into UEFI db)
      ↓  systemd-sysupdate
transactional raw A/B + rollback + health-check
      ↓
SHREK BASE (read-only, verified, versioned)
      +
THE ONION — signed sysext layers: graphics · desktop · dev · gaming · ai
```

## Why this over the alternatives

The honest weighing (all things considered):

| Factor | Verdict |
|---|---|
| Verified-boot integrity | **Tie** — dm-verity (Debian, turnkey via mkosi today) is adequate & proven (Android/ChromeOS use it); composefs (Fedora, newer) is a *bonus*, not a requirement, and is upstream so Debian can adopt it later |
| Update/rollback engine | **Resolved after spike** — `bootc` was preferred on paper, but the accepted shipped path is `systemd-sysupdate` raw A/B on Debian; bootc/composefs stays a later upgrade |
| MAC | **Slight Debian** — AppArmor is **path-based**, which fits the deterministic wall (`deny ~/Vault/**`) more naturally than SELinux labels |
| Secure Boot | **Tie** — cost is identical and distro-independent (see below); UEFI db enrollment now |
| Desktop HW freshness | Fedora edge, but only bites at Phase 10; mitigable via Debian backports/trixie |
| Builder fluency | **Debian** — a real velocity term on a multi-year solo build, compounding across the base-agnostic 85% |

Net: the only durable technical advantage Fedora had — bootc's update engine — is available
on Debian. Everything else is a tie or favors Debian. So we get the strong technical answer
*and* the base we'll actually enjoy building on.

### Not built ON a derivative
Debian is the provenance. We do **not** build on Vanilla OS/ABRoot or any downstream — that
inherits someone else's updater, package philosophy, and desktop decisions. Study them as
references (as with Universal Blue); build the image ourselves via mkosi.

## Secure Boot — the reality (why Debian doesn't change it)

Trust flows `UEFI firmware → shim (MS-signed) → embeds ONE distro cert → trusts UKI signed
by that cert`. Debian's `shim-signed` embeds **Debian's** cert; Fedora's embeds Fedora's.
**Neither shim chains to a Shrek-signed UKI** — and Shrek requires its own signed UKI
(sealed-root digest baked in). So the cost is identical on both distros. Three exits, all
distro-independent:

```
UEFI db enrollment  Shrek key enrolled directly into firmware db in setup-mode VM spike → no Microsoft.
                  Per-install manual step. → OUR CHOICE for homelab/personal now.
Own MS-signed shim  shim-review (github rhboot/shim-review), embed our cert → for PUBLIC
                  distribution only; weeks-months, needs a real entity. → defer.
Custom db keys    enroll Shrek KEK/db into firmware (Setup Mode) → controlled fleet only.
```

"Riding the distro's signed kernel" only defers this while booting *their* kernel with
*their* key — true on Debian and Fedora equally.

## Update transport — RESOLVED (spike ran; see update-model.md)

The Phase-1 spike settled this fork. bootc + composefs are **unpackaged on Debian trixie**, so
bootc-on-Debian is the fragile source-build path — the pre-authorized **janky → fallback** branch
fired. The shipping transport is `systemd-sysupdate` raw A/B, proven end to end (one update +
automatic rollback) in [`phase1-s7-sysupdate.md`](phase1-s7-sysupdate.md) /
[`phase1-s8-rollback.md`](phase1-s8-rollback.md). Full model + the rollback-vs-anti-rollback split
in [`update-model.md`](update-model.md).

```
RESOLVED  : mkosi + systemd-sysupdate (raw .raw A/B)   — native to trixie systemd 257, we own it
DEFERRED  : bootc + composefs (OCI image → A/B)         — a later UPGRADE, not the shipping path
```

Still Debian, still sealed/verified, just a lower-level updater we fully own.

## Keep the port cheap (either direction)

Three moves make a base swap a config change, not a rewrite — so Fedora stays a cheap escape
hatch and the Debian decision is low-risk:

1. **Author in `mkosi`** — speaks both Debian and Fedora; base swap ≈ `distro=` + package-list delta.
2. **Landlock-first agent wall** — kernel-native, distro-agnostic; shrinks the one genuinely
   distro-specific security rewrite (SELinux↔AppArmor) to a thin secondary layer.
3. **Base-agnostic control plane** — swampd/agentd/gatekeeperd/oniond/shrekctl don't move.

## Build sequence / acceptance test

```
0. Study a stock Fedora bootc image in a VM   ← oracle ("what correct looks like"), 1 afternoon, don't build on it
1. Timeboxed spike: Debian + bootc/sysupdate fork, Phase 1   ← mkosi image, dm-verity sealed, UEFI-db boot, empty scaffold
2. Clean?    → STAY on Debian. Never build Fedora.        (most likely)
3. Janky?    → Debian + mkosi + systemd-sysupdate fallback
4. Blocked?  → build oracle on Fedora, finish hard parts, port to Debian (cheap via mkosi move #1)
```

## Consequences

- Shrek's owned/trusted code = `swampd · agentd · gatekeeperd · shrekctl · oniond` +
  policies + mkosi image definition + AppArmor/Landlock profiles + semantic ACL +
  provenance + UI. Clean project boundary.
- Spec amendment: §5 MAC changes from "SELinux" to **AppArmor (system) + Landlock (agent
  wall)**. Landlock is the real wall; AppArmor is belt-and-suspenders.
- Integrity starts as **dm-verity** (fixed-size sealed root, proven), composefs as a later
  upgrade — not a blocker.
- First milestone = "hardened Debian mkosi image boots sealed under UEFI Secure Boot with an empty
  control-plane scaffold" (Phase 1 acceptance test above).
- **Security-model amendments incorporated** (see [`security-model.md`](security-model.md) §4,
  §8): (a) **TPM PCR sealing** targets are specified for the boot measurement, the mutable-policy
  digest root, and the image SVN; (b) **anti-rollback** — a monotonic **security version counter
  (SVN)** in a TPM NV index; boot refuses any image below the floor, the floor advances **only on
  greenboot-healthy commit** (so bootc A/B rollback never bricks), and recovery lands ≥ the
  current SVN; (c) **static policy is baked into the image** under the dm-verity root (it is
  security-critical + version-static, per the §3 routing rule), while per-machine grants are
  anchored to a **separate** TPM NV monotonic counter checked every load. TPM-absent ⇒ documented
  lower-assurance degrade (software counter), never a silent claim of a guarantee it can't back.

## Open questions

- Debian **Stable vs Trixie** for the prototype (hardware freshness vs stability) — decide at spike.
- Exact bootc-on-Debian maturity — the Phase-1 spike is the test.
- When/if composefs replaces dm-verity; when/if a custom kernel is worth stepping off the
  signed chain (treat as late/expensive/opt-in).
- **⚠ Live-verify on target TPMs** (security-model.md §8/AS3): the anti-rollback rests on the
  TPM 2.0 property that a (re)created NV counter initializes to ≥ the max any counter has ever
  held (defeats destroy-and-recreate) — confirm on real hardware at the Phase-1 spike, and pin
  the NV-index owner/policy auth custody.

## Sources

- [bootc — Introduction](https://bootc.dev/) · [generic build guidance (distro owns base integration)](https://bootc.dev/bootc/building/guidance.html) · [composefs backend](https://bootc.dev/bootc/experimental-composefs.html)
- [ubuntu-bootc — composefs-native Debian-family experiment](https://github.com/jmarrero/ubuntu-bootc)
- [mkosi.news(7) — signed sysext/confext, systemd-sysupdate (Debian)](https://manpages.debian.org/testing/mkosi/mkosi.news.7.en.html)
- [systemd-sysext(8)](https://www.freedesktop.org/software/systemd/man/systemd-sysext.html) · [ostree in Debian sid](https://packages.debian.org/sid/ostree)
- [Vanilla OS Orchid / ABRoot (Debian Sid base) — reference only](https://vanillaos.org/)
- [Image sealing with composefs (Scrivano)](https://scrivano.org/posts/2026-06-05-sealing-with-composefs/) · [Fedora sealed bootc images (Fedora Magazine)](https://fedoramagazine.org/sealed-atomic-desktops-test-images/)
