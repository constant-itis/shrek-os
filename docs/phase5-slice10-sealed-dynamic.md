# Phase-5 Slice-10 (provisional) — sealed-dynamic T-pinned execution: boundary & research

**Status: BUILT (2026-08-19) — host oracle GREEN on genuine fs-verity; sealed-VM S8 gate PENDING.**
The mechanism below is implemented across `pin_manifest`/`provenance_plane`/`mount_plane`/`proc_plane`/
`sandbox` (gatekeeperd 57 unit + shrek-policy 25 unchanged); `scripts/pin-manifest-proof.sh` §8/§9 prove
a pinned dynamic closure RUNS from the N-inode island and a tampered member fails closed. See §9 as-built.
Original boundary/design record (forks decided) preserved below.
**Forks decided (2026-08-19, by owner): F1 = distinct T-pinned sealed-dynamic path (NOT a T-first
collapse); F2 = (B) per-workload digest-pinned closure; F3 = `DT_NEEDED`-only v1 (no arbitrary
`dlopen`); F4 = build-time enumeration ASSISTS generation but is NOT authority — the sealed manifest
+ runtime re-measure/re-verify are the authority.**

Successor problem to slice-9 (`docs/phase5-slice9-pin-exec-home.md` §6 Fork A, §7 "Deliberately NOT
in this slice"). Slice-9 is frozen and shipped (`efe8d3e`): a **static-PIE** pin runs from a
per-inode exec island at T0 while every mutable grant stays `MS_NOEXEC`. Slice-9 explicitly deferred
*sealed-dynamic* as "a future extension that must define its own loader/library closure rules." **This
slice is that extension.** It changes nothing in the shipped static-PIE island, `shrek-policy`
(`floor(Pinned)=T0`, matrix), or the grant/writable `MS_NOEXEC` posture.

Numbering is provisional (slice-5 reserved for transport); the file may be renamed on build-go.

---

## 0. The semantic distinction this slice must preserve

Two trust properties, deliberately kept separate (owner directive, 2026-08-19):

- **`T-first` is trust/custody.** A publisher *signs* an artifact; the system trusts the **signature
  chain** (`--image-policy=usr=signed`, PKCS#7). It is delivered as a sealed layer that merges
  **system-wide** into `/usr` (Onion/sysext, `phase2-onion.md`). The unit of trust is a **key**.
- **`T-pinned` is approval of exact content identities.** No key, no publisher trust. The operator
  vets a **specific inode by its fs-verity digest** and records that digest in the sealed manifest.
  The unit of trust is a **hash of exact bytes**, scoped **per-workload**.

Slice-10 extends `T-pinned` to a dynamically-linked workload **without borrowing `T-first`'s custody
model**. We do **not** promote a dynamic pin to `T-first` merely because authenticating its closure is
work. A sealed-dynamic pin is approved as a **set of exact content identities** — the entrypoint, its
interpreter, and its transitive libraries — each by fs-verity digest, each per-workload. If the answer
were "sign it and make it `T-first`," there would be no slice-10; F1 rejects that.

## 1. The problem, precisely

Slice-9 could pin **one** inode because a static PIE *is* its whole executable closure. It enforces
this by refusing anything else: `reject_if_dynamic` (`proc_plane.rs:217`) fails closed on **any**
`PT_INTERP` segment ("PT_INTERP present (dynamically linked)"). A dynamic pin has no such property.

At `execve` of a dynamically-linked ELF the kernel:

1. reads the **`PT_INTERP`** segment (a pathname, e.g. `/lib/ld-linux-x86-64.so.2`) and executes that
   **interpreter** (`ld.so`) — the *first* third-party code to run and the loader of everything after;
2. `ld.so` then maps every **`DT_NEEDED`** shared object, **transitively**, resolving each SONAME
   through: `DT_RPATH`/`DT_RUNPATH` in the ELF, the `LD_LIBRARY_PATH`/`LD_PRELOAD`/`LD_AUDIT`
   environment, `ld.so.cache`, and the default search directories (`/lib`, `/usr/lib`, …);
3. (out of scope for v1) may `dlopen` further objects at runtime.

So the object that must remain sealed/pinned is the **whole executable closure**, not the entrypoint.
Authenticating only the entrypoint digest (slice-9) is necessary but far from sufficient: the loader
and every library it maps are unauthenticated bytes that become instructions.

**Kernel facts that frame the solution** (primary sources; slice-9 §1 + #2624, re-stated):

- **`MS_NOEXEC` blocks file-backed executable mappings, not just `execve`** (mmap(2) `EPERM`). This is
  what stops a mutable/grant byte from being loaded **as a library**, not only from being `execve`'d.
  It is the load-bearing no-laundering instrument (I2) and stays intact for everything outside the
  closure (I5).
- **Landlock is restrictive-only and gates `execve`** ("Execute a file"). It cannot re-grant exec on a
  `noexec` mount and is **not** relied on to govern `mmap(PROT_EXEC)`. **For a dynamic pin, Landlock
  EXECUTE is therefore NOT the executable-mapping boundary.** The boundary is `MS_NOEXEC` on **every
  file-bearing mount outside the authenticated closure — including `/usr` — in the workload mount
  namespace**; only the N re-verified closure-member islands are exec-capable (non-`noexec`), governed
  by fs-verity immutability, not by a mount flag. Landlock EXECUTE scoped to the closure inodes remains
  as defense-in-depth on the `execve` path, but the property that no non-member byte can be
  `mmap(PROT_EXEC)`-loaded rests entirely on `MS_NOEXEC`.
- **A bind preserves inode identity + fs-verity**; fs-verity re-verifies content against the Merkle
  root on every fault, so a verity inode is immutable and self-authenticating wherever bound. This is
  what lets the closure be re-verified after relocation (slice-9's `relocate_exec_island` already
  relies on it for the entrypoint).
- **Anonymous `PROT_EXEC` (JIT)** is governed by neither instrument → excluded by classification: a
  JIT/interpreter-of-open-world-input is open-world and can never earn `T-pinned` (slice-7 §5.1).

## 2. What a sealed-dynamic home MUST satisfy (acceptance invariants)

Inherits slice-9 **I1–I6** (exact-inode/digest bind; no-laundering via `MS_NOEXEC`; no
caller-controlled exec mount; fail-closed; grant/writable `NOEXEC` posture preserved; frozen
`shrek-policy`), each now read over the **whole closure** rather than one inode — plus:

- **I7 — complete transitive closure.** The sealed manifest names the **interpreter and every
  transitive `DT_NEEDED`** of the pinned entrypoint. Every byte the loader can map to instructions is a
  manifest-listed closure member; no SONAME resolves to anything outside the sealed set.
- **I8 — no unsealed loader input.** `DT_RPATH`/`DT_RUNPATH`, `LD_LIBRARY_PATH`/`LD_PRELOAD`/
  `LD_AUDIT`/`LD_*`, `/etc/ld.so.preload`, `/etc/ld.so.cache`, and default search dirs cannot cause the
  loader to map executable bytes outside the closure. **Enforcement is `MS_NOEXEC` on every file-bearing
  mount outside the authenticated closure — including `/usr` — in the workload mount namespace; only the
  N re-verified closure-member islands are exec-capable.** `/etc/ld.so.preload` and `/etc/ld.so.cache`
  are additionally **masked** (absent/empty) for v1, and loader-affecting `LD_*` env is stripped.
  RPATH/RUNPATH/default searches may still *occur*, but any hit outside the closure is on a `noexec`
  mount and cannot be `mmap(PROT_EXEC)`-loaded.
- **I9 — sanitized loader environment.** The interpreter runs with `LD_PRELOAD`/`LD_AUDIT`/
  `LD_LIBRARY_PATH`/`LD_*` stripped and a fixed, closed environment; no caller-supplied env may reach
  `ld.so`.
- **I10 — manifest + runtime re-measure is the authority (not build-time enumeration).** The build
  pipeline may *walk* the closure to help author the manifest, but authority is the **sealed manifest**
  plus a **runtime `FS_IOC_MEASURE_VERITY` of every member** at construct time, each asserted equal to
  its manifest digest. A discrepancy between build-time enumeration and the sealed manifest is not a
  runtime input; only the manifest counts.
- **I11 — closure is fixed at construction (no runtime extension).** v1 permits `DT_NEEDED` linkage
  only; **no arbitrary `dlopen`**. The set of executable inodes is fully determined before the
  entrypoint runs; nothing may add a member at runtime. (`dlopen`-of-a-closure-member is a possible v2;
  `dlopen`-of-arbitrary-path is open-world → never pinnable.)

A miss on any is a rejection.

## 3. The mechanism — N-inode digest-pinned closure island (F2 = B)

Generalize slice-9's **1-inode** exec island to an **N-inode closure** containing exactly: the pinned
**entrypoint**, the pinned **`PT_INTERP`** interpreter, and every pinned transitive **`DT_NEEDED`**
member — and nothing else executable.

### 3a. Manifest grammar — v2 closure records

Today (`pin_manifest.rs`, `HEADER_V1 = "shrek-pin-manifest v1"`): one line per pin,
`<algo> <digest_hex> <class>`, one identity `(algo, digest)`, `closed-world` eligible to seal.

Extend to **v2** so a pin can carry a **closure**: the entrypoint record plus a set of member records
(interpreter + libraries), each `<algo> <digest_hex>` with a role and the **loader-visible name**
(SONAME / interpreter path) the member must satisfy. Sketch (grammar TBD at build-go):

```
shrek-pin-manifest v2
entry   <algo> <hex> closed-world <entry-name>
interp  <algo> <hex> <interp-path>          # exactly one; the pinned ld.so
lib     <algo> <hex> <soname>               # one per transitive DT_NEEDED
lib     <algo> <hex> <soname>
...
```

Constraints (fail-high, as v1): exactly one `interp`; every `lib` digest distinct; the entrypoint's
declared closure must be **complete** (see I7 / §3c). v1 keeps `closed-world` as the only sealable
class; the class check now applies to the **closure as a whole** — no member may be open-world.

### 3b. Derivation — authenticate the closure by re-measure (`provenance_plane`)

Slice-9 carries a single `der.exec_fd`. Extend the pin arm to carry a **closure of derived fds**
(`der.closure`): for the entrypoint's `PT_INTERP` and each transitive `DT_NEEDED`, open the member
`O_RDONLY`, `FS_IOC_MEASURE_VERITY` it, and assert the digest equals the manifest member digest —
**runtime measure is the authority (I10)**. Any member that is absent, not verity-enabled, digest-
mismatched, or not present in the manifest ⇒ fail closed (no `T-pinned`). The entrypoint's declared
`DT_NEEDED`/`PT_INTERP` set must match the manifest closure set exactly (no unlisted need, no unused
listing) ⇒ **completeness** (I7).

### 3c. Construction — N-inode island in the private mount ns (`mount_plane` + `proc_plane`)

Generalize `relocate_exec_island` (`mount_plane.rs:215`) from one target to the closure. In the
per-request child's **private mount ns** (`proc_plane` P1, post-`CLONE_NEWNS` — same ns that hosts base
T0), for **each** closure member:

1. **ns-local re-open by path**, then re-verify. Per slice-9's load-bearing finding (#2626,
   `mount_plane.rs:223-236`): an `MS_BIND` of `/proc/self/fd/N` fails `EINVAL` across mount namespaces,
   so each member is re-opened **by path inside the child ns** for an ns-local `vfsmount`, then
   re-verified — `(dev,ino)` **and** `FS_IOC_MEASURE_VERITY` digest — against its `der.closure` entry
   (the derived-evidence/digest authority). A path swap resolves to a different inode (rejected);
   forging the digest is a content-hash preimage (infeasible). I1/I3 hold for **every** member.
2. **Bind the member's pinned inode** onto its **loader-visible path inside the island** — the
   entrypoint at the island exec path, each library at the path its SONAME resolves to — so the
   loader's own resolution lands on the pinned inode. The **interpreter is bound at the exact
   manifest/`PT_INTERP` pathname**, and construction **requires that pathname to resolve to the
   re-verified interpreter island** (else fail closed): the kernel reads `PT_INTERP` and must reach the
   pinned `ld.so`, nothing else. This is how I8 is met **positively**: the closure island reconstructs
   exactly the loader namespace the pin needs, populated only by re-verified pinned inodes.
3. **Harden RO WITHOUT `MS_NOEXEC`** (`MS_BIND|MS_REMOUNT|MS_RDONLY|MS_NOSUID|MS_NODEV`) — the same
   single deliberate omission as slice-9, now for exactly the N closure inodes, each proven
   fs-verity-immutable + manifest-digest-pinned. **Every other file-bearing mount stays/becomes
   `MS_NOEXEC` in the workload mount namespace — including `/usr`** (`seal_noexec_in_place` for mutable
   grants; `/usr` is remounted `noexec` for the dynamic-pin path — a divergence from slice-9's static
   path, which left `/usr` exec-capable because the entrypoint linked nothing; here `/usr` must not be
   an executable-mapping surface since RPATH/RUNPATH/default search can reach it). So the **only**
   exec-capable bytes in the ns are the N closure islands. I2/I5 intact.
4. **Landlock (defense-in-depth):** one `path_beneath` `EXECUTE|READ_FILE` rule per closure inode
   (generalize the single rule at `proc_plane.rs:408-423` to N); `/usr` gets **no EXECUTE** in the
   island ruleset (unchanged from slice-9 Fork A). Landlock gates *which file may `execve`*; it is
   **not** the executable-mapping boundary (step 3's `MS_NOEXEC` is). The closure is self-contained, not
   sourced from `/usr`.
5. **Sanitize the loader environment + mask loader config (I9/I8):** strip
   `LD_PRELOAD`/`LD_AUDIT`/`LD_LIBRARY_PATH`/`LD_*`; run the interpreter with a fixed closed env. For
   v1, **mask `/etc/ld.so.preload` and `/etc/ld.so.cache`** (bind them absent/empty in the ns) so the
   loader falls back to island-path resolution with no cache- or preload-driven input. Any RPATH/RUNPATH
   or default-dir hit outside the closure lands on a `noexec` mount (step 3) and cannot map as code.
6. **Replace `reject_if_dynamic` with `authenticate_closure`:** instead of rejecting any `PT_INTERP`,
   require that the entrypoint's `PT_INTERP` equals **the one pinned interpreter** and that its
   `DT_NEEDED` set equals the manifest closure — else fail closed. (The static-PIE path stays: a pin
   with no `PT_INTERP` still routes through slice-9's 1-inode island unchanged.)
7. **Execute** the re-verified island entrypoint (not any source fd). The kernel reads `PT_INTERP` →
   runs the **pinned** `ld.so` at its island path → which resolves each `DT_NEEDED` to an island path
   that is a **re-verified pinned inode**. Closure closed.

Invariant scorecard for the closure island: **I1 ✓ (every member) I2 ✓ I3 ✓ I4 ✓ I5 ✓ I6 ✓ I7 ✓
I8 ✓ I9 ✓ I10 ✓ I11 ✓**. `shrek-policy` stays frozen; the change is confined to the T0 constructor +
`mount_plane` + `provenance_plane` closure carry + `pin_manifest` v2 + the `sandbox` route.

## 4. Rejected / out-of-scope alternatives

- **(A) Source libraries from dm-verity `/usr`** (grant `/usr` EXECUTE in the island; entrypoint
  digest-pinned, libraries "whatever `/usr` provides"). **Rejected by F2.** It fails **I7** (the
  closure is *all of `/usr`*, not the `DT_NEEDED` set — the pin can load any `/usr` code), and it makes
  the closure **non-per-workload** (behavior floats with the image's `/usr` version). It also blurs §0:
  authenticating libraries by the **dm-verity system root** is `T-first`'s system-wide custody, not
  `T-pinned`'s per-workload content-identity approval.
- **(C) Sign the closure → `T-first` via Onion/sysext.** **Rejected by F1.** Per slice-9 §4c a
  signature promotes the artifact to a **different tier** (`T-first`): signature custody, system-wide
  `/usr` merge. That is a legitimate path *for artifacts that warrant a signature* — but it is **not** a
  per-workload digest-pinned home, and adopting it merely to avoid closure work erases the §0
  distinction. Documented as the correct route **only** when the operator actually wants custody-based,
  system-wide trust; it is not slice-10.
- **Arbitrary `dlopen` at runtime.** **Out of scope for v1 (I11 / F3).** A `dlopen` of a non-member
  hits no-EXECUTE + `MS_NOEXEC` and fails; a workload that *requires* runtime-discovered code is
  open-world → not pinnable. `dlopen`-restricted-to-closure-members is a possible v2.

## 5. Scorecard

| Option | I1 per-member | I7 complete closure | I8 no loader input | I5 posture | I6 frozen policy | §0 distinction | Verdict |
|---|---|---|---|---|---|---|---|
| **(B) N-inode digest closure island** | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ per-workload content-identity | **adopted (F2)** |
| (A) libs from dm-verity `/usr` | ✓ entry / dm-verity libs | ✗ closure = all `/usr` | ~ | ✓ | ✓ | ✗ = system custody | rejected |
| (C) sign → `T-first` (Onion) | n/a (signature) | ✓ system-wide | ✓ | ✓ | ✓ | ✗ different tier | different tier |

## 6. Forks — DECIDED (2026-08-19)

- **F1 — distinct vs collapse → DISTINCT.** Sealed-dynamic is a genuine `T-pinned` path (approval of
  exact content identities), not a promotion to `T-first`. §0 is the load-bearing invariant.
- **F2 — granularity → (B) per-workload digest-pinned closure.** The closure is exactly the
  entrypoint + pinned `PT_INTERP` + pinned transitive `DT_NEEDED`; nothing else executable. `/usr` gets
  no island EXECUTE.
- **F3 — `dlopen` scope → `DT_NEEDED`-only v1, no arbitrary `dlopen`** (I11). Closure fixed at
  construction.
- **F4 — enumeration authority → sealed manifest + runtime re-measure** (I10). Build-time closure
  enumeration is a **generation aid** for authoring the manifest, never the runtime authority.

## 7. Approved shape — build order for the future slice (still NO CODE here)

Oracle-before-VM per standing method; **get BUILD-GO first**. Confined blast radius: `pin_manifest`
(v2 grammar) + `provenance_plane` (closure carry + per-member re-measure) + `mount_plane`
(N-inode island) + `proc_plane` (`authenticate_closure`, N Landlock EXECUTE rules, loader-env
sanitize) + `sandbox` (route). **`shrek-policy` untouched; static-PIE slice-9 path untouched; grant
`MS_NOEXEC` posture untouched.**

1. **`pin_manifest` v2** — closure records (`entry`/`interp`/`lib`), fail-high parse, one interp,
   distinct lib digests, closed-world-only, back-compatible with v1 static pins.
2. **`provenance_plane`** — pin arm resolves `PT_INTERP` + transitive `DT_NEEDED` from the entrypoint
   ELF, looks each up in the manifest closure, opens `O_RDONLY`, `FS_IOC_MEASURE_VERITY`, asserts ==
   manifest; carries `der.closure`; asserts declared-set == manifest-set (completeness, I7). Fail
   closed on any gap.
3. **`mount_plane`** — generalize `relocate_exec_island` to bind each closure member at its
   loader-visible island path, re-verify `(dev,ino)` + fs-verity digest per member, remount
   `RO|NOSUID|NODEV` **without `NOEXEC`**; mutable grants keep `NOEXEC` via `seal_noexec_in_place`.
4. **`proc_plane`** — replace `reject_if_dynamic` with `authenticate_closure` (pinned-interp +
   manifest-`DT_NEEDED` gate; static-PIE pins still route as slice-9); N single-inode Landlock
   `EXECUTE|READ_FILE` rules; loader-env sanitization; `/usr` no EXECUTE.
5. **`sandbox.rs`** — route a dynamic `Pinned` derivation with a complete `der.closure` to
   construct-at-T0-with-closure-island; keep rc=15 fail-closed for incomplete closure / no interp /
   setup failure (I4).
6. **Host oracle** (extend `pin-manifest-proof.sh`) on genuine fs-verity: a pinned dynamic workload
   runs from the closure island; a `DT_NEEDED` swapped for a non-member (unlisted digest) ⇒ refuse; a
   library path outside the closure is `noexec` (`execve`/`mmap(PROT_EXEC)` `EPERM`); `LD_PRELOAD` of a
   mutable object is ignored/blocked; any member digest drift ⇒ refuse; `reject_if_dynamic`'s
   static-PIE behavior unchanged.
7. **Sealed-VM gate** (new S8 block) → selective commit (no Codex docs, no third-party tooling
   attributions, push gh constant-itis). `shrek-policy` unchanged.

**Deliberately NOT in this slice:** arbitrary/runtime `dlopen` (v2), sourcing libraries from `/usr`
(rejected A), signature/custody delivery (that is `T-first`), ≥T1 pinned containment (slice-9 Fork B
follow-up), any change to the grant/writable `NOEXEC` posture (never — I5).

## 8. Open sub-questions to settle at BUILD-GO (not blockers to this doc)

- **v2 manifest wire grammar** — exact tokens/roles/name encoding for `interp`/`lib`; how a SONAME maps
  to its island path (record the resolved leaf name vs the full search path).
- **Transitive resolution at derivation time** — read `DT_NEEDED`/`DT_RPATH`/`DT_RUNPATH` from the
  entrypoint + each library to compute the declared set for the completeness check (dep-free ELF walk,
  like `reject_if_dynamic`'s program-header parse) — vs. relying solely on the manifest and letting a
  missing member fail at load. Leaning: derive the declared set to enforce I7 *before* exec, not after.
- **`ld.so.cache` / `ld.so.preload` handling — SETTLED (amendment 2026-08-19):** mask both
  `/etc/ld.so.cache` and `/etc/ld.so.preload` (absent/empty) for v1; no sealed cache is baked. Loader
  falls back to island-path resolution.
- **Interpreter path binding — SETTLED (amendment 2026-08-19):** bind the pinned `ld.so` at the **exact
  manifest/`PT_INTERP` pathname** and require that pathname to resolve to the re-verified interpreter
  island (fail closed otherwise). Confirm at build that the kernel's `PT_INTERP` resolution happens
  inside the workload mount ns.
- **`build-in-container.sh` closure-bake** — the generation-only walk that emits the v2 manifest
  (analogue of slice-8's offline pin bake), explicitly marked non-authoritative (I10).

## 9. Implementation notes (as built)

Built per §3/§7 (BUILD-GO 2026-08-19, with the amendment: `MS_NOEXEC` on every non-closure file mount
incl. `/usr` is the executable-mapping boundary, not Landlock; mask `/etc/ld.so.preload` + `/etc/ld.so.cache`;
interpreter bound at its exact `PT_INTERP` path). Blast radius exactly as scoped; `shrek-policy` and the
slice-9 static-PIE path untouched.

- **`pin_manifest.rs`** — v2 grammar (`entry`/`interp`/`lib` records; `interp` absolute, `lib` bare
  SONAME), `Closure`/`ClosureMember`/`PinMatch`, `lookup_match`. Fail-high extended: closure record under
  v1 header, `interp`/`lib` with no open `entry`, ≠1 `interp`, duplicate lookup/member key, wrong member
  shape. v1 manifests parse unchanged.
- **`provenance_plane.rs`** — pin arm uses `lookup_match`; a closure match carries `Derivation.closure`
  (only on a `T-pinned` band, alongside `exec_fd`).
- **`mount_plane.rs`** — `relocate_member` (bind a member at its loader-visible path, authority = sealed
  digest re-measured, `RO|NOSUID|NODEV` **without `NOEXEC`**); `seal_subtree_noexec` (force `/usr`
  `MS_NOEXEC` in-ns); `mask_with_empty` (empty RO bind over loader config). `relocate_exec_island`
  (slice-9) untouched. Member targets are fully symlink-resolved before bind/re-measure (merged-usr:
  `/lib64/ld-linux…` is a symlink to a symlink).
- **`proc_plane.rs`** — `build_closure_island` (seal grants + `/usr` NOEXEC, mask loader config, entry
  island via slice-9 machinery, bind interp + libs, `authenticate_closure`); `authenticate_closure`
  (dep-free ELF parse of `PT_INTERP` + `DT_NEEDED` via `DT_STRTAB` vaddr→offset; require `PT_INTERP` ==
  pinned interp and every `DT_NEEDED` ∈ pinned libs); N-inode Landlock `EXECUTE` rules; `construct`
  routes closure→`build_closure_island`, else slice-9 `build_exec_island`. `reject_if_dynamic` stays as
  the static-route guard (a dynamic binary pinned as a *standalone* pin is still rejected).
- **`sandbox.rs`** — `closure_to_spec` lowers the derived `Closure`; `T-pinned` route audits
  `island=closure` vs `island=exec` and passes `closure` into `T0Spec`; rc=15 fail-closed unchanged.
- **Delivery convention (v1, resolves §8 sub-question):** closure members are delivered as same-basename
  fs-verity files in the entrypoint's own directory; the entrypoint carries `DT_RPATH $ORIGIN/lib`
  (transitive) so the pinned loader resolves every library from the island lib dir (`/run/shrek/<id>/exec/lib`).
- **Load-bearing empirical finding — member targets must be FULLY symlink-resolved before bind + re-
  measure.** On merged-usr the interpreter's `PT_INTERP` path (`/lib64/ld-linux-x86-64.so.2`) is a
  symlink whose *final component* is also a symlink; `relocate_member` therefore `canonicalize`s the
  whole target (existing → full; not-yet-created lib under `/run` → parent-canonicalize + rejoin) before
  binding and before `measure_at_path` (which uses `RESOLVE_NO_SYMLINKS` and would otherwise `ELOOP`).
  This only moves where the bind lands (same inode); the kernel/loader resolve the logical name the same
  way and reach our bind; content authority stays the pre-bind source-digest check.
- **Oracle (`pin-manifest-proof.sh`)** — §8: a `DT_RPATH $ORIGIN/lib` dynamic gate-probe + its full
  `ldd` closure, each verity-enabled, baked into a v2 manifest, RUNS from the closure island (loader
  resolved interp+libs to pinned inodes) and a mutable-grant `mmap(PROT_EXEC)` still `EPERM`. §9: a
  wrong interp digest ⇒ construction fails closed, bytes never run. Sections 1–7 (slice-8/9) still green.
- **Sealed-VM S8 gate** (`mount-plane-gate` + `build-in-container.sh` STAGE 1/2) — the production custody
  path: STAGE 1 stages a `DT_RPATH $ORIGIN/lib` dynamic probe + its enumerated closure; STAGE 2 bakes a
  **v2 manifest under dm-verity `/usr`** (offline `fsverity digest` per member == runtime kernel measure;
  enumeration only *generates*, the sealed manifest + runtime re-measure remain authority). S8 provisions
  a dedicated `ext4 -O verity` loopback, verity-enables the closure, and asserts on the sealed
  enforcing-Secure-Boot kernel: `derived=T-pinned`, `construct-at=T0 island=closure`, `island-ran`,
  mutable-grant `mmap(PROT_EXEC)`/`execve` `EPERM`, and a tampered (byte-different) interp inode ⇒
  construction fails closed, bytes never run.

---

**Boundary/design record above; §9 records the as-built implementation, host-oracle-green and gated by the sealed-VM S8 block.**
