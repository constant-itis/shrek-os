# Phase-6 Slice-1b — Named Egress for a T2 Untrusted-Ingest Coding Session

Status: BOUNDARY / DESIGN (BUILD-GO, bare-metal-conditioned). No core code yet.
Predecessor: P6-1a (untrusted-ingest T2 coding session, `--network=none`), HEAD `eeb9bd8`.

## 1. Scope

Give a **(T-untrust, C-net) = T2** coding session exactly the egress it legitimately
needs — the crate registry / git remote named by a **sealed** `EGRESS_PROFILE`
(`rust-crates`, `github-https`) — and nothing else. Everything unnamed stays dropped;
`C-broad` stays refused. This is the P6-1a session (gVisor/runsc, project rw+NOEXEC
grant + build rw+EXEC grant) with today's `--network=none` replaced by a governed,
deny-by-default egress plane.

## 2. The runsc-vs-netns decision (the predicted blocker — resolved)

The T1 vs T2 asymmetry, confirmed in code:

- **T1 (nspawn)** is forced into *late-attach*: `sandbox.rs` — `--private-users` cannot
  join a host-owned netns (EPERM, ref #2563/C2). nspawn OWNS the netns; `net_plane::inject`
  attaches AFTER start via `discover_leader` + `ip netns attach <leader>`, gated by the
  `EGRESS_BARRIER`.
- **T2 (runsc)** has no such constraint. runsc is the OCI runtime; it joins whatever netns
  the OCI `config.json` names. Today `build_config_json` emits `namespaces:[pid,mount,ipc,uts]`
  — no network namespace — and `--network=none` governs (loopback only).

So T2 does the thing T1 can't: **pre-create the netns, wire veth+addr+route+nft into it
BEFORE runsc boots, then hand runsc that netns** (the standard CNI+gVisor pattern).

**Decision: `--network=sandbox` (gVisor netstack) into a pre-created named netns. Not
`--network=host`.**

- netstack programs itself from the veth present in the netns at boot → the guest's network
  syscalls hit netstack, never host sockets. That userspace-TCP/IP wall is the point of T2;
  `--network=host` would discard it.
- The egress boundary is unchanged from slice-3: it lives at the HOST-side veth peer + the
  per-sandbox `nft` table (`saddr cont_ip` allow-endpoints → drop; masquerade in root netns).
  That boundary is independent of the guest's internal stack — netstack and the slice-3 nft
  plane COMPOSE rather than conflict.
- Net: T2 egress is structurally simpler than T1 — no `discover_leader`, no post-start
  barrier race. Pre-create → wire → boot runsc joined to it.

### Wiring

1. `net_plane`: add a pre-spawn variant — `ip netns add <ns>` (we own it) instead of
   `netns attach <leader>`. Everything else reused verbatim: `SandboxNet::for_id`,
   `ruleset()`, `resolve_profile_v4` (fail-closed A-record pin), `teardown()`.
2. `t2_plane::build_config_json`: add `{"type":"network","path":"/run/netns/<ns>"}` to the
   namespaces array; OCI-bind a gatekeeper-written `/etc/hosts` (pinned IPs, NO DNS egress —
   sealed rootfs is RO, so hosts arrives as a mount); launch `--network=sandbox`.
3. `T2Spec`: add an `egress: Egress` field (mirror `SandboxSpec.egress`); fail-closed
   teardown of netns+veth+nft on any construct error.

**The one empirical risk the oracle must retire:** that gVisor netstack correctly picks up
the pre-created veth's addr + default route and egresses through the nft filter. Standard
CNI+gVisor says yes; we prove it, not assume it.

## 3. Matrix (already correct — no `tier.rs` change)

`tier.rs`: `matrix(Untrust, Net) = T2`, `floor(Untrust) = T2` → `effective = T2`.
`matrix(Untrust, Broad) = T3` → refused. The cell and floor are already defined; P6-1b
writes zero policy.

The actual gate is one refusal in `recheck()` that P6-1b removes:

```rust
// the exact line P6-1b flips:
if effective == Tier::T2 && matches!(caps, CapsProfile::Net) {
    return Decision::Refuse { code: 12, reason: "no-gvisor-egress-plane-for-C-net-at-T2 (slice-6)" };
}
```

`recheck` already resolves the sealed profile and returns `Egress::Profile(p)` for C-net;
the T2 branch currently ignores `e` and logs `egress=none`. A named-egress T2 session
becomes constructible the moment that refusal is lifted and `e` is threaded into `T2Spec`,
while C-broad stays refused (T3 gate rc12 / the `Broad => rc13` arm — untouched). CLI
already coexists: `--ingest-harness --rw-grant --build-grant --egress-profile` are all
present; only the constructor drops egress.

## 4. Bare-metal safety (the acceptance bar)

The bare-metal difference is ONLY the gVisor platform: `select_platform()` picks KVM when
authoritatively not-virtualized AND the KVM probe succeeds, else systrap. Platform governs
syscall interception (KVM = hardware ring / `/dev/kvm`; systrap = seccomp-trap). It does NOT
touch the packet path:

- The egress boundary (netns/veth/addr/route/nft/ip_forward/masquerade) is host-kernel
  plumbing laid down BEFORE runsc boots — platform-independent by construction.
- `--network=sandbox` (netstack) sits above whichever platform and emits out the veth.
  netstack ⊥ platform in runsc.

So the packet path is identical on KVM and systrap. HONEST GAP: the whole proof chain is
virtualized (oracle nested → systrap; sealed VM virtualized → systrap), so KVM is inferred,
never executed — pre-existing since P6-1a (T2 itself), not introduced by egress. Retire it
with a **bare-metal smoke**: after oracle + sealed-VM green, one run on a KVM-capable host
asserting `select_platform → kvm` in the argv AND egress reach/deny/teardown under KVM.
Any KVM-path failure would be in runsc's KVM sentry, not the egress plane; fail-closed
teardown drops a broken session to no-network, never leaks.

## 5. Method (unchanged: boundary → review → build → oracle → VM → bare-metal smoke)

- Oracle first: an `egress-construct-proof.sh`-style script under privileged `debian:trixie`
  driving the real release gatekeeperd against a hermetic egress-target netns, proving:
  `rust-crates`/`github-https` reach the pinned endpoint from inside gVisor; an unlisted dest
  drops; `--network=sandbox` netstack confirmed in the runsc argv; unknown-profile rc13;
  C-broad rc13; teardown leaves zero residual netns/veth/nft.
- Sealed VM (~35-40min): new P6-1b gate section mirroring P6/S3 in `mount-plane-gate`
  (hermetic egress target reachable from a genuine T2 session; deny + teardown asserted;
  0 raw `SHREK_GATE:FAIL`).
- Bare-metal smoke: §4 — the run that actually retires "works on bare-metal."
