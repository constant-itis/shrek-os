# Shrek OS — trust bands and sandbox constructors

> Trust selects the wall. Capabilities select the radius. Neither is allowed to rewrite the other.

This document is the outside-facing map of the trust-band model. The implementation source is [`crates/shrek-policy/src/tier.rs`](../crates/shrek-policy/src/tier.rs); the full design spec is [`isolation.md`](isolation.md).

## 1. Axes

Trust is about the code being executed:

| Band | Meaning | Floor |
|---|---|---|
| `T-first` | first-party or Shrek-signed code | T0 |
| `T-pinned` | third-party but pinned and vetted at a known hash | T0 |
| `T-untrust` | unreviewed downloaded/cloned code | T2 |
| `T-hostile` | adversarial by assumption, generated code, unknown plugins | T2 |

Capabilities are about the reachable authority:

| Caps | Meaning |
|---|---|
| `C-ro-nosec` | read-only, no secret domains, no network |
| `C-proj-rw` | project-scoped read/write intent, no secrets, no network |
| `C-net` | any narrower profile plus named network egress |
| `C-broad` | broad home visibility, secret-domain access, or unrestricted egress |

Unknown trust evidence derives to `T-hostile`; unknown caps labels parse to `C-broad`. That is fail-high: uncertainty can only raise the wall or force refusal. `--trust` is not authoritative at the broker. Since Phase-5 slice 7, `gatekeeperd` derives the trust band from sealed provenance evidence and treats caller labels as audit input.

## 2. Matrix

The matrix is total over all 16 cells:

```
              C-ro-nosec     C-proj-rw      C-net          C-broad
            ┌──────────────┬──────────────┬──────────────┬──────────────┐
 T-first    │ T0           │ T0           │ T1           │ T1           │
 T-pinned   │ T0           │ T1           │ T1           │ T2           │
 T-untrust  │ T2           │ T2           │ T2           │ T3           │
 T-hostile  │ T2           │ T3           │ T3           │ T3           │
            └──────────────┴──────────────┴──────────────┴──────────────┘
```

Effective tier is:

```
effective_tier = max(matrix(trust, caps), floor(trust), explicit_escalation)
```

There is no downward override. A policy or operator may request a stronger wall, never a weaker one.

## 3. Constructors

| Tier | Constructor | Shipped state |
|---|---|---|
| T0 | `proc_plane`: Landlock + seccomp + user/mount/pid/net/uts/ipc/cgroup namespaces + cgroups v2; no rootfs; grant paths stay at real anchor paths | shipped for the three T0 cells |
| T1 | `sandbox` + `mount_plane`: `systemd-nspawn`, synthetic OS-shaped root, read-only relocated grant binds, private users, private network | shipped |
| T1 egress | `net_plane`: private netns, veth, sealed named egress profile, pre-resolved IPv4 A records, nftables allow-list, ready barrier | shipped for `C-net` cells that remain <= T1 |
| T2 | `t2_plane`: gVisor `runsc` userspace kernel, generated OCI bundle, sealed runtime artifacts, no-network profile | shipped for no-network T2 cells |
| T3 | libkrun/Firecracker/Kata microVM | designed, not constructed yet |

The broker fails closed for any tier or cap combination it cannot construct. Today T3 refuses, and T2 refuses for `C-net` because the gVisor egress plane is deferred. It never silently downgrades to T1.

## 4. T0 specifics

T0 is not "no sandbox." It is a process sandbox:

```
Landlock + seccomp + full namespace set + cgroups v2
```

It has no synthetic rootfs. The workload runs against host `/usr` read-only and the granted paths in place. Landlock handles every supported filesystem right and re-allows only `/usr`, minimal device nodes, and pinned grants. Ungranted paths fail with `EACCES`.

Shipped T0 limitations are deliberate:

- `C-proj-rw` write realization is deferred; grants are read-only today.
- container-root maps to real root; subuid range privilege drop is deferred.
- seccomp uses a curated deny-list and does not yet filter `clone`/`clone3` arguments.

Source: [`phase5-slice4-t0.md`](phase5-slice4-t0.md), [`crates/gatekeeperd/src/proc_plane.rs`](../crates/gatekeeperd/src/proc_plane.rs).

## 5. T1 specifics

T1 uses `systemd-nspawn`. The important property is the synthetic root:

- `/usr` is bound read-only as the runtime.
- the guest grant tree starts empty.
- only broker-relocated grants are bound into the tree.
- ungranted siblings are absent (`ENOENT`), not merely unreadable.
- `--private-users=pick` and `--private-network` are mandatory.

The mount source is not a path string trusted across time. `gatekeeperd` pins the granted object with `openat2`, records its identity with `statx`, relocates from `/proc/self/fd/N`, re-verifies, then bind-mounts read-only. Source: [`crates/gatekeeperd/src/mount_plane.rs`](../crates/gatekeeperd/src/mount_plane.rs).

## 6. T2 specifics

T2 is the gVisor userspace-kernel wall. Shrek drives `runsc` directly from `gatekeeperd` using an OCI bundle. The shipped constructor supports no-network cells: grants are mounted into the bundle, ungranted siblings are absent, and network is `none`. Platform selection prefers systrap in nested/VM contexts and KVM only where a direct probe says it is usable. Source: [`phase5-slice6-t2.md`](phase5-slice6-t2.md), [`crates/gatekeeperd/src/t2_plane.rs`](../crates/gatekeeperd/src/t2_plane.rs).

Deferred T2 work:

- egress for `C-net`;
- general rootfs supply and pooling;
- write realization, matching the T0/T1 write deferral.

## 7. T3 status

T3 is part of the design matrix, but its constructor is not shipped. This is a security property, not a convenience gap: a workload requiring T3 refuses until that constructor exists. The broker does not run it at T2 or T1 just because those tiers are available.

T3 is where virtio-fs belongs. T0/T1 use Landlock/bind-mounted grants; the older shorthand "virtio-fs capability mounts" should be read as the microVM mechanism, not the generic mount plane.

## 8. Trust-band provenance

The tier matrix is only meaningful if the band is not caller-asserted. Slice 7 closed that gap: `gatekeeperd` measures the workload entrypoint against sealed roots and derives evidence through [`crates/shrek-policy/src/provenance.rs`](../crates/shrek-policy/src/provenance.rs). A sealed, closed-world Shrek binary may derive `T-first`. Missing, unverifiable, mismatched, or open-world evidence derives `T-hostile`.

Slice 8 adds a sealed static pin-manifest for `T-pinned` classification (fs-verity digest vs a baked manifest). Slices 9 and 10 then give a pinned artifact an execution home: a static PIE runs from an exact-inode T0 **exec island** (slice 9), and a dynamically-linked entrypoint runs from an authenticated **N-inode closure island** — entrypoint + `PT_INTERP` + transitive `DT_NEEDED`, each re-verified `(dev,ino)`+fs-verity (slice 10). The writable island root is `MS_NOEXEC` with exec re-added only per authenticated member; any drift/tamper fails closed. The guarantees and their limits are the execution contract in [`security-model.md`](security-model.md) §11. See [`phase5-slice7-trust-provenance.md`](phase5-slice7-trust-provenance.md), [`phase5-slice8-pin-manifest.md`](phase5-slice8-pin-manifest.md), [`phase5-slice9-pin-exec-home.md`](phase5-slice9-pin-exec-home.md), and [`phase5-slice10-sealed-dynamic.md`](phase5-slice10-sealed-dynamic.md).
