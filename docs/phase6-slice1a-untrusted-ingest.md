# Phase-6 Slice-1a — integrity-bound untrusted-ingest coding session (as built)

The first end-to-end coding-agent vertical slice on the real Shrek system. One bounded session
proves: an untrusted project is admitted to the `T-untrust` band **only** by an integrity proof, runs
in a genuine `T2` (gVisor/runsc) sandbox with project-scoped write-through, does a **real** edit →
compile → execute loop, and cannot reach home/Vault or the network.

Method unchanged: oracle (`scripts/p6-coder-proof.sh`, privileged `debian:trixie`) → sealed-VM gate
(`image/overlay/usr/lib/shrek/mount-plane-gate`, section `P6`) → commit. Oracle before VM.

## 0. Scope & acceptance

In scope (user BUILD-GO 2026-08-20):

- Integrity-bound `Origin::UntrustedIngest` admission → FROZEN `shrek_policy::derive_band` → `T-untrust`.
- Genuine `T2` gVisor construction for the `(T-untrust, C-proj-rw)` cell.
- A project grant realized **rw + host-`noexec`** (write-through, source stays non-runnable) **and** a
  separate narrowly-scoped **rw + exec** build grant for compiler/test output.
- A real compiler (`tcc`) sealed into the T2 rootfs; a real edit/compile/execute loop.
- The wall: ungranted Vault absent (`ENOENT`), host FS isolated, `--network=none` (no egress).

Explicitly deferred: egress plane (P6-1b); a Rust/cargo toolchain in the rootfs (overlay-rootfs scaling
is a separate delivery track); caller-asserted `Origin`; any change to `derive_band`.

Acceptance = **real compile + execute under the enforcing kernel**: a real ELF compiled from project
source executes from the build grant, while the same ELF cannot execute from the `noexec` project grant.

## 1. Why a coding session must derive `T-untrust` via `UntrustedIngest`

A real coding session is **open-world**: the sandbox execs mutable `build.rs`/proc-macros/test/target
binaries. So it cannot earn `T-first` — `derive_band` (`shrek-policy/src/provenance.rs`) grants `T-first`
only with `entrypoint_sealed AND domain_execution_sealed`, and the domain gate is a CLOSED-WORLD list
(only the sealed gate-probe). The tier matrix (`tier.rs`) then forces the band:

- `(T-untrust, C-proj-rw) = T2`, `floor(T-untrust) = T2` — constructible.
- `(T-hostile, C-proj-rw) = T3` (no constructor) — refused.

`gatekeeperd`'s entrypoint measurement only ever emits `Origin::None` ⇒ `T-hostile` ⇒ coding is
unreachable. So the session must earn `Origin::UntrustedIngest` (⇒ `T-untrust`) through an integrity
proof. That admission is the slice.

## 2. The integrity-bound admission (`crates/gatekeeperd/src/ingest_admit.rs`)

A session earns `Origin::UntrustedIngest` — and thus `T-untrust` rather than the `T-hostile` floor —
**iff the T2 containment harness that will run it is integrity-authentic**: the `runsc` binary's
fs-verity digest is present in a sealed admit-list. The band and the wall are coupled *through
integrity*: you may treat code as merely untrusted (a weaker wall than hostile) precisely because an
authenticated gVisor harness exists to contain it.

- Admit-list: `/usr/lib/shrek/t2-ingest-admit`, grammar `shrek-t2-ingest-admit v1` + `sha256 <hex>`
  lines. Baked under dm-verity `/usr` at image build (`seal-t2-artifacts.sh`); changing it needs a
  signed image update. `SHREK_INGEST_ADMIT` relocates it for the oracle only (same discipline as
  `SHREK_T2_*`).
- `derive_session(runsc)` measures the harness (`measure_verity`); digest ∈ admit-list ⇒
  `Evidence{origin: UntrustedIngest}`, else `Origin::None`. The band is then the FROZEN `derive_band`
  over that one fact — the module never re-implements the lattice.
- Fail-high is total: missing/empty/malformed admit-list, no fs-verity, a measure error, or a digest
  miss ⇒ `Origin::None` ⇒ `T-hostile` ⇒ refused. `sandbox.rs --ingest-harness` selects this arm.

Why fs-verity (not sealed-root residency): `sealed_root_dev()` is verity-only with no oracle positive,
so fs-verity is the oracle-provable anchor (the S6 pattern), keeping full oracle coverage of the risky
writable path.

## 3. The execution model — project `noexec`, separate exec build grant

### 3a. The real-ELF finding (supersedes the shell-script G3)

Earlier slice-1a checkpoints recorded G3 ("in-sandbox exec of a freshly-written project file succeeds
despite host-`noexec`") as **RESOLVED, no invariant change**. That conclusion was measured with a
`#!/bin/sh` **script** stand-in. A script never needs `PROT_EXEC`: the interpreter (`/bin/sh`, on the
exec-capable rootfs) reads the script as *data*. It never tested executing a **binary** from the
`noexec` grant.

The real compiler is the first true test, and it **overturns** that conclusion. Under gVisor's `systrap`
platform, loading an ELF requires the Sentry to `mmap(PROT_EXEC)` the gofer-backed host file, and the
kernel denies that on a `MS_NOEXEC` mount. Isolated empirically (same OCI config, only the host bind
flag changed):

| host bind flag on the grant | result |
|---|---|
| `rw,exec`   | binary runs, `exit 42` |
| `rw,noexec` | `runsc: failed to load /proj/hello: permission denied` (rc 128); via `busybox sh`, `SIGSEGV` rc 139 |

So host-`noexec` on the T2 project grant is genuinely load-bearing (source bytes written there are not
runnable in-sandbox) **and** incompatible with running build output from that same grant.

### 3b. Owner decision (2026-08-20) — the build-grant split

Keep the project rw grant `MS_NOEXEC`; add a **separate, narrowly-scoped rw + exec build grant** for
T2. The workload directs compiler/test **output** to the build area (`CARGO_TARGET_DIR` for a Rust
session; the compiled ELF here for this slice) and runs it there. Prove **both**: a real ELF compiled
from project source executes from the build grant, **and** the same ELF copied into the `noexec` project
grant cannot execute. `NOSUID|NODEV` preserved on both. All existing T-pinned mutable-grant `NOEXEC`
behavior is untouched. The containment wall for T2 is gVisor; the host-`noexec` on the project is
defense-in-depth confining the exec surface to the one build grant.

### 3c. As built

- `mount_plane::relocate_rw` (unchanged): pin → bind-from-fd → re-verify identity → remount
  `rw, NOSUID, NODEV, NOEXEC`. The project grant. Its doc comment is corrected to state the real-ELF
  fact (the `noexec` is load-bearing; a binary written here cannot run in-sandbox).
- `mount_plane::relocate_rw_exec` (new): identical TOCTOU-safe path, final remount `rw, NOSUID, NODEV`
  **without** `MS_NOEXEC`. The build grant.
- `t2_plane::T2Spec` gains `build_grant: Option<String>`; `construct()` pins + relocates it in the same
  private mount ns as the project grant, mounts it `rbind,rw` (the exec difference lives in the host bind
  flags, not `config.json`), and — critically — **detaches every writable bind before `remove_dir_all`**
  in teardown, so the recursive removal can never unlink write-through content from a real inode.
- `sandbox.rs` gains `--build-grant NAME`, threaded into `T2Spec` on the `T2` path.

## 4. Sealing the compiler + the admit-list (`scripts/seal-t2-artifacts.sh`)

Baked into the T2 rootfs (`/usr/lib/shrek/t2-rootfs`) at image build (STAGE 2, where `tcc`/`fsverity`
are apt-installed):

- `tcc` + its dynamic closure: `/usr/bin/tcc`, `/lib64/ld-linux-x86-64.so.2`,
  `/lib/x86_64-linux-gnu/{libc,libm}.so.6` (~3.4 MB). `-nostdlib -static` needs **neither** `libtcc1.a`
  **nor** the tcc include dir (a freestanding raw-syscall `_start` program uses no libc); tcc's internal
  linker means no external `ld`. NOT `tcc -run` (JIT/anon-exec = PN5).
- `coder-src/hello.c`: the freestanding template the workload copies into the project and compiles.
- `cp`, `chmod` added to the busybox applet set.
- `t2-ingest-admit`: the admit-list, whose one entry is the **offline** `fsverity digest
  --hash-alg=sha256 --block-size=4096` of the runsc being sealed. fs-verity digest is content-addressed,
  so this offline bake **equals** the runtime kernel `FS_IOC_MEASURE_VERITY` (verified: baked digest ==
  the oracle's kernel-measured runsc digest == `264464ce…0492`). This is the same "offline bake ==
  kernel measure" property S6 proves for the pin-manifest.

## 5. Coverage — oracle & VM gate

Both drive the identical release `gatekeeperd` and the same coding loop.

- **Oracle** `scripts/p6-coder-proof.sh` (fast, before the ~35–40 min VM): provisions a genuine ext4
  `-O verity` runsc harness, runs `--ingest-harness --rw-grant project --build-grant build`, asserts
  G1–G5 + REAL-COMPILE-OK. **ALL PASS.**
- **VM gate** section `P6`: the sealed root's runsc is block-level dm-verity (no per-file digest), so —
  mirroring S6/S8 — the gate provisions a dedicated ext4 `-O verity` loopback, copies the sealed runsc
  onto it, enables fs-verity, and points the substrate at it via `SHREK_T2_RUNSC` (spike-only). The
  **baked** admit-list authenticates it (offline bake == kernel measure). Rootfs = the sealed
  `/usr/lib/shrek/t2-rootfs` (tcc baked in; no `SHREK_T2_ROOTFS` override). FAIL, never skip.

Gates asserted (anchored `SHREK_GATE:` lines; probed content + exit codes, never console strings — the
M4 lesson; the EXIT trap fails closed on absence-of-verdict):

- **G1** admission ⇒ `derived=T-untrust`, `harness_authentic=true`, `construct-at=T2`.
- **REAL-COMPILE-OK** `tcc` built an ELF from freshly-written project source; it **ran from the exec
  build grant** and controlled its exit (`exit=42`).
- **G3 project noexec ENFORCED** the same ELF copied into the project grant **cannot** execute
  (judged on the run marker, not the exit code — the ELF's own success exit is itself nonzero, so a leak
  cannot hide); no exec leak.
- **G2** write-through on **both** inodes (edited source on project, ELF on build) + teardown
  non-destructive (pre-existing README survives).
- **G4** Vault `ENOENT`, host FS isolated, no egress, no leak markers.
- **G5** with the admit-list admitting nothing, the SAME request ⇒ `T-hostile` ⇒ `T3` ⇒ refused below
  floor (rc 10/12); never constructed, never ran, no host write. The one cell flips construct→refuse
  purely on harness integrity.

## 6. Tier

From the matrix, not invented: `T-untrust · C-proj-rw · effective T2` (systrap). `(Untrust, ProjRw)=T2`,
`floor(Untrust)=T2`.

## 7. Non-guarantees / parked

- Egress plane deferred to **P6-1b**.
- Rust/cargo toolchain in the sealed rootfs is a separate delivery track (overlay-rootfs scaling: a
  ~1 GB toolchain vs the per-construct `cp -a` into a 512 MB-`mem_max` tmpfs).
- The gate's `SHREK_T2_RUNSC` / `SHREK_INGEST_ADMIT`(G5) env relocations + the `P6` gate block are
  spike-only — strip before ship, with `mount-plane-gate`, `shrek-mount-gate.service`, and the
  `multi-user.target.wants` symlink.
- Unchanged parked forks: slice-10 v2 dlopen (PN3); `≥T1` pinned containment (PN4);
  `F-M4` VM-confirm; anon JIT `PROT_EXEC` governed by classification not runtime.
