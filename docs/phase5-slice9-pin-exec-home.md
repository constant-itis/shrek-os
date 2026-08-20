# Phase-5 Slice-9 (provisional) — T-pinned execution-home: boundary & research

**Status: BUILT (2026-08-19). Host oracle green on genuine fs-verity; sealed-VM S7 gate added.**
**Forks decided (2026-08-19): Fork A = static-PIE-only; Fork B = host at T0; Fork C = settled.**
Successor problem to slice-8 (`docs/phase5-slice8-pin-manifest.md` §10). Slice-8 is frozen and
shipped (classification-only). This document scopes the *next, separately-reviewed* boundary the
slice-8 doc deferred: giving a `T-pinned` artifact an executable home without reopening the
fail-closed `MS_NOEXEC` posture for anything else.

> **Correction (2026-08-19, per boundary review):** an earlier draft claimed
> `LANDLOCK_ACCESS_FS_EXECUTE` governs `mmap(PROT_EXEC)` / shared-library loading. That is **not**
> relied upon — landlock(7) documents the right only as "Execute a file" and the mmap behaviour is
> not authoritatively established. The mechanism that blocks file-backed executable mappings of
> mutable bytes is **`MS_NOEXEC`**, which is documented: mmap(2) `EPERM` — *"The prot argument asks
> for PROT_EXEC but the mapped area belongs to a file on a filesystem that was mounted no-exec."*
> `MS_NOEXEC` therefore covers **both** `execve` and file-backed `mmap(PROT_EXEC)`. Landlock EXECUTE
> is used **only** to gate the entrypoint's `execve` (allow exactly the pinned inode). §1–§3 below
> reflect this.

Numbering is provisional (slice-5 is reserved for transport); the file may be renamed on build-go.

---

## 1. The problem, precisely

Today `gatekeeperd sandbox` with a `T-pinned` derivation **refuses deterministically**:
`SANDBOX-DECISION refused reason=pinned-exec-home-unavailable` (rc=15), no up/down constructor
(`sandbox.rs:379`). The refusal is correct, not a stub: a pinned artifact has **no exec-capable,
integrity-guaranteed place to run**.

Why — the two independent gates that block it (both verified this session):

- **T0 (`proc_plane.rs`):** `grant_access()` (line 89) grants `READ_FILE` (+`READ_DIR`) but **no
  `LANDLOCK_ACCESS_FS_EXECUTE`**. Only `usr_access()` (line 83) carries EXECUTE, and only `/usr`
  (dm-verity sealed) gets it. A pinned third-party inode is not on `/usr`, so Landlock denies its
  execution. T0 is otherwise **mount-free** — grants stay at their anchor, governed purely by
  Landlock.
- **T1 (`mount_plane.rs`):** `relocate_ro()` remounts every grant `…|MS_NOEXEC` (line 149); only
  `bind_ro()` (the `/usr` OS-tree bind, line 111) omits `NOEXEC`. So a pinned inode bound as a grant
  is `noexec` at the VFS level.

Kernel facts that frame the solution (primary sources — mmap(2), execve(2), landlock(7) — this
session):

- **`MS_NOEXEC` blocks file-backed executable mappings, not just `execve`.** mmap(2): `EPERM` — *"The
  prot argument asks for PROT_EXEC but the mapped area belongs to a file on a filesystem that was
  mounted no-exec."* So a `noexec` mount stops **both** `execve` of a file **and** any
  `mmap(PROT_EXEC)` of it (i.e. loading it as a shared library). This — not Landlock — is the
  instrument that keeps mutable bytes from becoming instructions on grant/writable mounts. It is the
  load-bearing enforcement for I2, and is exactly the posture I5 keeps intact.
- **Landlock is restrictive-only and gates `execve`.** landlock(7): a right named only "Execute a
  file"; and *"a sandboxed thread can only access a file path if all its enforced policy layers grant
  the access **as well as all the other system access controls** (e.g., filesystem DAC, other LSM
  policies, etc.)."* Landlock cannot re-grant exec on a `noexec` mount. In this design Landlock
  EXECUTE is scoped to the **island inode** solely to gate *which file the entrypoint may `execve`* —
  it is **not** relied on to govern `mmap(PROT_EXEC)`.
- **Anonymous `PROT_EXEC` (JIT) is governed by neither `noexec` nor Landlock** — it is not
  file-backed. It is excluded upstream by the **closed-world manifest class**: a JIT/interpreter is
  open-world and can never earn `T-pinned` (slice-7 §5.1). The home relies on that classification, not
  a runtime memory control.
- **An exec home needs a non-`noexec` mount AND an `execve`-permitting Landlock rule** — both gates
  are independent and both must pass.
- **A bind mount preserves inode identity (`st_dev`,`st_ino`) and its fs-verity property**;
  fs-verity re-verifies content against the Merkle root on **every** read/mmap fault, so a
  verity-enabled inode is immutable and self-authenticating wherever it is bound. (`relocate_ro`
  already relies on identity-preservation-across-bind for its TOCTOU re-check.)
- **Single-file (non-directory) Landlock rules work** — proven in-tree: `install_landlock` already
  adds per-file rules for `/dev` nodes via `open_opath` (non-dir `O_PATH` parent_fd).

## 2. What an execution-home MUST satisfy (acceptance invariants)

Any option is judged against these. They are hard; a miss on any is a rejection.

- **I1 — Exact-inode/digest binding.** Execution is cryptographically bound to the *exact* fs-verity
  inode/digest that earned `T-pinned`. No path reopen, no re-resolution seam between the measurement
  and the exec.
- **I2 — No laundering.** No mutable/unmeasured bytes can become instructions. Enforced by
  **`MS_NOEXEC` on every mutable/grant mount** (blocks `execve` *and* file-backed `mmap(PROT_EXEC)` —
  mmap(2) `EPERM`), plus the closed-world manifest class that forbids the artifact itself *being* an
  interpreter/JIT (slice-7 §5.1). The home must not drop `NOEXEC` on anything but the pinned inode.
- **I3 — No caller-controlled exec mounts.** The exec surface is constructed by `gatekeeperd` from
  **derived evidence** (`der.exec_fd`), never from a caller path/flag.
- **I4 — Fail closed.** Any setup failure (mount, verity re-assert, Landlock add, exec) → refuse; no
  fall-back to a `noexec` run, an unpinned exec, or a weaker tier.
- **I5 — Posture containment.** The currently-fail-closed `MS_NOEXEC`/Landlock-no-EXECUTE posture for
  **grants and writable mounts stays intact**. Only the pinned artifact's own inode may gain exec.
- **I6 — Minimal blast radius / frozen policy.** Prefer a change that keeps `shrek-policy` (the pure
  crate: matrix + `floor(Pinned)=T0`) **frozen**.

## 3. The candidate mechanism — per-workload exact-inode exec island (option (a))

The strongest candidate. It reuses the proven `mount_plane` pin→bind→re-verify machinery, changed in
exactly two ways: drop `NOEXEC` for this one inode, and add a scoped Landlock EXECUTE rule.

Construction (in the per-request child, fail-closed throughout):

1. **Source = derived evidence.** Start from `der.exec_fd` — the `O_RDONLY` fd measured during
   derivation, already bound to the exact inode that matched the manifest (carried through
   `provenance_plane`). Never a caller path. *(I3)*
2. **Private mount ns.** `enter_private_mount_ns()` so the island never touches the host mount table.
   *(New for the T0 path, which is currently mount-free — see Fork B.)*
3. **Bind the inode** `/proc/self/fd/<exec_fd>` → a gatekeeper-owned island path
   (`/run/shrek/<id>/exec/<name>`), exactly as `relocate_ro`.
4. **Re-verify, twice.** (a) `statx` the island target `(dev,ino)` == the pinned identity — drift ⇒
   unmount + fail. (b) Re-assert fs-verity is enabled and re-`measure_verity` the island fd; the
   digest must still equal the manifest-pinned digest. *(I1)*
5. **Keep mutable grants `MS_NOEXEC`.** Base T0 is mount-free (grants stay at the anchor); the
   pinned-exec variant enters a mount ns anyway (step 2), so it **binds each mutable grant
   `…|MS_NOEXEC`** exactly as `relocate_ro` does at T1. This is what stops the pinned binary from
   `execve`-ing or `mmap(PROT_EXEC)`-loading any mutable byte (mmap(2) `EPERM`). *(I2, I5 — the whole
   `NOEXEC` posture for grants is preserved and is load-bearing.)*
6. **Harden the island mount:** `MS_BIND|MS_REMOUNT|MS_RDONLY|MS_NOSUID|MS_NODEV` — **NOT
   `MS_NOEXEC`.** This single, deliberate omission — for exactly one inode — is the whole boundary. It
   is safe *because* the inode is fs-verity-immutable + manifest-digest-pinned + closed-world-classified
   (not because of a mount flag; slice-7 §5.1: flags gate `execve`, not trust).
7. **Landlock:** add one `path_beneath` rule, `EXECUTE|READ_FILE`, `parent_fd` = the island's single
   inode `O_PATH` fd — so the entrypoint's `execve` is permitted for exactly that inode and denied for
   everything else. Grants keep their read-only (no-EXECUTE) rule. **Static-PIE-only (Fork A): `/usr`
   is NOT granted EXECUTE in the island ruleset**, and the pinned binary links nothing — so there is no
   dynamic-loader/library closure to define or authenticate.
8. **Execute the island** (`execve` the island path under `RESOLVE_NO_SYMLINKS`, or re-open+`execveat`
   the island fd). Not the original `exec_fd` — that fd is on the source mount, which may be `noexec`;
   the island is the exec-capable bind of the *same* inode, re-verified in step 4. *(I1)*

Invariant scorecard for the island: **I1 ✓ I2 ✓ I3 ✓ I4 ✓ I5 ✓ I6 ✓** (policy stays frozen —
`floor(Pinned)=T0` and the matrix are untouched; the change is entirely in the T0 constructor +
`mount_plane` + the `sandbox` refusal→construct route). Note the entrypoint being static PIE means the
island is the workload's *only* non-`noexec`, non-sealed surface, so the no-laundering property rests
on two independent facts: mutable grants are `MS_NOEXEC`, and the one exec-capable third-party inode is
fs-verity-frozen.

## 4. The other three options — why they collapse

**(b) Private materialization — DOMINATED.** Copy the measured bytes into a sandbox-private
exec-capable store (tmpfs), exec from there. Fails **I1**: you execute a *copy*, not the measured
inode; tmpfs has no fs-verity, so the copy is not self-authenticating and there is a write→exec TOCTOU
— you'd have to re-hash it in userspace (reintroducing exactly the sha256 slice-8 avoided by using
`FS_IOC_MEASURE_VERITY`). Fails **I2**: a writable exec-capable mount inside the sandbox *is* the
"mutable bytes become instructions" surface. Materialization is only justified when the source
*cannot* be exec-mounted in place — but an fs-verity file **can**. **Reject for fs-verity pins.**

**(c) Onion/sysext delivery — DIFFERENT PROBLEM.** `phase2-onion.md`: a sysext merges **system-wide**
into `/usr` via overlayfs and requires a **PKCS#7 verity signature** (`--image-policy=usr=signed`).
Two mismatches: (1) it is not **per-workload** (violates the stated goal); (2) it is **signature**
custody, whereas the pin-manifest is **digest** custody. Onion is the right path to make an artifact
**`T-first`** (sign it, ship it as a sealed layer) — a legitimately *different trust tier*, not an
exec-home for a digest-vetted third-party pin. **Out of scope here; note as "if the artifact warrants
a signature, promote it to `T-first` via Onion instead of pinning it."**

**(d) Require T1/T2 for pinned execution — COLLAPSES INTO Fork B.** T1 (nspawn) and T2 (gVisor)
already provide an exec-capable rootfs — but to run the *pinned inode* you must still bind it into the
guest's exec area with the exact-inode re-verify, i.e. **the same island, hosted inside a heavier
constructor.** So (d) is not a distinct mechanism; it is the island *plus* a `shrek-policy` change
raising `floor(Pinned)` to T1/T2 (unfreezing the pure crate) and paying container cost on every
pinned exec. Its real content is the isolation-strength question captured as **Fork B** below.

**Conclusion:** the mechanism is the **exact-inode exec island (a)**. (b) and (c) are
dominated/different-problem. (d) is "island hosted at ≥T1" — a tier choice, not a rival mechanism.
The decision therefore reduces to *adopt the island*, then two sub-forks.

## 5. Scorecard

| Option | I1 inode-bind | I2 no-launder | I3 no caller mount | I4 fail-closed | I5 posture | I6 frozen policy | Verdict |
|---|---|---|---|---|---|---|---|
| (a) exec island | ✓ strong | ✓ | ✓ | ✓ | ✓ | ✓ | **candidate** |
| (b) materialization | ✗ copy/TOCTOU | ✗ writable exec | ✓ | ~ | ✓ | ✓ | reject |
| (c) Onion/sysext | ~ (layer, signed) | ✓ | ✓ | ✓ | ✓ (system-wide) | ✓ | different tier (→T-first) |
| (d) require T1/T2 | ✓ (island in guest) | ✓ | ✓ | ✓ | ✓ | ✗ floor change | = island + Fork B |

## 6. FORKS to decide before any code

**FORK A — Static-only vs. sealed-dynamic pins. → DECIDED: static-PIE-only for slice-9 v1.** The
island grants EXECUTE (for `execve`) to exactly the pinned inode; the artifact must be a **static
PIE**. `/usr` is **not** granted EXECUTE in the island ruleset and the binary links nothing, so there
is **no dynamic-loader/library closure to define or authenticate** — the whole point of the
constraint. Mutable grants stay `MS_NOEXEC`, which (mmap(2) `EPERM`) blocks any file-backed executable
mapping of mutable bytes regardless of Landlock. *Sealed-dynamic* (grant EXECUTE to dm-verity `/usr`
so a dynamically-linked pin can load sealed libs + `ld.so`) is a **future extension that must define
its own loader/library closure rules** and is out of scope here.

**FORK B — Host the island at T0, or require ≥T1 (option (d))? → DECIDED: T0.** A T0 process sandbox
(Landlock+seccomp+ns+cgroups) hosts the island. Matches `floor(Pinned)=T0`, keeps `shrek-policy`
**frozen**, smallest blast radius. *(≥T1 containment — a container wall around third-party code —
remains a documented follow-up if a specific pinned artifact's threat model warrants it; it would cost
a `floor(Pinned)` change in the pure crate.)*

**FORK C (settled unless you object) — mechanism details.** (1) Exec surface is a **mount-level
island** (per-inode bind, `NOEXEC` dropped), *not* a Landlock-only rule over the caller's anchor mount
(which would make exec-capability depend on caller mount flags — violates I3). (2) Execution is
`execve` of the **re-verified island**, not `execveat` of the original `exec_fd` (whose source mount
may be `noexec`). These fall directly out of the invariants; flagged for visibility, not as open
questions.

## 7. Approved path (decided — build is a later slice, still no code here)

**(a) exact-inode exec island**, **static-PIE-only** (Fork A), **hosted at T0** (Fork B), mount-level
island executing the re-verified inode (Fork C). Meets all six invariants, keeps `shrek-policy` frozen,
reuses the proven `mount_plane` pin/re-verify machinery, and confines the reopened exec surface to a
single cryptographically-frozen inode while every mutable grant keeps `MS_NOEXEC` (the load-bearing
no-laundering enforcement, per mmap(2) `EPERM`).

Build order for the future slice (oracle-before-VM per standing method; get build-go first):
1. `mount_plane`: an **exec-island** variant of `relocate_ro` — bind the pinned inode, re-verify
   `(dev,ino)`, **re-assert fs-verity + re-`measure_verity`** the digest == manifest, remount
   `RO|NOSUID|NODEV` **without `NOEXEC`**; and (same call site) bind mutable grants **with `NOEXEC`**.
2. `proc_plane`: enter a private mount ns for the pinned-exec build; add the single-inode Landlock
   `EXECUTE|READ_FILE` island rule, gated on `trust==Pinned` **and** a bound `exec_fd` present; enforce
   the artifact is a static PIE (no interpreter/`INTERP` segment) — else fail closed.
3. `sandbox.rs`: route a `Pinned` derivation to construct-at-T0-with-island instead of the rc=15
   refusal; keep rc=15 for the no-`exec_fd` / setup-failure cases (I4).
4. Host oracle on genuine fs-verity (extend `pin-manifest-proof.sh`): pinned static PIE runs from the
   island; a mutable-grant `execve`/`mmap(PROT_EXEC)` is `EPERM`; a rogue inode never runs; any island
   setup failure ⇒ refuse.
5. Sealed-VM gate (new S7 block) → selective commit (no Codex docs, no AI refs, push gh constant-itis).
   `shrek-policy` unchanged.

**Deliberately NOT in this slice:** sealed-dynamic pins (Fork A follow-up, needs its own loader/library
closure), ≥T1 pinned containment (Fork B follow-up), and any change to the grant/writable `NOEXEC`
posture (never — I5).

## 8. Implementation notes (as built)

Built exactly as §7. Files: `mount_plane.rs` (`relocate_exec_island`, `seal_noexec_in_place`),
`proc_plane.rs` (island variant of the T0 build: `build_exec_island`, single-inode Landlock EXECUTE,
static-PIE `reject_if_dynamic`, `T0Spec.exec_island`), `sandbox.rs` (`Pinned`→island route, keeps
rc=15 for no-`exec_fd`/non-T0/no-Landlock). `shrek-policy` untouched. Oracle:
`scripts/pin-manifest-proof.sh` §1/§6/§7 (island runs; mutable-grant `mmap(PROT_EXEC)`+`execve`
`EPERM`; rogue no-island; dynamic-pin fail-closed). VM: `mount-plane-gate` S7.

**Load-bearing empirical finding — the bind source must be opened in the constructor's own mount
namespace.** `der.exec_fd` is measured by the gatekeeper in *its* mount ns; an `MS_BIND` of
`/proc/self/fd/N` fails **`EINVAL`** when N's `vfsmount` belongs to a different mount ns (proven in a
privileged container: a bind from a parent-ns fd fails, an identical bind from a fd opened *inside* the
ns succeeds — and re-opening the parent fd via its own `/proc/self/fd` magic-link does **not** rebase
the mount, so that does not help). Grants never hit this because `pin_beneath` opens them inside the
child. So the island **re-opens the entrypoint by path inside the private mount ns** (giving an ns-local
`vfsmount` usable as a bind source) and **re-verifies that inode's `(dev,ino)` AND fs-verity digest
against `der.exec_fd`** — the derived fd stays the identity/digest authority, so a path swap resolves to
a different inode (rejected) and forging the digest is a content-hash preimage (infeasible). I1/I3 hold:
the exec surface's identity is bound to the derived evidence, not the reopened path. The bind happens in
the *same* private-mount-ns child that already hosts base T0 (`proc_plane` P1, post-`CLONE_NEWNS`), so
no extra namespace is entered.

**Constructing the pin oracle** additionally required cgroup-v2 delegation (gatekeeperd's `_daemon`
self-move dance needs a base cgroup it alone occupies) — slice-8's oracle never exercised it because it
always refused at rc=15. The oracle now launches gatekeeperd into a fresh delegated base per run, as the
sealed VM does via `systemd-run --scope -p Delegate=yes`.
