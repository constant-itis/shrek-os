# Phase 5 — slice 1 — the mount plane (sandbox construction, tier T1)

> **Status: PLAN (pending build-go).** The first real *agent execution* slice: `gatekeeperd`
> constructs one isolation tier for a trivial workload with a **capability-enforced filesystem
> mount-set**, proving `caps ⊆ profile` holds **at construction** — a granted-out path is *absent*
> from the sandbox, not merely unreadable. Egress plane (tap + nftables), other tiers, and the
> `(trust×caps)→tier` selection matrix are **later slices**; this slice stubs tier selection to T1.

## Grounding (researched before scoping — facts, not assumptions)

- **Tier = `systemd-nspawn` (T1).** Packaged on trixie (`systemd-container` 257.13-1~deb13u1).
  Rejected alternatives: **runsc** (in trixie, but *rootless `create` is unsupported* — needs sudo
  throughout, wrong for a constrained broker); **Incus** (local Unix-socket access "always grants
  full access" — no per-request scoping, architecturally wrong for a per-caller broker). nspawn is
  pure namespaces (no nested-`/dev/kvm` dependency), one CLI call yields a cap- **and** mount-bounded
  sandbox, and `gatekeeperd` already holds the `CAP_SYS_ADMIN` construction needs.
- **Mount mechanism = bind mounts, `--bind-ro=HOST:GUEST`.** virtio-fs/virtiofsd is a vhost-user
  microVM mechanism — **strictly T3**, irrelevant here (this corrects the roadmap's "virtio-fs cap
  mounts" phrasing for the T1 slice). Construction needs `CAP_SYS_ADMIN` only; **`CAP_NET_ADMIN` is
  out of scope** (egress slice).
- **fd-pinning = pin → verify → relocate.** Neither nspawn nor runsc accepts an fd-path bind source
  as a documented contract (nspawn re-resolves the `--bind=` string fresh via a symlink-following
  `open_tree` walk — the same TOCTOU class CVE-2019-5736 exploited). So the broker:
  1. `rustix::fs::openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS)` → `O_PATH` fd;
  2. `statx`, record dev/inode;
  3. in a private mount-ns, `mount("/proc/self/fd/N", "/run/shrek/<id>/project", MS_BIND)`;
  4. re-verify dev/inode on the dest;
  5. invoke nspawn against the controlled plain path `/run/shrek/<id>/project` — nothing left to race.
  Use `rustix` (ResolveFlags map 1:1 to the kernel); the thin `openat2` crate is lower quality.
- **Two false-pass traps** — the gate is designed to defeat both:
  1. nspawn `--bind` only *adds* onto the **intact host root**, so `/srv/vault` stays *listable*. The
     broker must stage a **synthetic empty root** and pass it as `--directory=`, binding only the grant
     into it → the denied path is **ENOENT** (absent), the strong result, not EACCES.
  2. nspawn is **not isolated by default** — `--private-users=` is **mandatory**, or the FS test
     passes while UID isolation silently does not hold.

## Gates (M-series)

- **M0 — VERIFY-FIRST on the real image** (nothing below is binding until these pass):
  `dpkg -l systemd-container` ≥ **257.12** (CVE-2026-40226 container-root-via-crafted-config fix);
  nested-virt state (only matters if a later tier wants runsc KVM — noted, not used here);
  `ls /srv` inside a real bound container confirms the synthetic-root design hides siblings;
  QEMU `-serial stdio`/`-nographic` flushes the final line before poweroff; confirm nspawn does not
  accept an fd-path bind source (drives the relocate step).
- **M1 — host/container repro:** nspawn + synthetic root + `--bind-ro` + the caps test, validated on
  the host in seconds, **before** the ~35-min VM cycle. (Same "fast oracle before VM" that carried
  Phase 1/2/4.)
- **M2 — fd-pinning proven TOCTOU-safe:** a symlink/rename swap on the source between pin and mount
  cannot escape the pinned subtree (EAGAIN-retry on race; NO_SYMLINKS/NO_MAGICLINKS enforced).
- **M3 — `gatekeeperd` drives it end-to-end** from a grant (subject, read-caps) → constructed sandbox.
- **M4 — VM GATE (the real proof), on the sealed enforcing-Secure-Boot image:** grant `/srv/project`
  read, deny `/srv/vault`; inside the sandbox assert **(a)** `/srv/project` readable, **(b)**
  `/srv/vault` → **ENOENT**, **(c)** vault absent from the parent `ls`; `--private-users=` enforced.
  Emit one anchored line per gate `SHREK_GATE: PASS gate=<n>` / `FAIL gate=<n> reason=<code>`; host
  matches `^SHREK_GATE: (PASS|FAIL)`; **absence-of-PASS ⇒ FAIL** (timeout fails closed); wrap the
  workload in an **EXIT trap keyed on `$?`** so a crash emits `FAIL reason=crashed`, never silence.
  Assert on exit codes + probed mounted content, **never on console-string presence** (systemd can
  swallow bind-source errors to the console).

## M0 — RESULTS (verified 2026-08-18, privileged `debian:trixie` container oracle)

All M0 checks pass. Findings fold back into the design below.

- **CHECK 1 — PASS.** `systemd-container = 257.13-1~deb13u1` (≥ 257.12 → CVE-2026-40226 fixed).
- **CHECK 2 — noted.** Host nested-virt `kvm_intel/nested=Y`. Unused in T1; matters only if a later
  tier wants runsc-KVM.
- **CHECK 3 — PASS (with a shape correction).** Synthetic root + `--bind-ro=/srv/project`: inside the
  sandbox `ls /srv` shows **only `project`**; `cat /srv/project/hello.txt` → readable;
  `cat /srv/vault/secret.txt` → **ENOENT** (`No such file or directory`), the strong absence result,
  not EACCES. Trap 1 defeated. **CORRECTION:** nspawn refuses a *literally empty* `--directory` root
  — *"doesn't look like it has an OS tree (/usr/ directory is missing). Refusing."* The root must be
  **OS-tree-shaped** (contain `/usr/`); it is the **grant-mount tree (`/srv`) that stays empty**, and
  that is what hides siblings. Read "synthetic empty root" as **"synthetic minimal root: `/usr/`
  present, grant tree empty"** everywhere below.
- **CHECK 3-priv — PASS.** `--private-users=pick` accepted and functional; sibling-hiding still holds
  under UID isolation. (M4 additionally asserts the *host-side* mapped UID is non-0 — the userns view
  of `uid=0` is expected and not by itself proof.)
- **CHECK 5 — PASS.** `--bind-ro=/proc/self/fd/N` → **`mount(... MS_BIND): Invalid argument`**. The fd
  number is process-local; in nspawn's own process it is not a valid fd, so the path re-resolves to
  nothing. Confirms a pinned fd **cannot cross the exec boundary** → the pin→verify→**relocate** step
  is mandatory (mount the `O_PATH` fd's `/proc/self/fd/N` to a controlled plain path *inside the
  broker's own process/mount-ns*, then hand nspawn the plain path).
- **CHECK 4 — satisfied by prior art.** `scripts/boot-vm.sh` already captures serial via `-serial
  file:` and flushed the final console line across the S6/S7 gates. Re-confirmed at M4.

### nspawn-in-container prerequisites (environment, not design)
The container oracle needs, and the M1 repro harness must replicate (they hold natively on the real
Shrek image where gatekeeperd runs under systemd PID1): `/run` as **tmpfs** (else nspawn refuses to
tear down its propagate dir on overlayfs); `mount --make-rshared /`; and **`--keep-unit
--register=no`** so nspawn reuses the current cgroup instead of asking an absent system bus for a
transient scope.

### OPEN — resolve before M2 code
The plan's fd-pinning step names the **`rustix`** crate for `openat2`. The entire control plane is
**dependency-free by design** (`Cargo.toml`: "No dependencies by design"; `gatekeeperd` hand-writes
`SO_PEERCRED` via inline-asm rather than pull even a syscall crate). rustix-vs-hand-roll is a
supply-chain decision for a **sealed, privileged** binary — flagged for a call before writing M2.

## Method (unchanged from Phase 1/2/4)

Researcher-before-build (done). Host/container repro before the VM cycle. Empirical VM gate before
commit. No third-party tooling attributions in-tree. Repo stays unpushed. Commit only after M4 is green.

## Spike-only (strip before ship)

`/srv/project` + `/srv/vault` fixtures, the trivial workload, the `SHREK_GATE:` console harness, any
`--directory=` staging fixture. The tier-selection stub (hardcoded T1) is replaced by the real
`(trust×caps)→tier` matrix in the selection slice.

## Deferred to later Phase-5 slices

Egress plane (per-workload tap + nftables allow-list + authenticated egress proxy / DLP chokepoint,
security-model §7); T2 (gVisor) / T3 (libkrun/microVM + virtio-fs) construction; the deterministic
`(trust×caps)→tier` selection + downward-forbidden floor (ADV-2 trust-band is the high-leverage soft
spot); evdev / `/dev/console` / `/dev/tty0` stripping (grant-protocol.md Sandbox-prerequisites,
agents.md §8 — a trusted-path dependency).
