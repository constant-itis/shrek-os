# Shrek OS — Roadmap (revised)

Revised from the original 11-phase plan to reflect the two-track decision: the product is
built from **Debian package provenance via mkosi**, not LFS. The first transport branch
resolved to `systemd-sysupdate` raw A/B; bootc/composefs is deferred. LFS work moves to a
frozen research track that runs *in parallel and does not block* the product.

## Guiding order (when forced to choose)

```
correctness → security → recoverability → performance → cleanliness → AI features → cosmetics
```

AI is deliberately late. Keep the fun AI concepts from infecting the foundation before the
boring OS problems are solved.

---

## Research track (parallel, non-blocking, frozen when done)

**R0 — LFS reference build.** Build a bootable LFS system once, end-to-end:
toolchain → libc → systemd → kernel → initrd → UKI → rootfs → EROFS → dm-verity → sysext.
Goal is *understanding*, not a shippable distro. Document the bottom-up story, then
**freeze it.** Do not track upstream. Port lessons to the product image, never the code.

**R1 — Reproducible build lab (Nix/Guix).** Stand up the "verified tarball → pinned build
recipe → isolated build → artifact → signed manifest" pipeline that later produces the
*contents* of Shrek sysext layers. Record source URL, source hash, patches, compiler,
flags, dependency versions, output hashes.

---

## Product track (the critical path)

### Phase 0 — Architecture spec ✅ (this repo)
Freeze invariants before implementation. `architecture.md`, `base-selection.md`,
`roadmap.md`, `isolation.md` (four tiers, the trust×caps dials, the selection matrix),
`threat-model.md` (assets, adversary catalog, trust boundaries, attack narratives),
`security-model.md` (threat→primitive mapping + the OPEN-resolving amendments) done.
`filesystem-intelligence.md` (three maps, logical domains, escalation ladder, modular tiers),
`swamp.md` (swampd component, object record, confinement, event pipeline),
`update-model.md` (sysupdate A/B transport, rollback vs anti-rollback), and
`agents.md` (identity, capability profile, trust band, lifecycle, untrusted-content-is-not-instruction)
done. **All Phase-0 docs written.**
**Milestone:** Shrek Architecture v0.1 frozen.

### Phase 1 — Hardened Debian sealed base (+ base acceptance test)
Produce a Shrek image from **Debian Stable via mkosi**: hardened config, AppArmor enforcing,
UKI (systemd-stub, Shrek key), **dm-verity** sealed root, Secure Boot (Shrek key auto-enrolled
into UEFI db — no shim/MOK, since we sign our own UKI), empty Shrek control-plane scaffold.
**Acceptance test (breaks the base tie):** clean → stay on Debian+bootc; janky → fall back
to `mkosi` + `systemd-sysupdate`; blocked → build a Fedora oracle and port back.
**RESOLVED:** bootc/composefs unpackaged on trixie → the **janky→fallback** branch fired.
**Stay on Debian**, transport = `systemd-sysupdate` raw A/B ([`update-model.md`](update-model.md),
S7/S8). bootc deferred to a later upgrade.
**Milestone (met):** Shrek boots *sealed* under enforcing Secure Boot; a broken update rolls back
automatically ([`phase1-s8-rollback.md`](phase1-s8-rollback.md)).

### Phase 2 — The Onion (sysext layers)
Build `graphics`, `desktop`, `dev`, `gaming`, `ai` as signed, Verity-authenticated sysext
images (confext for `/etc`). Establish the delivery routing rule (image vs sysext vs
Flatpak). Compatibility metadata + signatures + activation/deactivation/dependency mgmt —
but layering itself is `systemd-sysext`, not our code.
**Milestone:** Shrek's visible OS composes from independently managed, signed layers.

### Phase 3 — Transactional boot + recovery hardening
Built on `systemd-sysupdate` A/B + boot-assessment (bootc dropped, §Phase 1); add greenboot-style
health validation, staged updates, recovery mode. Model in [`update-model.md`](update-model.md).
**Milestone (core met at S8):** Deliberately destroy an update → automatic return to the last
good system ([`phase1-s8-rollback.md`](phase1-s8-rollback.md)).

### Phase 4 — Rust control plane
`shrekctl`, `oniond` (policy/orchestration over sysext), `gatekeeperd` (privileged broker).
Establish IPC conventions, structured audit logs, privilege separation, policy format,
daemon security profiles.
**Milestone:** Routine privileged ops no longer need arbitrary root.

Design-of-record: `grant-protocol.md` (the trusted-path capability-approval flow / policy/agent-UI
role from `shell-architecture.md`). Extends the `gatekeeperd` broker skeleton with a `grant` verb
family and a human trusted path (VT-primary via systemd `SecureAttentionKey`); introduces the first
mutable grants (TPM NV-counter). The rendered prompt is the Phase-10 agent permission UI.

### Phase 5 — Isolation runtime
Wire the four-tier `shrek run --trust=<tier> --caps=<profile>` model: Tier 0 (Landlock/
seccomp + full namespaces + cgroups, no rootfs), Tier 1 (`systemd-nspawn`), Tier 2
(**gVisor**), Tier 3 (libkrun/Kata). Build the plumbing the one-liner hides: capability
mounts, private networking + nftables egress, signed minimal rootfs supply for later VM
tiers, pooled cold-start.

**Current state:** Slice 1 shipped T1 mount construction (pin→verify→relocate, synthetic root);
slice 2 shipped the compiled trust×caps decision plane and broker re-check; slice 3 shipped
T1 private-netns egress with sealed named profiles; slice 4 shipped genuine T0 Landlock/seccomp
construction for the T0 matrix cells; slice 6 shipped the T2 gVisor constructor; slice 7 shipped
trust-band provenance derivation; slice 8 shipped static pin-manifest classification for
`T-pinned`; slice 9 shipped the `T-pinned` execution home (static PIE from a one-inode exec
island); slice 10 shipped sealed-dynamic `T-pinned` execution (dynamically-linked entrypoint
from an authenticated N-inode closure island). **≥T1 containment of a pinned artifact**
(floor(Pinned)=T0), **T3**, and **T2 egress** remain deferred.
**Milestone:** the same workload runs at constructed tiers by flag; caps are enforced at the mount/
net layer independent of tier, and unbuilt stronger tiers fail closed.

### Phase 6 — Swamp metadata layer (no AI yet)
`swampd`: coalesced FS events, SQLite metadata, hashes, MIME, FTS, provenance, ignore
rules. **Landlock swampd itself** out of the human-only domains from day one (confused-
deputy fix). Commands: `shrek find | history | related | status`.
**Milestone:** useful system-wide indexed search with zero embeddings.

### Phase 7 — Semantic Swamp (optional)
Local embeddings, relationships, vector retrieval, NL queries — all behind the search-tier
escalation (metadata → FTS → semantic → LLM). Enforce `semantic authority ≤ data
authority` at index *and* query time.
**Milestone:** NL retrieval that cannot cross a filesystem/security boundary.

### Phase 8 — Agent runtime
`agentd`: agent identity, capability profiles, sandbox construction across the four tiers,
FS/network/tool scope, resource limits, human approval, provenance. Composes with Phase 5.
**Milestone:** an agent works autonomously inside one project while *technically
incapable* of touching a protected directory.

### Phase 9 — Semantic security / DLP
Data classification, export policy, semantic DLP **as a tripwire only** (deterministic wall
already enforced in Phases 6/8), secret awareness, human-only-domain isolation.
**Milestone:** restricted info cannot leak via direct file access *or* semantic inference.

### Phase 10 — Desktop productization
Only after the base is stable: Wayland desktop, settings UI, graphical layer manager, agent
permission UI, semantic search UI, recovery UI, installer, hardware detection, Shrek
identity. Borrow ergonomics from Universal Blue / Vanilla OS (reference only). Decide the
stable channel here (Debian Stable vs Trixie for hardware freshness).
**Milestone:** a non-developer can install and use Shrek.

Design-of-record: `shell-architecture.md` (**Shrek Shell** — roles as swappable systemd user
services, delivered as Onion layers). It adds a **three-rung ladder** the original single "Wayland
desktop" line did not capture: Rung 0 bare (no shell layer), **Rung 1 a terminal-native TUI shell**
(new — zellij-based, no Wayland, runs over SSH), Rung 2 the graphical Wayland shell. The agent
permission UI is the one trusted-path role and couples forward to Phase 4 (`gatekeeperd`).

---

## Benchmark profiles (measured from Phase 1 on)

```
A — Debian bootc baseline (no Shrek control plane)
B — Shrek core enabled, swampd metadata only
C — full Shrek intelligence, semantic workers enabled
```

Hard requirement: **B stays very close to A for interactive workloads.** Semantic
processing may use idle resources; interactive work always wins. Semantic workers run under
cgroup limits and pause on battery/thermal/gaming pressure.
