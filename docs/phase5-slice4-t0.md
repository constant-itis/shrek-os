# Phase-5 slice-4 — the genuine T0 (Landlock) process-sandbox constructor

Slice-2 taught gatekeeperd to *decide* the tier; slice-3 added the T1 egress plane. But three cells
of the matrix resolve to **T0** — `(T-first, C-ro-nosec)`, `(T-first, C-proj-rw)`, `(T-pinned,
C-ro-nosec)` — and both prior slices *escalated* them to the T1 nspawn constructor, with the standing
excuse in `recheck()`: *"a T0 result at T1 is a legal upward escalation until the real T0 (Landlock)
constructor lands."*

Slice-4 is that constructor. Those three cells now build at **genuine T0**: Landlock + seccomp +
namespaces (user/mount/pid/net/uts/ipc/cgroup) + cgroups-v2, with **no rootfs** — the workload runs
against the host `/usr` (read-only; dm-verity in the shipped image) and the granted paths in place,
contained by the LSM and the syscall filter rather than a remapped mount tree.

## What this slice is (and is NOT)

**In scope:** `crates/gatekeeperd/src/proc_plane.rs` (the constructor), the Landlock/seccomp/prctl
ABI in `linux_uapi.rs`, the `sandbox.rs` dispatch (`effective==T0 → proc_plane`), the oracle proof,
and the sealed-VM gate.

**Out of scope (deferred, by explicit decision):**
- **Write realization.** Grants mount/allow **read-only** in slice-4 — matching the T1 mount plane,
  which is still `bind-ro`. Actual `C-proj-rw` write is a later slice for **both** tiers.
- **Privilege-dropping userns.** slice-4 maps container-root → real root (single-line `0 0 1`, the
  only map a self-unshared process may write). Mapping container-root to an unprivileged host subuid
  range needs the runc-style *parent-writes-the-child-map* handshake — a documented hardening
  follow-up. The real T0 wall is Landlock + seccomp; the userns is a boundary, not yet a drop.
- **clone-flag seccomp filtering.** The deny-list does **not** block `clone`/`clone3` (they are
  load-bearing for normal process/thread creation). Restricting `CLONE_NEWUSER` via seccomp argument
  inspection (so a workload cannot spawn a fresh userns to regain caps) is a follow-up; because of
  that gap the seccomp proof case is `mount`, never nested-userns.
- T2/T3 constructors; the allow-list source seal (already effectively sealed — compiled into
  `shrek-policy`, shipped under dm-verity); trust-band integrity-sourcing (OPEN B1).

## The fold being removed — `sandbox.rs recheck()`

`effective_tier == T0` iff `matrix == T0` **and** `floor == T0` **and** no operator escalation ≥ T1:

| cell | matrix | floor | effective | before slice-4 | after |
|---|---|---|---|---|---|
| T-first / C-ro-nosec | T0 | T0 | **T0** | built at T1 | **T0** |
| T-first / C-proj-rw | T0 | T0 | **T0** | built at T1 | **T0** |
| T-pinned / C-ro-nosec | T0 | T0 | **T0** | built at T1 | **T0** |
| T-pinned / C-proj-rw | T1 | T0 | T1 | T1 | **T1** (unchanged) |
| any operator escalation ≥ T1 | — | — | ≥T1 | ≥T1 | **≥T1** (unchanged) |

No `C-net` cell is ever T0 (`matrix(First,Net)=T1`), so the T0 constructor **never touches the
veth/nft egress plane** — egress is always `None`, a fresh loopback-only netns.

## The constructor — `proc_plane.rs`

Filesystem caps are enforced by a **Landlock ruleset** that handles every supported FS access class
(so everything unlisted is denied) and re-allows only: `/usr` (exec+read), a minimal `/dev`
(`null`/`zero`/`full` rw, `urandom`/`random` ro — best-effort, a missing node only tightens the
sandbox), and each **pinned grant** (read). The grant is pinned with the same TOCTOU-safe
`openat2(RESOLVE_BENEATH|NO_SYMLINKS)` the mount plane uses, and the pinned **`O_PATH` fd** — not any
path string — is handed to Landlock as the rule's `parent_fd`. Ungranted paths fail **EACCES** (T0's
deny) rather than T1's **ENOENT** (absent-from-mount).

**Access path differs from T1.** Because there is no mount remap, the workload reads a grant at its
**real anchor path** (e.g. `<anchor>/project`), *not* at T1's `guest_prefix` (`/srv/project`). A
caller that wants a stable in-sandbox path is the reason T1 remaps; at T0 the grant stays where it
is. (This bit the first VM gate: its probe read the T1 `/srv/...` path and saw ENOENT.)

Construction is **two forks**, an ordering the kernel forces:

```
gatekeeper (host root)  ── creates the cgroup-v2 leaf + memory.max/pids.max (needs host cgroupfs write)
  └─ P1 (host root)     ── joins the cgroup, THEN unshare(user|mnt|pid|net|uts|ipc|cgroup) + id-maps
       └─ P2 (pid 1)    ── scrub inherited fds → Landlock → no_new_privs → restrict_self → seccomp → execve
```

- **cgroup before userns.** After `unshare(CLONE_NEWUSER)` a process can no longer write host
  cgroupfs, so the leaf is created (and P1 joins it) while still host-root. The daemon moves itself
  to a `_daemon` leaf first so its base cgroup has no internal process and can delegate the
  controllers (the cgroup-v2 "no internal process" rule). In production a systemd `Delegate=yes`
  service cgroup supplies the base; the oracle/VM stand one up explicitly.
- **fork for pidns.** `unshare(CLONE_NEWPID)` does not move the caller into the new pid namespace —
  only its first child becomes PID 1 there. Hence P2.
- **single-line id-map.** A self-unshared userns may only write the single self-map `0 0 1` (a range
  needs CAP_SETUID in the parent, which the unshare drops).

### Fail-closed invariants

1. **No unconfined fallback, ever.** If any wall — cgroup, unshare, id-map, fd-scrub, Landlock,
   seccomp, exec — fails, construction aborts with **no workload run** and no weaker path.
2. **Fall-up only at clean preflight.** Landlock-unavailable / insufficient-ABI is decided **before**
   construction begins (`proc_plane::preflight` probes the Landlock ABI). Only there may the cell
   fall **up** to the stronger T1 nspawn wall — a legal upward escalation, loudly audited, never a
   downgrade. Any failure **after** `construct` starts fails closed; there is no mid-build fall-up.
3. **Rules-before-usable.** Every wall is live before the first workload instruction. Landlock and
   seccomp survive `execve` (with `NO_NEW_PRIVS` set); there is no window.
4. **Inherited-FD scrub is mandatory.** Landlock governs only opens that happen *after*
   `restrict_self` — a descriptor inherited from the privileged parent (esp. an `O_PATH` dir fd) is
   exempt and would be an escape hatch. P2 `close_range(3, ~0)` before opening anything; the
   ruleset-building fds it then opens are all `O_CLOEXEC` and vanish at `execve`.
5. **Same caps property as T1, LSM-enforced.** Granted path readable; ungranted sibling + secret path
   denied — now by Landlock (EACCES), not mount absence.
6. **No new authority source.** The ruleset derives from the same sealed grant set + compiled-in
   policy; no writable config (nothing new to seal).

## Gates

- **Oracle** (`scripts/t0-construct-proof.sh`, privileged `debian:trixie`, the real release binary;
  the pure policy/ABI logic is covered by unit tests): **G1** a T0 cell constructs at genuine T0 —
  decision says `construct-at=T0`, and inside the sandbox the grant is readable, the vault is
  Landlock-denied, `mount` is seccomp-EPERM, and the fresh netns has no egress; **G2** `T-pinned/
  C-proj-rw ⇒ T1` still routes to nspawn; **G3** a ≥T2 requirement still fails closed. (`NO_NEW_PRIVS`
  is implied — seccomp cannot install without it. Landlock deliberately denies the sandbox its own
  `/proc`+`/sys`, so probes stay in-band.)
- **Sealed VM** (`image/overlay/usr/lib/shrek/mount-plane-gate`, section **S4**): re-asserts the same
  properties on the shipped kernel + dm-verity `/usr` — the one place that proves Landlock actually
  **enforces** on the target (a container's kernel cannot guarantee it). gatekeeperd runs in a
  `systemd-run --scope -p Delegate=yes` scope so its cgroup self-management has a clean base. The
  `10-hardening.conf` cmdline pins `lsm=…,landlock,…` so Landlock is active at boot; if it were
  disabled, preflight would fall the cell up to T1 and the S4 `construct-at=T0` assertion would FAIL
  (surfacing the regression rather than hiding it).

Method (unchanged): host/container oracle before the ~35-min VM cycle; empirical VM gate before
commit; no unconfined fallback anywhere.

## Spike artifacts to strip before ship

Unchanged from slice-3 (the T0 work adds only the S4 section to the existing gate, no new spike
files): `image/overlay/usr/lib/shrek/mount-plane-gate` (now M4+S2+S3+**S4**),
`shrek-mount-gate.service`, the `multi-user.target.wants` symlink, and `90-vm-acceptance.conf`.
`scripts/t0-construct-proof.sh` is spike-only like its slice-1/2/3 siblings.
