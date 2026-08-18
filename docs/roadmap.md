# Shrek OS — Roadmap (revised)

Revised from the original 11-phase plan to reflect the two-track decision: the product is
built on **bootc**, not LFS. LFS work moves to a frozen research track that runs *in
parallel and does not block* the product.

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
`threat-model.md` (assets, adversary catalog, trust boundaries, attack narratives) done.
Still to write: `security-model.md`, `filesystem-intelligence.md`, `update-model.md`,
`swamp.md`, `agents.md`.
**Milestone:** Shrek Architecture v0.1 frozen.

### Phase 1 — Hardened Debian bootc base (+ base acceptance test)
Produce a Shrek image from **Debian Stable via mkosi**: hardened config, AppArmor enforcing,
UKI (systemd-stub, Shrek key), **dm-verity** sealed root, MOK-enrolled Secure Boot, empty
Shrek control-plane scaffold, delivered via **bootc**.
**Acceptance test (breaks the base tie):** clean → stay on Debian+bootc; janky → fall back
to `mkosi` + `systemd-sysupdate`; blocked → build a Fedora oracle and port back (cheap via
mkosi). Study a stock Fedora bootc image in a VM first as the reference.
**Milestone:** Shrek boots *sealed* on bare metal + VM under MOK; a broken update rolls back.

### Phase 2 — The Onion (sysext layers)
Build `graphics`, `desktop`, `dev`, `gaming`, `ai` as signed, Verity-authenticated sysext
images (confext for `/etc`). Establish the delivery routing rule (image vs sysext vs
Flatpak). Compatibility metadata + signatures + activation/deactivation/dependency mgmt —
but layering itself is `systemd-sysext`, not our code.
**Milestone:** Shrek's visible OS composes from independently managed, signed layers.

### Phase 3 — Transactional boot + recovery hardening
Lean on bootc's transactional model; add health validation, staged updates, recovery mode.
**Milestone:** Deliberately destroy an update → automatic return to the last good system.

### Phase 4 — Rust control plane
`shrekctl`, `oniond` (policy/orchestration over sysext), `gatekeeperd` (privileged broker).
Establish IPC conventions, structured audit logs, privilege separation, policy format,
daemon security profiles.
**Milestone:** Routine privileged ops no longer need arbitrary root.

### Phase 5 — Isolation runtime
Wire the four-tier `shrek run --trust=<tier> --caps=<profile>` model: Tier 0 (Landlock/
seccomp), Tier 1 (nspawn/Incus), Tier 2 (**gVisor**), Tier 3 (libkrun/Kata). Build the
plumbing the one-liner hides: virtio-fs capability mounts, per-VM tap + nftables egress,
signed minimal rootfs supply, pooled cold-start.
**Milestone:** the same workload runs at any tier by flag; caps are enforced at the mount/
net layer independent of tier.

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
