# Shrek OS — concept to code

> Start with the invariant, then find the enforcing code and proof gate.

This is a developer crosswalk for contributors who want to move from the design
docs to implementation. It is generated from the current code shape and should
be checked against the tree whenever the control plane changes.

## 1. Control-plane map

| Concept | Design docs | Implementation | Proof / acceptance gate |
|---|---|---|---|
| Sealed base | [`architecture.md`](architecture.md), [`update-model.md`](update-model.md) | [`image/`](../image), [`image/mkosi.conf`](../image/mkosi.conf), [`image/mkosi.repart/`](../image/mkosi.repart) | [`scripts/boot-vm.sh`](../scripts/boot-vm.sh), [`scripts/rollback-proof.sh`](../scripts/rollback-proof.sh) |
| Raw A/B updates | [`update-model.md`](update-model.md), [`phase1-s7-sysupdate.md`](phase1-s7-sysupdate.md), [`phase1-s8-rollback.md`](phase1-s8-rollback.md) | [`image/overlay/usr/lib/sysupdate.d/`](../image/overlay/usr/lib/sysupdate.d) | [`scripts/update-in-container.sh`](../scripts/update-in-container.sh), [`scripts/rollback-proof.sh`](../scripts/rollback-proof.sh) |
| Onion layers | [`phase2-onion.md`](phase2-onion.md), [`phase4-oniond.md`](phase4-oniond.md) | [`crates/oniond/src/main.rs`](../crates/oniond/src/main.rs), [`crates/gatekeeperd/src/main.rs`](../crates/gatekeeperd/src/main.rs) | [`scripts/onion-proof.sh`](../scripts/onion-proof.sh), [`scripts/oniond-proof.sh`](../scripts/oniond-proof.sh), [`scripts/gatekeeperd-proof.sh`](../scripts/gatekeeperd-proof.sh) |
| Agent request resolution | [`agents.md`](agents.md), [`isolation.md`](isolation.md) | [`crates/agentd/src/main.rs`](../crates/agentd/src/main.rs), [`crates/shrek-policy/src/tier.rs`](../crates/shrek-policy/src/tier.rs) | [`scripts/tier-plane-proof.sh`](../scripts/tier-plane-proof.sh) |
| Privileged broker | [`phase4-gatekeeperd.md`](phase4-gatekeeperd.md), [`grant-protocol.md`](grant-protocol.md) | [`crates/gatekeeperd/src/main.rs`](../crates/gatekeeperd/src/main.rs), [`crates/gatekeeperd/src/sandbox.rs`](../crates/gatekeeperd/src/sandbox.rs) | [`scripts/gatekeeperd-proof.sh`](../scripts/gatekeeperd-proof.sh), [`scripts/sandbox-proof.sh`](../scripts/sandbox-proof.sh) |
| Trust provenance | [`trust-bands.md`](trust-bands.md), [`phase5-slice7-trust-provenance.md`](phase5-slice7-trust-provenance.md) | [`crates/shrek-policy/src/provenance.rs`](../crates/shrek-policy/src/provenance.rs), [`crates/gatekeeperd/src/provenance_plane.rs`](../crates/gatekeeperd/src/provenance_plane.rs) | [`scripts/b1-provenance-proof.sh`](../scripts/b1-provenance-proof.sh) |
| Static pin manifest | [`phase5-slice8-pin-manifest.md`](phase5-slice8-pin-manifest.md), [`trust-bands.md`](trust-bands.md) | [`crates/gatekeeperd/src/pin_manifest.rs`](../crates/gatekeeperd/src/pin_manifest.rs), [`crates/gatekeeperd/src/provenance_plane.rs`](../crates/gatekeeperd/src/provenance_plane.rs) | [`scripts/pin-manifest-proof.sh`](../scripts/pin-manifest-proof.sh) |
| T0 process sandbox | [`phase5-slice4-t0.md`](phase5-slice4-t0.md), [`trust-bands.md`](trust-bands.md) | [`crates/gatekeeperd/src/proc_plane.rs`](../crates/gatekeeperd/src/proc_plane.rs), [`crates/gatekeeperd/src/linux_uapi.rs`](../crates/gatekeeperd/src/linux_uapi.rs) | [`scripts/t0-construct-proof.sh`](../scripts/t0-construct-proof.sh) |
| T1 mount plane | [`phase5-slice1-mount.md`](phase5-slice1-mount.md), [`trust-bands.md`](trust-bands.md) | [`crates/gatekeeperd/src/mount_plane.rs`](../crates/gatekeeperd/src/mount_plane.rs), [`crates/gatekeeperd/src/linux_uapi.rs`](../crates/gatekeeperd/src/linux_uapi.rs) | [`scripts/mount-plane-repro.sh`](../scripts/mount-plane-repro.sh), then [`scripts/sandbox-proof.sh`](../scripts/sandbox-proof.sh) for broader broker coverage |
| T1 egress plane | [`phase5-slice2-tier.md`](phase5-slice2-tier.md), [`trust-bands.md`](trust-bands.md) | [`crates/shrek-policy/src/egress.rs`](../crates/shrek-policy/src/egress.rs), [`crates/gatekeeperd/src/net_plane.rs`](../crates/gatekeeperd/src/net_plane.rs) | [`scripts/egress-plane-repro.sh`](../scripts/egress-plane-repro.sh), [`scripts/egress-construct-proof.sh`](../scripts/egress-construct-proof.sh) |
| T2 gVisor constructor | [`phase5-slice6-t2.md`](phase5-slice6-t2.md), [`trust-bands.md`](trust-bands.md) | [`crates/gatekeeperd/src/t2_plane.rs`](../crates/gatekeeperd/src/t2_plane.rs), [`image/mkosi.conf.d/30-t2-gvisor.conf`](../image/mkosi.conf.d/30-t2-gvisor.conf) | [`scripts/t2-construct-proof.sh`](../scripts/t2-construct-proof.sh) |
| **Coding-agent enablement (P6-1a):** `shrek run` front door + integrity-bound untrusted-ingest T2 coding session | [`phase6-slice1a-untrusted-ingest.md`](phase6-slice1a-untrusted-ingest.md) | [`crates/shrek/src/main.rs`](../crates/shrek/src/main.rs), [`crates/gatekeeperd/src/t2_plane.rs`](../crates/gatekeeperd/src/t2_plane.rs) | [`scripts/shrek-run-proof.sh`](../scripts/shrek-run-proof.sh), [`scripts/p6-coder-proof.sh`](../scripts/p6-coder-proof.sh), [`mount-plane-gate`](../image/overlay/usr/lib/shrek/mount-plane-gate) (P6) |
| Named egress on the ingest session (P6-1b) | [`phase6-slice1b-egress.md`](phase6-slice1b-egress.md) | [`crates/shrek-policy/src/egress.rs`](../crates/shrek-policy/src/egress.rs), [`crates/gatekeeperd/src/net_plane.rs`](../crates/gatekeeperd/src/net_plane.rs) | [`scripts/p6-egress-proof.sh`](../scripts/p6-egress-proof.sh), [`mount-plane-gate`](../image/overlay/usr/lib/shrek/mount-plane-gate) (P6B) |
| Exec-tmpfs rootfs staging for the session (P6-1c) | [`phase6-slice1a-untrusted-ingest.md`](phase6-slice1a-untrusted-ingest.md) (as-built hardening) | [`crates/gatekeeperd/src/mount_plane.rs`](../crates/gatekeeperd/src/mount_plane.rs) `stage_tmpfs()`, [`crates/gatekeeperd/src/t2_plane.rs`](../crates/gatekeeperd/src/t2_plane.rs) | [`mount-plane-gate`](../image/overlay/usr/lib/shrek/mount-plane-gate) (P6) |
| Coding-agent workload (P6-2) | [`phase6-slice2-coder-agent.md`](phase6-slice2-coder-agent.md) | [`crates/coder/src/main.rs`](../crates/coder/src/main.rs) | [`scripts/p6-coder-agent-proof.sh`](../scripts/p6-coder-agent-proof.sh), [`mount-plane-gate`](../image/overlay/usr/lib/shrek/mount-plane-gate) (P6-2) |
| Model-provider abstraction + broker proxy (P6-3) | [`phase6-slice3-provider-abstraction.md`](phase6-slice3-provider-abstraction.md) | [`crates/coder/src/main.rs`](../crates/coder/src/main.rs) `Provider`, [`crates/model-proxy/src/main.rs`](../crates/model-proxy/src/main.rs), [`crates/shrek-policy/src/egress.rs`](../crates/shrek-policy/src/egress.rs) `model-anthropic` | [`scripts/p6-anthropic-proxy-proof.sh`](../scripts/p6-anthropic-proxy-proof.sh), [`mount-plane-gate`](../image/overlay/usr/lib/shrek/mount-plane-gate) (P6-3) |
| Subscription-model provider via the logged-in CLI (P6-4) | [`phase6-slice4-claude-cli-broker.md`](phase6-slice4-claude-cli-broker.md) | [`crates/claude-broker/src/main.rs`](../crates/claude-broker/src/main.rs), [`crates/shrek-policy/src/egress.rs`](../crates/shrek-policy/src/egress.rs) `model-claude-cli` | [`scripts/p6-claude-cli-broker-proof.sh`](../scripts/p6-claude-cli-broker-proof.sh) (broker-side; oracle is the gate) |
| "Sign in with Claude" login UX — trusted operator path (P6-5) | [`phase6-slice5-claude-login-ux.md`](phase6-slice5-claude-login-ux.md) | [`crates/claude-broker/src/main.rs`](../crates/claude-broker/src/main.rs) `cmd_login`/`cmd_health`/`write_availability_to` | [`scripts/p6-claude-cli-login-proof.sh`](../scripts/p6-claude-cli-login-proof.sh) (broker-side; oracle is the gate) |
| Semantic filesystem boundary — the **upcoming Swamp/semantic Phase-6** track, distinct from the coding-agent enablement rows above | [`filesystem-intelligence.md`](filesystem-intelligence.md), [`swamp.md`](swamp.md), [`security-model.md`](security-model.md) | [`crates/swampd/src/main.rs`](../crates/swampd/src/main.rs) | not shipped past the current scaffold |

## 2. Trust and capability path

The public model is:

```text
agent/workload -> agentd resolve -> gatekeeperd provenance -> gatekeeperd recheck -> constructor
```

Code path:

1. [`crates/agentd/src/main.rs`](../crates/agentd/src/main.rs) parses a request,
   resolves a known profile, and emits the requested trust/capability shape.
2. [`crates/gatekeeperd/src/provenance_plane.rs`](../crates/gatekeeperd/src/provenance_plane.rs)
   derives provenance from sealed evidence. The caller's trust label is not
   authoritative.
3. [`crates/shrek-policy/src/tier.rs`](../crates/shrek-policy/src/tier.rs)
   defines `TrustBand`, `CapsProfile`, `Tier`, `matrix()`, `floor()`, and
   `effective_tier()`.
4. [`crates/gatekeeperd/src/sandbox.rs`](../crates/gatekeeperd/src/sandbox.rs)
   re-checks the resolved request, refuses invalid cells, and dispatches to the
   available constructor.

Useful tests are embedded near the policy code. Look for cases such as
`effective_is_never_below_floor_or_matrix`,
`forged_downgrade_below_floor_is_refused`, `t3_has_no_constructor`, and
`hostile_with_write_or_net_is_always_t3_except_readonly`.

## 3. Constructor path

Each constructor realizes a wall. Capabilities still define the radius.

| Tier | Main code | Important functions / types |
|---|---|---|
| T0 | [`crates/gatekeeperd/src/proc_plane.rs`](../crates/gatekeeperd/src/proc_plane.rs) | `T0Spec`, `build_seccomp_program()`, `install_landlock()`, `sandbox_init_and_exec()`, `construct()` |
| T1 mount | [`crates/gatekeeperd/src/mount_plane.rs`](../crates/gatekeeperd/src/mount_plane.rs) | `open_anchor()`, `pin_beneath()`, `ident_at_path()`, `bind_ro()`, `relocate_ro()`, `construct()` |
| T1 egress | [`crates/gatekeeperd/src/net_plane.rs`](../crates/gatekeeperd/src/net_plane.rs) | `Endpoint`, `SandboxNet`, veth/netns/nftables setup |
| T2 | [`crates/gatekeeperd/src/t2_plane.rs`](../crates/gatekeeperd/src/t2_plane.rs) | `Platform`, `PlatformChoice`, `T2Spec`, `GrantMount`, `select_platform()`, `build_config_json()`, `construct()` |
| T3 | not implemented | requests that require T3 must refuse |

The shared syscall wrapper layer is
[`crates/gatekeeperd/src/linux_uapi.rs`](../crates/gatekeeperd/src/linux_uapi.rs).
This is where wrappers such as `openat2()`, `statx_fd()`, `mount()`,
`landlock_create_ruleset()`, and `seccomp_set_mode_filter()` live.

## 4. Provenance and pins

Trust-band provenance has two layers:

1. The policy lattice in [`crates/shrek-policy/src/provenance.rs`](../crates/shrek-policy/src/provenance.rs).
2. The broker's evidence collector in
   [`crates/gatekeeperd/src/provenance_plane.rs`](../crates/gatekeeperd/src/provenance_plane.rs).

Pinned classification and execution add:

- manifest parsing (v1 static + v2 closure) in [`crates/gatekeeperd/src/pin_manifest.rs`](../crates/gatekeeperd/src/pin_manifest.rs)
- manifest loading, lookup, and closure carry in
  [`crates/gatekeeperd/src/provenance_plane.rs`](../crates/gatekeeperd/src/provenance_plane.rs)
- exec-island / closure-island construction in
  [`crates/gatekeeperd/src/mount_plane.rs`](../crates/gatekeeperd/src/mount_plane.rs) and
  [`crates/gatekeeperd/src/proc_plane.rs`](../crates/gatekeeperd/src/proc_plane.rs)
- proof coverage in [`scripts/pin-manifest-proof.sh`](../scripts/pin-manifest-proof.sh) and
  [`scripts/spike-stripped-proof.sh`](../scripts/spike-stripped-proof.sh)

As-built: a `T-pinned` artifact **executes** — a static PIE from an exact-inode exec island
(slice 9), a dynamically-linked entrypoint from an authenticated N-inode closure island (slice 10).
Every member is re-verified `(dev,ino)`+fs-verity; the writable island root is `MS_NOEXEC` with exec
re-added only per authenticated member. The guarantees and their limits are
[`security-model.md`](security-model.md) §11 (PG3/PG4/PN3/PN5/PN6).

## 5. Maintainer checklist

When changing a subsystem, run the smallest proof gate that covers it:

| If you touch | Run first |
|---|---|
| `crates/shrek-policy/src/tier.rs` | `scripts/tier-plane-proof.sh` |
| `crates/shrek-policy/src/egress.rs` or `crates/gatekeeperd/src/net_plane.rs` | `scripts/egress-plane-repro.sh` and `scripts/egress-construct-proof.sh` |
| `crates/shrek-policy/src/provenance.rs` or `crates/gatekeeperd/src/provenance_plane.rs` | `scripts/b1-provenance-proof.sh` |
| `crates/gatekeeperd/src/pin_manifest.rs` | `scripts/pin-manifest-proof.sh` |
| `crates/gatekeeperd/src/mount_plane.rs` | `scripts/mount-plane-repro.sh`, then `scripts/sandbox-proof.sh` for broader broker coverage |
| `crates/gatekeeperd/src/proc_plane.rs` | `scripts/t0-construct-proof.sh` |
| `crates/gatekeeperd/src/t2_plane.rs` | `scripts/t2-construct-proof.sh` |
| Onion policy or merge routing | `scripts/oniond-proof.sh` and `scripts/gatekeeperd-proof.sh` |

For broader changes, run [`scripts/sandbox-proof.sh`](../scripts/sandbox-proof.sh)
after the targeted gate.

## 6. How this file is maintained

This file should be refreshed from the code graph, then verified against exact
files before commit:

```bash
graphify extract . --code-only --cargo --out /tmp/shrek-graphify
graphify god-nodes --graph /tmp/shrek-graphify/graphify-out/graph.json
graphify query "what code implements trust bands and constructor dispatch?" \
  --graph /tmp/shrek-graphify/graphify-out/graph.json
```

Graph output is only a locator. Before changing this document, hydrate the files
with `rg`/`sed` and verify the proof scripts still exist.
