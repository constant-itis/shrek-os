# Phase 5 — Slice 8: the sealed pin-manifest (B1 evidence store — T-pinned)

> Status: **BOUNDARY ACCEPTED (with amendments, folded in below) — BUILD-GO for Slice-8.**
> Scope locked by direction (AskUserQuestion, 2026-08-19): **A1 — static, image-baked
> pin-manifest only.** T-pinned earned by a content-digest match against a dm-verity-sealed pin
> list. **Zero coupling to the not-yet-built §4 mutable plane; no TPM; no grant-protocol
> dependency.** The two larger forks (A2 build the §4 mutable plane here; A3 full T-untrust
> provenance-log) were considered and **declined for this slice** — they belong with / after the
> grant-protocol slice, per slice-7 §8 sequencing.
>
> **Approved amendments (2026-08-19), locked:**
> - **A.** `FS_IOC_MEASURE_VERITY`. Pin identity is the tuple **`(digest_algorithm, digest)`** —
>   an unknown algorithm or an unexpected digest size **fails high**. Measurement stays bound to the
>   **same pinned object ultimately executed**: measure the pinned fd and execute *that fd* — **no
>   measure-by-path / reopen seam** anywhere between measurement and exec.
> - **B.** *Reversed from the boundary rec:* a **versioned static pin-manifest FILE under dm-verity
>   `/usr`**, NOT compiled-in. Retains §4 static custody (sealed on the verity root) while separating
>   mechanism (gatekeeperd) from policy (the manifest), and lets acceptance exercise the **production
>   gatekeeper binary** against a **fixture manifest**. Parser is tiny + dependency-free and **fails
>   high on malformed / unknown / conflicting input**.
> - **C.** Per-entry exec-class stays, with **`OPEN_WORLD` defined BEHAVIORALLY**: any profile that
>   lets **mutable / unmeasured bytes become instructions** — interpreters, JITs, plugin/extension
>   loaders — is open-world and cannot earn a positive band.
> - **D.** The VM gate gets a **deterministic, dedicated fs-verity-capable writable filesystem** and
>   **fails (not skips)** if the primitive is unavailable. Anti-spoof fixture is a **one-byte-different
>   second verity inode** (unpinned digest ⇒ `T-hostile`) — NOT in-place tamper, since an
>   fs-verity-enabled file is itself immutable.
>
> **SCOPE NARROWED to CLASSIFICATION-ONLY (post-GO finding, user-directed 2026-08-19):** during build
> it emerged that a pinned artifact has **no executable home** — a writable grant is `MS_NOEXEC`
> (mount_plane.rs:149) + Landlock read-only (proc_plane.rs:89), and `/usr` (the only exec path) is
> whole-device dm-verity, not per-file fs-verity, so it is already `T-first`. `MNT_NOEXEC` + Landlock
> block execution regardless of fd-vs-path. **This slice therefore delivers the pin STORE +
> measurement + fd-binding + `T-pinned` DERIVATION only, and proves pathname-independent `T-pinned`
> CLASSIFICATION on a writable fs-verity filesystem. Runnable `T-pinned` workloads are EXPLICITLY
> UNSUPPORTED: a `T-pinned` construction REFUSES deterministically with reason
> `pinned-exec-home-unavailable` and NO downward (T2) or upward (T1 fall-up) constructor workaround.**
> T0's `MS_NOEXEC` / Landlock-`EXECUTE` posture is **NOT reopened** here. The execution-home design is
> the **next, separately-reviewed boundary** — comparing a dedicated exec mount vs private
> materialization vs Onion/sysext vs higher-tier-only execution — **before** touching frozen
> constructor security. Sections below that predate this narrowing (esp. §2's "constructs at T0/T1"
> use case, §5 step 4, §8) are superseded by this note and by §10.

This is the **first B1 evidence store**. Slice-7 shipped the derivation *spine*
([`phase5-slice7-trust-provenance.md`](phase5-slice7-trust-provenance.md)) and left the two
supply-chain stores deferred (slice-7 §8, lines 232–235). This slice builds the smaller,
fully-decoupled one: the **static pin-manifest** that lets a *specific, digest-vetted* third-party
artifact earn `T-pinned` (floor T0) instead of failing high to `T-hostile` (floor T2).

## 1. What is already locked (do NOT relitigate)

Inherited verbatim from slice-7 and §4; this slice does not re-decide any of it:

- **The pure lattice already supports T-pinned.** `shrek_policy::derive_band` returns
  `TrustBand::Pinned` for `pinned_digest_match && domain_execution_sealed`
  ([`crates/shrek-policy/src/provenance.rs`](../crates/shrek-policy/src/provenance.rs):114–117,
  tested :171–178). `Evidence.pinned_digest_match` (:72) is already declared *"so the deferred
  store drops in without a signature change."* **No policy-logic change. This slice only builds the
  store that sets that field.**
- **The resolution invariant (slice-7 §3).** `T-hostile` is the floor; every band above it needs
  its own affirmative proof. A pin *match* is that affirmative proof for `T-pinned`; a **miss, a
  tamper, or a domain-gate failure is `T-hostile`**, never a silent weaker band.
- **The no-laundering domain gate (slice-7 §5.1) applies to `T-pinned` too.** A pin match is
  necessary but **not sufficient**: `domain_execution_sealed` must also hold. A pinned *interpreter*
  (open-world) can match its digest and still fails high — pinning a `python` does not let it launder
  `T-pinned` onto the scripts it runs.
- **Custody class = §4 static.** The pin list is **sealed policy baked under the dm-verity root**,
  changed only by a signed image update (security-model.md §4, "STATIC POLICY … BAKED INTO THE
  sealed IMAGE"). It is **never** a file on writable `/home`,`/var`, and there is **no runtime
  override** — the same posture as the compiled-in `CLOSED_WORLD` list today.
- **`agentd` proposes, `gatekeeperd` re-derives** (slice-7 §6). Unchanged. The pin match is computed
  entirely broker-side from the measured object; the caller supplies nothing the broker trusts.

## 2. The gap this slice closes

After slice-7, the *only* band `gatekeeperd` can positively earn is `T-first`, and only for a
closed-world program **resident on the sealed dm-verity root** (`entrypoint_sealed` via `st_dev`
match, provenance_plane.rs:124–131). Every artifact **not** on the sealed image — including code an
operator has explicitly vetted and wants to run — falls to `T-hostile` / floor T2.

That is the correct fail-safe default, but it leaves no path for *"this specific third-party binary,
at this exact content, is blessed to run below T2."* The pin-manifest is that path: a digest-keyed
allow-list, sealed with the same integrity class as the matrix and floor, that `gatekeeperd`
consults when the entrypoint is **not** on the sealed root.

Concrete use case (CLASSIFICATION, this slice): a vetted, statically-linked tool (e.g. a specific
`ripgrep` build) on a writable **fs-verity** filesystem. Today its band is asserted, not proven. With
a pin entry for its exact fs-verity digest + `closed-world` class, gatekeeperd **derives `T-pinned`**
from the measurement — pathname-independently, correcting a caller who under- or over-claims. Change
the bytes ⇒ the digest changes ⇒ no match ⇒ `T-hostile`. The **operator changes the trusted-pin set
only via a signed image update** — deliberately the same friction as changing static policy.

> **This slice stops at the classification.** *Running* that pinned tool below T2 is NOT delivered
> here — a pinned artifact has no executable home (see the SCOPE-NARROWED note above), so a `T-pinned`
> construction refuses `pinned-exec-home-unavailable`. The value shipped is the **hardened band
> derivation** (ADV-2): the pin becomes measurable evidence instead of a caller assertion. Making it
> *runnable* is the next boundary.

## 3. The digest — fs-verity Merkle root, kernel-computed

**Decision (recommend; §9-A): the pin key is the file's fs-verity digest, read via
`FS_IOC_MEASURE_VERITY`.** Rationale, all three of which the alternative (userspace SHA-256) fails:

1. **Dep-free.** `gatekeeperd` has *zero* external crates today
   ([`crates/gatekeeperd/Cargo.toml`](../crates/gatekeeperd/Cargo.toml) — only the `shrek-policy`
   path dep). Userspace hashing means a new crypto dependency or vendored crypto in-tree — against
   the minimal-deps rule and a needless attack-surface add. fs-verity's digest is computed **by the
   kernel**; we add one ioctl to `linux_uapi`, no crypto code.
2. **TOCTOU-immune.** fs-verity makes the file **immutable** (writes rejected after
   `FS_IOC_ENABLE_VERITY`) and **re-verifies every block against the sealed Merkle root on every
   read**. So "measure the digest, then exec the file" cannot be raced — the bytes the kernel
   executes are the bytes it measured, enforced continuously. A userspace hash has a
   hash-then-exec swap window unless the fd is carried through exec (which the path-based constructor
   does not do).
3. **Already mandated by §4.** security-model.md §4 states mutable grant/pin files are
   *"fs-verity-sealed (immutable once written: write-new → `FS_IOC_ENABLE_VERITY` → atomic swap)."*
   Using the verity digest here is implementing §4's stated custody for the pin case, not new debt.

**Pin identity is the tuple `(digest_algorithm, digest)`, not a bare hash (amendment A).**
`FS_IOC_MEASURE_VERITY` returns a `fsverity_digest{ digest_algorithm, digest_size, digest[] }`;
gatekeeperd matches **all three** — an entry whose algorithm the kernel/manifest doesn't both know,
or whose size disagrees with the algorithm, **fails high** (never a truncated/zero-padded compare).
MVP algorithm: `sha256` (fs-verity's default). The digest is a pure function of file **content**,
independent of inode/filesystem, so it is **computed at image-build time** from the artifact and
written into the manifest; build-time producer and runtime consumer agree by construction.

**Measurement stays bound to the executed object — no reopen seam (amendment A).** gatekeeperd pins
the entrypoint **once** as an fd (`openat2` O_PATH, `RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS`),
measures verity **on that fd**, and hands **that same fd** to the constructor for execution
(`execveat(fd, "", …, AT_EMPTY_PATH)` / `/proc/self/fd/N`) — the object measured *is* the object
executed. There is deliberately no "measure path, then re-open path to exec" step where a swap could
land. (fs-verity already makes each file immutable + block-verified, but carrying the fd removes the
*resolution* race too, not just the content race.)

**Cost / prerequisite (§9-D risk):** the writable backing store holding pinned files must support
fs-verity (ext4/f2fs/btrfs with the feature) and each pinned file must have verity **enabled** at
provisioning time. In the VM gate this is an acceptance-harness setup step (see §8). If a pinned
file lacks verity, the `FS_IOC_MEASURE_VERITY` ioctl returns `ENODATA` ⇒ no digest ⇒ no match ⇒
`T-hostile` — a safe fail, and a legible one (the audit line distinguishes it).

## 4. The manifest — a versioned sealed FILE under dm-verity `/usr` (amendment B)

**Decision (amendment B — reversed from the boundary rec): a versioned pin-manifest FILE at a fixed
sealed path, e.g. `/usr/lib/shrek/pin-manifest`.** It lives under the dm-verity root, so it keeps
full §4-static custody (change it ⇒ signed image update, verified by the boot chain), while
**separating mechanism from policy**: gatekeeperd carries the parser + match logic, the manifest
carries the policy. The decisive win over compiling entries in: **acceptance exercises the real,
shipped gatekeeper binary against a fixture manifest** — the test image bakes a manifest with the
fixture pin; the production image ships an empty one; the binary is byte-identical either way.

Format — a tiny, **dependency-free, line-oriented** grammar with a version header, parsed by a
hand-rolled reader (no serde, no external crate):

```
shrek-pin-manifest v1
<digest_algorithm> <digest_hex> <exec_class>      # e.g.  sha256 a1b2…  closed-world
…
```

The parser **fails high on any doubt** — the whole load yields "no pins" (⇒ nothing earns `T-pinned`)
rather than a partial/optimistic set:
- missing/unknown version header, or any unknown token/field ⇒ fail high;
- unknown `digest_algorithm`, or a `digest_hex` whose length ≠ that algorithm's size ⇒ fail high
  (amendment A: unknown algorithm/size never matches);
- unknown `exec_class` ⇒ fail high;
- **conflicting entries** (same `(algorithm,digest)` with differing class) ⇒ fail high — no
  last-writer-wins, a contradiction is a policy error, not a silent pick.
- A missing manifest file ⇒ **no pins** (not an error): the shipped default. `T-pinned` simply cannot
  be earned, the correct fail-safe.

**Each entry carries its execution class — mandatory, the load-bearing correctness point.** A pinned
binary on a writable mount is never in the compiled-in `CLOSED_WORLD` *path* list, so
`exec_class_closed_world()` returns false for it; without a per-entry class a legitimate pin match
would still derive `T-hostile`. So the *sealed manifest* is the authority for the pinned artifact's
execution class (amendment C defines the classes **behaviorally**):

- `closed-world` — the profile does **not** let mutable/unmeasured bytes become instructions ⇒ the
  match sets `domain_execution_sealed = true` ⇒ eligible for `T-pinned`.
- `open-world` — the profile **does** let mutable/unmeasured bytes become instructions (an
  interpreter, a JIT, a plugin/extension loader) ⇒ `domain_execution_sealed = false` ⇒ **fails high
  to `T-hostile`** even though the digest matched (no-laundering, slice-7 §5.1, holds for pins). The
  entry is still legible: "we recognize this exact artifact, and it is deliberately not below T2."

Because the manifest is verity-sealed, this classification is trusted with the same assurance as the
matrix itself — a writable-label attacker (ADV-8) cannot forge it.

## 5. Binding — measure the object about to execute (mirrors §5)

`gatekeeperd`, at construct time, extends the slice-7 `measure()` path
(provenance_plane.rs:108–132) with a pin arm that runs **when `entrypoint_sealed` is false** (a
sealed-root entrypoint is already `T-first`; strongest-first ordering in `derive_band` makes pin
evaluation moot for it):

1. Pin the entrypoint TOCTOU-safely with the **existing** `openat2(RESOLVE_NO_SYMLINKS |
   RESOLVE_NO_MAGICLINKS)` (`open_entry_rdonly`, mirroring `statx_of`) — **one** O_RDONLY fd, not a
   re-resolvable path; the fs-verity measurement is taken on THIS fd, and it is **bound** to the
   derivation (`Derivation.exec_fd`) so the classification is provably tied to that object.
2. Read `(algorithm, digest)` from that fd via `FS_IOC_MEASURE_VERITY`. `ENODATA` (no verity), any
   ioctl error, or an unknown algorithm/size ⇒ no digest ⇒ pin arm yields nothing (fail high).
3. Look `(algorithm, digest)` up in the **loaded sealed manifest** (§4). Miss ⇒ nothing. Hit ⇒
   `pinned_digest_match = true` and `domain_execution_sealed = (entry.class == closed-world)`.
4. `derive_band` yields `T-pinned` iff both hold, else `T-hostile` — no new logic. **`T-pinned` then
   REFUSES construction** (`pinned-exec-home-unavailable`, §7) this slice — the bound fd is never
   executed; it demonstrates the pathname-independent binding for the future exec-home slice.

`Evidence` gains no field; `measure()` stops hardcoding `pinned_digest_match: false` and instead
fills it from the pin arm. `domain_execution_sealed` becomes `exec_class_closed_world(path) OR
(pin_hit && entry.class == ClosedWorld)` — the two sources are disjoint by `st_dev` (sealed-root vs
writable-mount), so there is no ambiguity.

## 6. Wire contract — unchanged

`agentd` is **not touched**. A content digest is self-authenticating against the sealed manifest, so
no `provenance-id` reference is needed (that seam, slice-7 §6, is for the mutable *log* / T-untrust,
where the record is not derivable from the bytes alone). The caller's proposed band remains an
audited proposal that `gatekeeperd`'s derivation overrides; a caller cannot influence the pin match.

## 7. Refusal & audit (fail-high, no downgrade)

- Miss / tamper / `ENODATA` / open-world pin ⇒ `T-hostile` ⇒ floor T2 (existing behaviour; refuse if
  no constructor, never a silent drop). A digest match with an open-world class stays `T-hostile`
  (no-laundering).
- **A `T-pinned` derivation itself REFUSES construction**, deterministically, with
  `SANDBOX-DECISION refused reason=pinned-exec-home-unavailable` and rc=15, at EVERY tier — no
  downward (T2) or upward (T1 fall-up) constructor workaround (classification-only scope; the bound
  fd is never executed).
- The `SANDBOX-PROVENANCE` audit line gains `pinned={bool}` (digest matched) and `exec_fd_bound={bool}`
  (the measured fd is bound to the derivation) beside the existing `derived`/`proposed`/`match`
  fields — so the classification, the fd-binding, and a systematic proposal/derivation mismatch are
  all legible on the serial console.

## 8. Phased scope & test topology

**In-slice:**
- `linux_uapi`: add `FS_IOC_MEASURE_VERITY` (+ the `fsverity_digest` struct) and a thin
  `measure_verity(fd) -> io::Result<Vec<u8>>` wrapper over the existing raw `ioctl`.
- `provenance_plane.rs`: `PinClass`, `Pin`, `PIN_MANIFEST` (empty in shipped image), a `pin_lookup`
  arm, and the `measure()` wiring in §5. Extend the audit token (§7).
- **Fixture:** reuse the sealed **gate-probe** binary (a genuine closed-world program, slice-7 §8.1)
  placed *also* on a writable, verity-enabled mount, its verity digest entered in a **test**
  `PIN_MANIFEST` with `class = ClosedWorld`. Same binary, two placements — the sealed-root copy earns
  `T-first`, the pinned writable copy earns `T-pinned`. This proves the pin path is **digest-based,
  not path-based**.

**Host/container oracle (before any VM):**
- Pure `derive_band` pin branches are already covered (provenance.rs:171–185). New oracle over the
  *store*: a verity-enabled fixture whose digest is in a test manifest ⇒ `pinned_digest_match` +
  `domain_execution_sealed` ⇒ `T-pinned`; **one byte flipped** ⇒ different verity digest ⇒ miss ⇒
  `T-hostile`; an **open-world**-classed pin entry ⇒ match but `T-hostile` (no-laundering); a
  **non-verity** fixture ⇒ `ENODATA` ⇒ `T-hostile`. Off a non-verity host, `sealed_root_dev()` is
  `None` and nothing fabricates `T-first`, exactly as slice-7 — the pin oracle uses its own fixture
  fs and does not depend on the sealed root.

**Sealed VM gate (~35-min cycle, before commit):**
- The gate provisions a **deterministic, dedicated fs-verity-capable writable filesystem** (a
  fixed-image loopback fs formatted with the verity feature). If the primitive is unavailable, the
  gate **FAILS — it does not skip** (amendment D): a silent skip would read as "pins work" when they
  were never exercised.
- The pinned gate-probe on that writable verity mount (verity enabled, digest in the sealed fixture
  manifest) **derives `T-pinned`** — `SANDBOX-PROVENANCE derived=T-pinned pinned=true
  exec_fd_bound=true` — proving pathname-independent classification AND the fd-binding; then the
  construction **refuses `pinned-exec-home-unavailable` (rc=15)** with no T0/T1/T2 construction
  (classification-only scope).
- Anti-spoof is a **second verity inode, one byte different** (amendment D): a separate
  verity-enabled file with content ≠ the pinned probe ⇒ its `(algorithm,digest)` is not in the
  manifest ⇒ `derived=T-hostile`. This is *not* in-place tamper — an fs-verity-enabled file is itself
  immutable, so a distinct unpinned inode is the real anti-spoof.
- Existing slice-7 gates (`T-first` sealed-root probe → constructs at real tier; foreign-`/bin/sh`
  anti-spoof) stay green — the pin arm only *adds* a positive-proof classification and never lowers a
  band, so T-first and hostile paths are unchanged.

**Implementation (S6, `image/overlay/usr/lib/shrek/mount-plane-gate`):**
- **Bake (build time, `build-in-container.sh`):** the sealed manifest is written into the overlay
  *before* mkosi seals `/usr` under dm-verity, pinning gate-probe's digest from an **offline** `fsverity
  digest --hash-alg=sha256 --block-size=4096`. fs-verity's digest is content-addressed (sha256 over
  4096-byte Merkle blocks), so this offline value is bit-identical to the kernel `FS_IOC_MEASURE_VERITY`
  measurement the gate takes at runtime — the gate proves the two agree on the sealed kernel, closing
  the classification loop without a writable manifest. The baked file is gitignored (a build artifact).
- **Runtime primitive:** the sealed root is *block-level* dm-verity (read-only), not per-file fs-verity,
  so the gate provisions a dedicated `mkfs.ext4 -b 4096 -O verity` loopback on the writable `/run`
  tmpfs (needs `e2fsprogs`, added to the image `Packages` for the gate — strip with the scaffolding),
  enables verity on the sealed gate-probe copy via `gatekeeperd pin-verity enable`, and drives the
  `sandbox --tier T0 --trust T-pinned` re-check against it.

## 9. Decisions — RESOLVED (boundary review, 2026-08-19)

- **(A) Digest primitive — RESOLVED:** `FS_IOC_MEASURE_VERITY`; identity = `(digest_algorithm,
  digest)`; unknown algorithm/size fails high; measured fd == executed fd, no reopen seam (§3).
- **(B) Manifest storage — RESOLVED (reversed to file):** a versioned sealed manifest **file** under
  dm-verity `/usr`, tiny dependency-free parser, fail-high on malformed/unknown/conflicting (§4).
  Chosen to separate mechanism from policy and to test the production binary with a fixture manifest.
- **(C) Per-entry execution class — RESOLVED (mandatory):** the sealed manifest is the authority;
  `OPEN_WORLD` is defined **behaviorally** — allows mutable/unmeasured bytes to become instructions
  (interpreter/JIT/plugin loader) ⇒ never a positive band (§4).
- **(D) VM-gate fs-verity — RESOLVED:** dedicated deterministic verity-capable writable fs; **fail,
  not skip**, if unavailable; anti-spoof = second one-byte-different verity inode (§8).
- **(E) Keep acceptance scaffolding — RESOLVED:** **do not spike-strip** while Phase 5 is active; the
  pinned-probe fixture + gate join the existing scaffolding.

## 10. What this slice deliberately does NOT do

- **No executable home for pinned code, and NO change to T0's `MS_NOEXEC` / Landlock-`EXECUTE`
  posture.** A `T-pinned` workload is not runnable here; it refuses `pinned-exec-home-unavailable`.
  The **execution-home is the next, separately-reviewed boundary**, which must compare at least:
  *(a)* a **dedicated exec mount** for pinned artifacts (exec-allowed because verity-immutable +
  digest-pinned); *(b)* **private materialization** (copy the measured bytes into a sandbox-private
  exec-capable store); *(c)* **Onion/sysext delivery** (the pinned artifact arrives as a signed layer
  on an exec+verity mount, slice-7 §4); *(d)* **higher-tier-only execution** (run pinned code only in
  a tier whose constructor already provides an exec+verity rootfs). No constructor security is touched
  until that boundary is reviewed.
- No §4 **mutable** plane, no TPM NV counter, no signed-manifest freshness — those are the A2/A3
  forks, owned by / sequenced with the grant-protocol slice (slice-7 §8, lines 233–235). A static
  image-baked pin list needs none of it.
- No `T-untrust` origin records — needs the mutable provenance log (A3).
- No interpreter/JIT content attestation — an open-world pin still fails high (slice-7 §5.1 deferred
  mechanism, unchanged).
- No `agentd` change, no wire-protocol change.

Net posture: **first-party sealed code ⇒ `T-first`; a digest-vetted, closed-world, image-pinned
artifact ⇒ `T-pinned`; everything else ⇒ `T-hostile`.** One more positive-proof arm on the
fail-high lattice, built with the integrity class §4 already specifies and zero new coupling.
