# Phase 5 — Slice 7: trust-band provenance (B1)

> Status: **BOUNDARY ACCEPTED (with tightenings, folded in below) — BUILD-GO for Slice-7.**
> Decisions locked: A = MVP spine / T-first-via-dm-verity / fail-high everything else · B = reuse
> existing §4 custody roots, no new PKI · C = entrypoint granularity, where `st_dev` proves **sealed
> entrypoint provenance only** and the no-laundering rule (§5.1) requires a **separate** gatekeeper-
> derived `domain_execution_sealed` fact from a sealed execution profile that excludes arbitrary/
> interpreted/generated-code execution (`noexec` = defense-in-depth, not the proof). No other scope
> expansion; interpreter/JIT/plugin/generated-code provenance deferred.
> Trust selects the wall. This slice fixes *how the wall's index is earned* — the one input the
> whole `(trust × caps) → tier` matrix rides on, and today the only value in the pipeline still
> **asserted by the caller** rather than derived from integrity.

This is the OPEN **B1** — the ADV-2 "single highest-leverage soft spot" (threat-model.md ADV-2,
§7.2) and the upstream input to the matrix realized in [`crates/shrek-policy/src/tier.rs`](../crates/shrek-policy/src/tier.rs).
The tier plane (slice-2) already *consumes* a band and fail-highs an unknown one; the code object
being executed is never measured, and `sandbox.rs` marks this exactly:

> `crates/gatekeeperd/src/sandbox.rs` — *"Integrity-sourcing the trust band ITSELF is OPEN B1, a
> separate upstream slice; here `trust`/`caps` still ride in with the request, and the fail-high
> parse guarantees a garbled band can only raise the wall."*

## 1. What is already locked (do NOT relitigate)

Pinned upstream; this slice inherits them verbatim:

- **The security requirement (security-model.md §6/B1).** The band is derived ONLY from an
  integrity-checked source — *(a)* a signature over the code/manifest by a trusted authority
  (`T-first`/`T-pinned`), OR *(b)* a measured provenance record in the sealed log (§4) — **never an
  attacker-writable label. UNKNOWN or UNVERIFIABLE ⇒ `T-hostile`** (fail-safe HIGH, isolation.md
  §5.2: escalation is upward-only). This slice does not re-decide the requirement; it fixes the
  *mechanism* the requirement left open.
- **The sealed policy plane (security-model.md §4)** is the integrity substrate: static policy
  baked under dm-verity in the image; mutable state fs-verity-sealed under a signed manifest bound
  to a TPM NV monotonic counter. Trust roots are *of this custody class* — never a file on writable
  `/home`,`/var`.
- **The matrix, floor, and 4 bands** (isolation.md §5, trust-bands.md) are fixed. `floor(T-first)=T0`,
  `floor(T-pinned)=T0`, `floor(T-untrust)=T2`, `floor(T-hostile)=T2`.
- **`agentd` proposes, `gatekeeperd` re-checks** (isolation.md §7, ADV-8; slice-2 re-check
  invariant). B3 ("recheck source independence") is closed by §4. This slice extends that same
  invariant to the *band itself*.
- **Fail-high parse already ships** (`shrek_policy::TrustBand::parse`: unknown ⇒ `T-hostile`). This
  slice makes the band's *origin* integrity-sourced; it does not weaken the parse.

## 2. The gap this slice closes

Everything downstream of the band is deterministic and re-checked from sealed sources. The band is
the last caller-asserted value: `agentd` receives `--trust=<band>` (or defaults) and forwards a
*string*; `gatekeeperd` parses it fail-high but has **no independent way to contradict a caller who
claims `T-first`**. If an attacker gets their code labelled `T-first`/`T-pinned`, the matrix hands
them T0/T1 (shared kernel) and the floor drops (threat-model.md ADV-2 soft spot, §7.2, N-3/downgrade
§downward). **Provenance spoofing collapses `T-hostile → T-first`. That is the #1 thing to harden.**

The fix is structural, not a bigger allow-list: **the band is not transmitted as authority. Only a
reference to evidence is transmitted, and `gatekeeperd` re-derives the band by measuring the code
object that is about to execute against its own sealed/compiled-in trust roots** — trusting nothing
from `agentd`. This is the mount-plane move (slice-1: pin the fd, measure identity, re-verify)
applied to *code provenance* instead of *mount sources*.

## 3. The derivation model — evidence per band

Derivation is a **monotonic fail-high lattice**, evaluated strongest-first. Every "no" moves toward
`T-hostile`; the band is the *strongest* claim that is cryptographically/measurably proven, and
`T-hostile` is the floor of the lattice (the answer whenever nothing stronger is proven).

```
derive_band(code_object, evidence) :=
  if  code_object is measured under the sealed dm-verity image root
      OR carries a valid signature by the Shrek image-signing root      → T-first
  elif code_object's content digest matches an entry in the sealed
      pin-manifest (static §4 or counter-anchored mutable §4)           → T-pinned
  elif a measured provenance record in the sealed log (§4) AFFIRMATIVELY
      classifies its origin as untrusted-ingest (cloned/downloaded)     → T-untrust
  else  (agent-authored/generated record, no record at all, an
         unverifiable record, OR a proposal/derivation mismatch)        → T-hostile
```

**Resolution invariant (the load-bearing rule):** `T-hostile` is the unambiguous floor of the
lattice. Every band **above** `T-hostile` — including `T-untrust` — requires its **own affirmative
qualifying evidence**; `T-untrust` is *not* the default for "no strong proof." Anything that is
unknown, unverifiable, absent, or a mismatch resolves to `T-hostile`, always. There is exactly one
answer for doubt, and it is the strongest wall the floor allows.

The asymmetry is the point and is deliberately fail-high:

- **`T-first` / `T-pinned` require a POSITIVE proof** of the artifact — a measurement under the
  sealed root, a signature by a sealed root, or a digest match against a sealed pin-manifest. First-
  party code overwhelmingly *is* the sealed image / a Shrek-signed sysext, so `T-first` is mostly
  "the entrypoint resolves to a dm-verity-measured object" — the strongest evidence available, and
  one we already produce (slice-6 seals T2 artifacts under verity `/usr`).
- **`T-untrust` also requires an affirmative record** — a measured, integrity-checked provenance
  entry classifying the origin as untrusted-ingest. It refines legibility (untrust vs hostile) but
  never lowers the floor below T2. **Absence of any qualifying evidence is `T-hostile`, not
  `T-untrust`** — the weaker band still has to be earned.

## 4. Trust roots — reuse existing custody, sealed / compiled-in

**No new PKI.** The task's constraint — *gatekeeperd verifies from its own sealed/compiled-in trust
roots* — is met by reusing the three custody classes the system already defines (security-model §2,
§4), so there is no new key to protect and no new attacker surface:

| Band evidence | Root | Custody (existing) |
|---|---|---|
| `T-first` — sealed-image measurement / Shrek signature | Shrek image-signing key / dm-verity root hash | §2 boot trust; compiled-in / under dm-verity |
| `T-pinned` — pin-manifest digest match | sealed pin-manifest | §4 static (image) or §4 mutable (counter-anchored) |
| `T-untrust`/`T-hostile` — origin record | sealed provenance log verify key + TPM NV counter | §4 mutable plane machinery |

`oniond` trust roots (§4 static) fold into `T-pinned`/`T-first` when code arrives as a signed Onion
layer. `gatekeeperd` reads all roots from the sealed plane, independent of `agentd` — the same
independence §4 already gives the matrix/floor/profile.

## 5. Binding — measure the code object, not a path or a label

A valid signature over *some* code does not authorize *this* invocation. The evidence must bind to
the **exact object about to execute**. `gatekeeperd`, at construct time (mirroring `mount_plane`):

1. Pins the workload entrypoint object with `openat2` (RESOLVE_NO_MAGICLINKS / beneath a pinned
   root — the slice-1/§6 B4 pattern), obtaining an fd, not a re-resolvable pathname.
2. Measures identity from that fd (`statx` + the relevant content/verity digest).
3. Derives the band (§3) from the *measurement*, against sealed roots (§4).
4. Constructs at `max(matrix[derived_band][caps], floor(derived_band), escalation)`.

The band is bound to what is *actually* on the fd, closing the path/label TOCTOU: swapping the file
after classification changes the measurement, which re-derives the band (or fails high).

**What is "the code object"?** The workload entrypoint artifact: for `T-first` it is a
dm-verity-measured binary under `/usr`; for untrusted code it is the script/binary in a grant path,
whose *measurement matches no sealed root* ⇒ falls to the untrust/hostile arm. Entrypoint
granularity is the MVP measurement unit (Decision C), **bounded by the rule below.**

### 5.1 The no-laundering domain rule (Decision C bound — mandatory)

`st_dev` equality proves exactly one thing: **sealed entrypoint provenance** — the entrypoint binary
is authentic and resident on the boot-authenticated dm-verity root. It says **nothing about what that
entrypoint does once running.** A sealed interpreter, JIT, plugin/extension host, or any program that
reads-then-executes external bytes can run mutable, attacker-controlled content **without that content
ever being executable to the kernel** — so `noexec` / no-`+x` on writable mounts never sees it and
cannot prevent it. `python3 /grant/evil.py` under a sealed (T-first) `python3` would launder `T-first`
onto `evil.py`. Mount flags are the wrong instrument for this; they gate `execve`, not interpretation.

So the two facts the `Evidence` type already separates must be derived from **different** sources:

- **`entrypoint_sealed`** ⇐ `st_dev` equality (§5). Proves the entrypoint's *provenance* only.
- **`domain_execution_sealed`** ⇐ a **gatekeeper-derived fact from a sealed EXECUTION PROFILE that
  excludes arbitrary / interpreted / generated-code execution.** It is derived from compiled-in
  sealed policy, **never from mount flags and never from the caller.** Any resolved profile *capable*
  of arbitrary code execution — an interpreter, a JIT, a plugin/extension host, a generated-code
  runner — **cannot earn `T-first` through this arm**: the fact is false for it and it fails high to
  `T-hostile` (until a future affirmative provenance store earns it a band).

```
T-first via this arm requires BOTH:
  (1) entrypoint_sealed        — entrypoint resolves to the sealed dm-verity root (st_dev match)
  (2) domain_execution_sealed  — the resolved sealed execution profile is CLOSED-WORLD: a fixed
                                 sealed program that does NOT read-and-execute external / mutable /
                                 interpreted / generated code. Gatekeeper-derived from compiled-in
                                 sealed policy. NOT a mount flag. NOT a caller claim.
An OPEN-WORLD profile (interpreter / JIT / plugin host / arbitrary-exec-capable) FAILS HIGH — no
transitive T-first, no laundering. Unknown execution class ⇒ not closed-world ⇒ fail high.
```

**`noexec` / no-`+x` on writable & grant mounts stays — as defense-in-depth, not as the proof.** It
closes the *kernel*-`execve` path (a dropped-binary direct-exec) cheaply and is worth keeping, but it
is not what establishes `domain_execution_sealed` and must never be mistaken for it.

**Deferred — general interpreter / JIT / plugin-host / generated-code provenance:** a mechanism to
attest the *content* an open-world executor runs (so a sealed interpreter fed only sealed, measured
scripts could earn a band) is out of MVP scope. Until it exists, arbitrary-code-capable profiles
simply fail high — the correct fail-safe. (Fine-grained per-`dlopen`/per-`exec` content measurement
is part of this deferred mechanism, not the MVP.)

## 6. Wire-contract change — agentd proposes, gatekeeperd re-derives

The isolation §7 sealed construction request **already carries a `provenance-id`** field that is
currently unused. This slice activates it and demotes the raw band from authority to *proposal*.

**`provenance-id` is a non-authoritative, opaque, content-addressed evidence reference — NOT a
caller-selected evidence path.** It is a handle (a content digest / sealed-log key) that
`gatekeeperd` **resolves itself** against the sealed roots; the caller cannot hand gatekeeperd "the
evidence file to read." A caller-chosen path would just be the writable-label attack renamed (ADV-8):
point the broker at attacker-authored "evidence" and forge any band. Because the id is
content-addressed, a mismatch between the id and what gatekeeperd actually measures on the code
object resolves `T-hostile` (§3 resolution invariant) — the id can *locate* evidence, never *assert*
a conclusion.

```
agentd (unprivileged)
  - resolves a band PROPOSAL for legibility/audit (best-effort; may be wrong or absent)
  - emits sealed request: { tier_proposal, mount-set, net-set, limits,
                            provenance-id,            # ref into the sealed provenance log (§4)
                            code-object reference }   # path pinned+measured by gatekeeperd, not trusted

gatekeeperd (privileged broker — the wall)
  - RE-DERIVES the band from the measured code object + sealed roots + provenance-id (§3/§5),
    trusting NEITHER agentd's proposed band NOR any writable label
  - if re-derived band ≠ agentd's proposal → gatekeeperd's derivation WINS; the discrepancy is an
    audited event (a wrong/optimistic proposal never widens the wall — it only ever gets corrected up)
  - recomputes effective_tier from the re-derived band and constructs, or refuses (audited)
```

This is the slice-2 re-check invariant extended one hop upstream: previously `gatekeeperd` re-checked
`caps ⊆ profile` and `tier ≥ max(matrix,floor)` but *took the band on faith*. Now the band is
re-derived too. ADV-8 "trust nothing from the caller" now covers the last un-re-checked input.

## 7. Refusal & audit semantics (fail-high, no downgrade)

- Unverifiable evidence ⇒ `T-hostile` ⇒ floor T2. If no T2/T3 constructor exists for the resulting
  cell, the request **refuses** (existing fail-closed behavior; slice-6 rc=12 no-constructor) —
  never a silent drop to T1. Downgrade below `max(matrix,floor)` stays an audited rejection
  (isolation §5.2), not a warning.
- A band-proposal/derivation mismatch is logged as a first-class provenance event (agents.md §9),
  with both values and the measurement, so an `agentd` compromise that systematically over-claims
  `T-first` is *visible*, not just contained.

## 8. Phased scope — what the FIRST implementation slice delivers

Fail-high lets us ship the derivation spine before the full supply-chain evidence stores exist —
because "no store yet ⇒ no positive proof ⇒ T-hostile" is a *safe* default, not a gap.

**In-slice (recommended MVP):**
- `shrek_policy::derive_band(evidence) → TrustBand`: the pure, unit-tested §3 lattice (fail-high),
  compiled into both daemons (⇒ dm-verity-sealed, like the matrix).
- `gatekeeperd` re-derivation: pin+measure the code object (§5) for `entrypoint_sealed` via the
  dm-verity `st_dev` match we already have (slice-6), AND derive `domain_execution_sealed` from a
  compiled-in sealed **execution-class** for the resolved profile (§5.1): closed-world ⇒ true;
  open-world (interpreter/JIT/plugin host / arbitrary-exec) or unknown ⇒ false ⇒ fail high. `T-first`
  needs both; everything else ⇒ `T-hostile`.
- Activate the `provenance-id` seam and the proposal/re-derivation split (§6); demote the raw
  `--trust` string to an audited proposal.
- `noexec`/no-`+x` on writable & grant mounts as defense-in-depth (not the proof).

**Deferred (documented, not gaps — each needs an evidence store not yet built):**
- `T-pinned` via a sealed pin-manifest (needs the §4 static/mutable pin list + digest-match path).
- `T-untrust` origin records (needs the §4 mutable provenance log + TPM NV counter — the machinery
  the **grant-protocol slice** lands first; B1 consumes it, does not build it).
- The taint/endorsement bridge: content endorsement (grant-protocol.md D2/§8b) is a *data*-taint
  flip; its relationship to *code* bands is a later reconciliation, not this slice.

Net posture of the MVP: **only code we can prove is first-party runs below T2; everything else is
`T-hostile` until its evidence plane ships.** That is the correct fail-safe ordering and is exactly
the "T2/T3 refuse rather than downgrade" property applied to provenance.

### 8.1 Acceptance fixture — the sealed gate-probe (forced by B1, not feature scope)

B1 correctly reclassifies a shell/interpreter as **open-world** (§5.1), so `/bin/sh -c <probe>` — the
vehicle the existing T0/T1 gates use to assert in-sandbox isolation mechanics — can no longer
legitimately derive `T-first`; it derives `T-hostile` and floors at T2. Re-proving the T0/T1
*constructor mechanics* end-to-end through the real B1-derived path therefore needs a **sealed
closed-world gate-probe**: a tiny fixed program (not an interpreter) that performs the checks and
emits fixed `SHREK_GATE:` lines. This is **acceptance infrastructure required by B1**, not general
feature scope.

Probe invariants (a genuine closed-world sealed program, so it legitimately earns `T-first`):
- sealed on the dm-verity root; enrolled in the compiled-in closed-world list.
- **no child `exec`, no `dlopen` of mutable objects, no interpreted/generated code.**
- **arguments are treated only as DATA** (which checks to run / which paths to stat) — never as code.
- **fixed, enumerated `SHREK_GATE:` outputs.**

**Legacy container oracles (bounded follow-up).** The pre-B1 `scripts/{tier-plane-repro,
tier-plane-proof,t0-construct-proof,t2-construct-proof,egress-construct-proof}.sh` inject a band via
`--trust` and predate derive-by-default; off a real verity root they now derive `T-hostile`, so their
synthetic-band assertions no longer hold. Their coverage is superseded here: pure policy ⇒ the
`derive_band`/`recheck` unit tests; fail-high/anti-spoof ⇒ `b1-provenance-proof.sh`; constructor
mechanics end-to-end ⇒ the sealed-VM `mount-plane-gate`. A B1-aware rewrite of those dev oracles is a
follow-up; they are left untouched (not deleted) for now.

**Test topology (no faked T-first anywhere):**
- **Fast no-verity oracle** owns: provenance **fail-high** + **anti-spoof** + **mismatch**
  (`b1-provenance-proof`); the **pure policy** (`derive_band` / `recheck` unit tests); and
  constructor **mechanics** at a **non-production test seam** (the constructors are exercised without
  the B1-derived production path — no `T-first` is fabricated, because `sealed_root_dev()` is `None`
  off a real verity root).
- **Sealed VM** owns the **full end-to-end**: real dm-verity ⇒ the gate-probe derives `T-first` ⇒
  constructs at T0/T1 ⇒ the in-sandbox mechanics gates (grant-readable, vault-denied/absent,
  private-users, loopback-only, Landlock/seccomp EPERM) run **at their real tier** — assertions
  unchanged from the frozen gates. A planted foreign entrypoint proposing `T-first` derives
  `T-hostile` and is refused (anti-spoof). Note the reachable-derivation set (MVP): `T-first`+ro/proj
  ⇒ T0, `T-first`+ro-nosec escalated ⇒ T1, `T-hostile`+ro-nosec ⇒ T2; `T-pinned`/`T-untrust` cells
  are unreachable (no store) and belong to the pure seam. T2/T3 workloads run in their own rootfs, so
  their *host-path* entrypoint derives `T-hostile` by default — the correct fail-safe (rootfs
  provenance is the deferred general mechanism).

## 9. Open decisions — for boundary review

- **(A) MVP scope ambition.** *Recommend:* the §8 MVP — derivation spine + `T-first`-via-verity +
  fail-high-everything-else, no coupling to the not-yet-built §4 mutable plane. Alternative: wait
  and land B1 *with* the pin-manifest (T-pinned) so downloaded/pinned code can run at <T2 sooner —
  but that couples B1 to unbuilt supply-chain work and delays closing the ADV-2 soft spot.
- **(B) Trust-root reuse vs a dedicated band authority.** *Recommend:* reuse existing custody (§4) —
  no new key. Alternative: a separate band-signing root (more moving parts, more to protect; no
  benefit I can see).
- **(C) Measurement granularity.** *Recommend:* measure the **entrypoint artifact** for the `T-first`
  arm (cheap, exact, matches what actually executes first). Alternative: whole-bundle/tree
  measurement (needed eventually for multi-file untrusted workloads; heavier — defer with T-untrust).
- **(D) Doc reconciliation — DONE in this slice.** `agents.md` §4 used the older 3-band vocabulary
  (trusted/semi/hostile) with a `hostile → T3` floor, which **conflicted** with the canonical
  isolation §5.1 4-band table (`T-hostile → T2`). Reconciled to the 4-band canonical (`T-hostile`
  floor = T2; T3 is reached only where the matrix/escalation raises it, never as the band floor).
  `trust-bands.md` (the outward *what*) and this doc (the *how-derived*) are complementary.

## 10. Oracle & gate plan (per standing method)

- **Host/container oracle** (before any VM): a pure-Rust unit suite over `derive_band` (every lattice
  branch, unknown/absent ⇒ `T-hostile`), plus a gatekeeperd oracle that pins+measures a fixture
  entrypoint under a bind-mounted read-only verity-shaped tree and asserts: closed-world entrypoint
  measured-in-root ⇒ `T-first`; any tampered/foreign object ⇒ `T-hostile`; a caller claiming
  `T-first` over a foreign object is **corrected down to T-hostile** (the anti-spoof assertion); and
  a sealed **open-world** entrypoint (interpreter fixture) measured-in-root ⇒ `T-hostile`, **not**
  `T-first` (the no-laundering assertion — `entrypoint_sealed` true but `domain_execution_sealed`
  false).
- **Empirical VM gate** (the ~35-min cycle) before commit: on the sealed image, the **sealed
  gate-probe** (§8.1) derives `T-first` from real dm-verity and constructs at T0 (and at T1 via
  upward escalation), with the in-sandbox mechanics gates asserted **at their real tier** (frozen
  assertions unchanged); a planted foreign/`/bin/sh` entrypoint proposing `T-first` re-derives
  `T-hostile` and is refused or floored (audited rc, no T0/T1 downgrade). The `SANDBOX-PROVENANCE`
  line records `derived`/`proposed`/`match` for the mismatch audit.
- Commit selectively (this doc + slice code only; never sweep unrelated uncommitted docs). No
  third-party tooling attributions in-tree.
