# Phase-5 Consolidation (provisional) — boundary & scope

**Status: BOUNDARY / SCOPE ONLY (2026-08-19). No code, no attack execution, no canonical-doc
edits yet.** This document scopes the Phase-5 closeout across three deliverables and surfaces the
forks that need an owner decision before the hostile pass runs. Per standing method it is brought back
for review **before** the adversarial pass executes; the review's findings, not this doc, determine
what can be published as Phase-5 guarantees.

Frozen baseline: `f7d3063` (slice-10 shipped). Explicitly **not** in scope this cycle:
`dlopen`-of-closure-member (slice-10 v2 / Fork F3), ≥T1 pinned containment (slice-9 Fork B), any new
capability, any code, any doc reconciliation. This cycle produces a *plan*, an *attack plan*, and a
*doc-authority plan* — nothing that touches the frozen constructor security surface.

Sequencing (locked): **(2) hostile review runs FIRST** (after this boundary is approved). Its output is
the honest guarantees / non-guarantees statement, which becomes the **Phase-6 interface contract**
higher layers (Swamp / Donkey / desktop) build against. (1) is the input to (2); (3) is scheduled last
because the canonical contract can only assert what (2) leaves standing.

---

## 1. Invariant → implementation → gate traceability map

Every security invariant mapped to the **exact code that enforces it** and the **gate that proves it**.
This is the INPUT to the hostile pass (§2): each row is a claim the attack plan tries to break, and any
row whose gate is weaker than its invariant is a hostile-pass target.

Line anchors are as of `f7d3063`. Enforcement crates: `gatekeeperd` (`mount_plane` / `proc_plane` /
`provenance_plane` / `sandbox` / `pin_manifest`) and `shrek-policy` (pure: `tier` / `provenance` /
`egress`). Gate columns: **U** = `cargo test` (gatekeeperd 57 lib + shrek-policy 25); **O** = host
oracle `scripts/pin-manifest-proof.sh` §1–§9 on genuine fs-verity; **V** = sealed-VM
`image/overlay/usr/lib/shrek/mount-plane-gate` (M4 / S2–S8, enforcing Secure Boot + dm-verity).

### 1a. Closure-execution invariants (slice-9 I1–I6 read over the whole closure; slice-10 I7–I11)

| Inv | Statement | Enforced by (symbol : line) | Gate |
|---|---|---|---|
| **I1** | Exec bound to the exact fs-verity inode/digest that earned `T-pinned`; no reopen/re-resolve seam between measure and exec | `provenance_plane::pin_arm:206` + `derive:289` (measured==carried `exec_fd`/`closure`); `mount_plane::relocate_exec_island:215` and `relocate_member:304` (re-verify `(dev,ino)`+`FS_IOC_MEASURE_VERITY` post-bind); `authenticate_closure:446` | O §1/§8; V S7-island-construct / S8-closure-construct; U `authenticate_closure_*` |
| **I2** | No mutable/unmeasured byte becomes an instruction | **Existing bytes:** `mount_plane::relocate_ro:149` (`MS_NOEXEC` grants), `seal_noexec_in_place:161`, `seal_subtree_noexec:404` (`/usr`), `seal_subtree_noexec_writable` (island root, F2). **New bytes (co-load-bearing):** `proc_plane::install_landlock` deny-all `MAKE_REG`/`WRITE` (`handled_fs_for_abi:69` incl. MAKE_REG) — no exec mount is writable by the workload. `grant_access:92` (no EXECUTE bit) | O §1/§8 (grant `mmap(PROT_EXEC)`+`execve`=EPERM; F2 island-flags self-check); V S7/S8 |
| **I3** | Exec surface built from derived evidence (`der.exec_fd`/`der.closure`), never a caller path/flag | `provenance_plane::Derivation.exec_fd:70` + `.closure:76` (only set on `Pinned` band, `derive:314`); `sandbox::construct:101`→`closure_to_spec:40` (spec from derivation, not request) | U derive/closure tests; O §1/§8 |
| **I4** | Any setup failure ⇒ refuse; no fall-back to noexec run / unpinned exec / weaker tier | `sandbox::recheck:251` refuse `rc=15` `pinned-exec-home-unavailable:436`; `authenticate_closure:446` + `build_closure_island:468` propagate `io::Error`→refuse | O §7/§9 (fail-closed); V S8-tamper (`rc=3`) |
| **I5** | Grant/writable `MS_NOEXEC`/no-EXECUTE posture stays intact; only closure inodes gain exec | `mount_plane::relocate_ro:149` + `seal_noexec_in_place:161`; `proc_plane::grant_access:92` vs `usr_access:86`; `build_exec_island:518` / `build_closure_island:468` drop NOEXEC for exactly the N island binds | O §1/§8; V S7/S8 |
| **I6** | Minimal blast radius; `shrek-policy` (matrix + `floor(Pinned)=T0`) frozen | `shrek-policy/tier.rs` unchanged since slice-2 (25 tests green); slice-9/10 confined to gatekeeperd constructor | U shrek-policy 25 (frozen); diff review |
| **I7** | Complete transitive closure — interp + every transitive `DT_NEEDED` is a manifest member; no SONAME resolves outside the sealed set | `proc_plane::authenticate_closure:446` (dep-free ELF parse: `PT_INTERP`==pinned interp AND every `DT_NEEDED`∈pinned libs); `pin_manifest::lookup_match:261` + `Closure:124` completeness | O §8 (closure runs) / §9 (missing member fail); U `authenticate_closure_matches_interp_and_needed_and_rejects_drift:951` |
| **I8** | No unsealed loader input maps executable bytes outside the closure | `mount_plane::seal_subtree_noexec:404` (`/usr` NOEXEC in-ns) + `seal_subtree_noexec_writable` (island root NOEXEC, F2) + `mask_with_empty:424` (empty bind over `ld.so.preload`/`ld.so.cache`); island binds at loader-visible paths (`relocate_member:304`); Landlock `MAKE_REG`/`WRITE` deny-all co-load-bearing for workload-created bytes | O §8 (+F2 island-flags self-check); V S8-closure-construct |
| **I9** | Interpreter runs with a fixed closed env; `LD_*` stripped | `proc_plane::build_closure_island:468` (loader-env sanitize) | O §8 (`LD_PRELOAD` of mutable object ignored) |
| **I10** | Sealed manifest + runtime `FS_IOC_MEASURE_VERITY` re-measure is authority; build-time enumeration only generates | `provenance_plane::measure:134` + `pin_arm:206` (open `O_RDONLY`→measure→assert==manifest); `mount_plane::relocate_member:304` re-measure post-bind | O §8 (offline bake==kernel measure); V S8-classify |
| **I11** | Closure fixed at construction; no arbitrary `dlopen` (v1) | `proc_plane::reject_if_dynamic:253` (static-route guard); `authenticate_closure:446` set fixed before exec; no runtime member add path exists | O §9; U `authenticate_closure_rejects_a_static_entrypoint:984` |

### 1b. Cross-cutting policy invariants (the trust/tier wall — `shrek-policy`, sealed by dm-verity)

| Inv | Statement | Enforced by (symbol : line) | Gate |
|---|---|---|---|
| **no-laundering / MS_NOEXEC** | The load-bearing instrument for I2/I8 — a `noexec` mount blocks `execve` AND file-backed `mmap(PROT_EXEC)` (mmap(2) `EPERM`) | `mount_plane::relocate_ro:149`, `seal_noexec_in_place:161`, `seal_subtree_noexec:404` | O §1/§8; V S7/S8 grant-mmap-exec-eperm |
| **fail-high** | Unknown/unverifiable **trust** ⇒ `Hostile`; unknown **caps** ⇒ `Broad` (strongest wall); malformed manifest ⇒ reject | `shrek-policy/tier.rs` `TrustBand::parse:136-141` (default `Hostile`), caps parse `:158` (default `Broad`); `pin_manifest::parse:176` fail-high on malformed/≠1-interp/dup key | U tier + pin_manifest parse tests; O §9; V S7-antispoof (rogue⇒T-hostile) |
| **no-fall-down / no-fall-up** | `effective_tier = max(matrix, floor, escalation)`; provenance sets a floor caps cannot buy back; a garbled *tier* is a malformed request (fail closed), not a weak wall | `shrek-policy/tier.rs` `floor:111-116` (`Pinned⇒T0`), `matrix:92-95`, `effective_tier:128`; `Tier::parse:187` strict (`None`⇒caller refuses); `sandbox::recheck:251` never constructs below floor | U `forged_downgrade_below_floor_is_refused:549`, `caps_exceed_profile_is_refused:557`; V S6/S7/S8 (`construct-at=T0`) |
| **frozen-policy** | Matrix + floor baked into every binary, dm-verity-sealed at rest; caps can only raise the wall | `shrek-policy` compiled-in (`tier.rs` module doc §4-16); sealed image dm-verity `/usr` | U shrek-policy 25; V (policy read on sealed kernel) |

### 1c. Map-pass fix flagged here (tier-3, fix during the pass — not its own event)

**M4 literal-FAIL nspawn argv-echo noise (F-M4).** The sealed-VM console carries one raw
`SHREK_GATE: FAIL` at the M4 stage that is **benign**: gatekeeperd echoes the nspawn argv (whose `-c`
probe SOURCE contained literal `SHREK_GATE: FAIL` strings in its `||` branches) to stderr/console. The
gate's anchored `emit`-verdict parsing already excludes it — the aggregate is correct and the Phase-5
security baseline is green — but the raw `FAIL` reads as a failure to a human scanning the console
(64 PASS / **1 raw FAIL** every boot). **Fix (applied, isolated in commit C — hygiene, not security):**
the M4 probe assembles the token from parts (`G="SHREK_GATE:"; P=PASS; F=FAIL` → `echo "$G $F …"`) so
the source carries no literal `SHREK_GATE: FAIL`; runtime stdout still expands to the real token, so the
anchored parser is unchanged. **VM-confirmation deferred** to the next mandatory sealed-image cycle
(no dedicated rebuild for a cosmetic change); that cycle adds the acceptance condition **0 raw
`SHREK_GATE:FAIL`** and closes the hygiene item. Anchored verdict parsing remains authoritative; this
does not weaken or recharacterize the already-green Phase-5 baseline.

---

## 2. Adversarial cross-slice composition review — ATTACK PLAN

**This is an attack plan, not a checklist.** The value is in *finding a path*, not confirming there
isn't one. Each slice was proven in isolation; the hostile pass hunts **emergent** authority-laundering
that only appears when slices *compose*. Three attacker win-conditions — any one is a Phase-5 breach:

- **W1 — get one unmeasured byte executed** (an instruction the manifest never measured).
- **W2 — upgrade one trust band** (make `Hostile`/`Untrust` derive `Pinned`/`First`).
- **W3 — cause one tier fall-down** (run below `floor(trust)`, or refuse-should-have-been-construct
  inverted into construct-should-have-been-refuse).
- **W4 — obtain capability-composition authority** (assemble, from legal per-plane grants, a combined
  authority surface — fs × exec × net — larger than the workload's `(trust, caps)` tuple entitles,
  *without* any single tier/trust violation). W4 is distinct from W3: the tier can be exactly correct
  while the *composed capability reach* across planes exceeds what was authorized. Seam **F** is its
  primary hunting ground (F2 caps⊄profile cross-plane leak, F4 escalation-term asymmetry), extended by
  any grant/egress/exec authority that composes across a slice boundary.

Method: **oracle-first, adversary-authored fixtures**, on genuine fs-verity in the privileged
container; VM only to confirm a live finding on the sealed kernel. No finding is "real" until a fixture
demonstrates it end-to-end (standing method: oracle before VM). Attacks are grouped by seam.

### A. Closure delivery convention (slice-10 §9) — member smuggling → W1
Delivery v1: members are same-basename fs-verity files in the **entrypoint's own directory**, resolved
via `DT_RPATH $ORIGIN/lib`. The attacker questions:
- **A1 — caller-influenced entry dir.** Can a caller choose an entry path whose sibling `lib/` dir
  contains attacker inodes, such that a member SONAME resolves to an *attacker* file that nonetheless
  passes `authenticate_closure`? Target: the gap between "same-basename in `$ORIGIN/lib`" (delivery)
  and "digest∈manifest" (authority). If authority truly re-measures every bound member
  (`relocate_member:304` → I10), a swapped inode must fail digest — the attack proves whether any member
  is bound *without* the re-measure, or resolved *after* the re-measure (a TOCTOU between authenticate
  and execve).
- **A2 — `$ORIGIN` re-computation.** `$ORIGIN` is the loader's view of the entrypoint's dir *inside the
  island*. If the island exec path and the caller's original dir diverge, does the pinned `ld.so`
  compute `$ORIGIN` against the island (safe) or leak the caller path (smuggle)? Fixture: entrypoint
  delivered from a caller dir with a decoy `lib/`.
- **A3 — basename collision.** Two closure members (or a member and the entrypoint) share a basename
  across `entry` dir and `lib/`; does the same-basename convention bind the wrong inode at one of the
  two loader-visible paths?

### B. `canonicalize`-then-bind seam (`mount_plane::relocate_member:304`) → W1
The load-bearing finding: member targets are `std::fs::canonicalize`d before bind + re-measure
(merged-usr `/lib64/ld-linux…` is symlink-to-symlink). Canonicalization is a **name→inode resolution
that runs in the constructor** — a classic TOCTOU surface.
- **B1 — canonicalize/measure/bind ordering.** Is the inode that gets *measured* provably the inode
  that gets *bound* and later *executed*? If `canonicalize` resolves a path, then a separate `openat`/
  bind re-resolves it, an attacker who can flip a symlink between the two wins W1. Read the exact
  sequence; author a fixture that races a symlink flip (or proves the fd, not the path, carries through).
- **B2 — canonicalize escaping the intended root.** Can a member path canonicalize *out* of the
  entrypoint dir / island into `/usr` or a grant, binding an inode the manifest measured at a different
  path? (I10 says digest is authority, so a matching digest is still safe — the attack is a *digest
  mismatch that is nonetheless bound/executed*, i.e. a measure-vs-bind inode divergence.)
- **B3 — not-yet-created lib target.** §9 notes lib targets under `/run` are parent-canonicalized +
  rejoined (target doesn't exist yet). Does the rejoin admit a `..` or symlinked parent that lands the
  bind off-island?

### C. `pin-verity` enable — writable-verity fixture verb → W1/W2 (must NEVER ship)
`gatekeeperd pin-verity` (`provenance_plane::pin_verity_cli:234`) can `FS_IOC_ENABLE_VERITY`. This is a
**spike/oracle verb** that turns a writable file into a verity inode — i.e. it can *mint* the exact
identity the manifest trusts. If it reaches a shipped image it is a direct W2 (attacker enables verity
on chosen bytes) and W1.
- **C1 — reachability audit.** Prove `pin-verity` is not dispatchable on the sealed image: not in the
  shipped CLI surface, gated behind a spike flag, or stripped by the pre-ship strip list. This is the
  single highest-severity seam — a fixture that invokes it on the sealed VM and succeeds is a ship
  blocker.
- **C2 — enable-then-pin composition.** Even in the oracle, confirm `pin-verity enable` on a mutable
  file followed by a `derive` does NOT yield `Pinned` unless the digest is already in the sealed
  manifest (it must not — but compose it and watch).

### D. `der.exec_fd` + closure carry across the P1/P2 fork + fd scrub → W1/W3
The derived fds are opened by the gatekeeper, then the constructor forks (P1 unshare ns, P2 pid1 scrub
FDs + Landlock + execve). The fd-scrub (`scrub_fds_except`) must exempt exactly the closure fds and
nothing else.
- **D1 — scrub exemption scope.** Does the scrub leak any non-closure fd (a gatekeeper fd, a manifest
  fd, a cgroup fd) into the workload — a confused-deputy handle? Conversely, does it scrub a closure fd
  the loader still needs, forcing a path-reopen fall-back that reintroduces a resolve seam (I1)?
- **D2 — closure carry integrity across unshare.** The ns-local-bind finding (#2626): binds must
  re-open by path in the child. Does *every* closure member get re-verified in the child ns, or does
  any member ride the parent fd across the fork and get bound without the child-ns re-measure (a W1 if
  the child-ns path resolves differently)?
- **D3 — P1→P2 authority ordering.** Is `authenticate_closure` provably *before* the point of no return
  (execve), and after the final bind/remount, so no member can change between authenticate and exec?

### E. fs-verity re-measure seams — "offline bake == kernel measure" assumption → W2
The whole pin chain rests on: offline `fsverity digest` (build) == runtime `FS_IOC_MEASURE_VERITY`
(kernel). S8 asserts byte-identity for the closure.
- **E1 — algorithm/params drift.** Does any path admit a digest computed with a different hash-alg or
  block-size than the kernel uses, such that a mismatch is silently treated as match (or a match is
  forged)? `pin_manifest` fail-high on unknown algo (`parse:176`) is the guard — attack an algo-id the
  parser accepts but the kernel measures differently.
- **E2 — non-verity inode measured.** If a member is on a filesystem *without* fs-verity (a grant, a
  tmpfs), does `measure` return an error (fail-closed, correct) or a zero/garbage digest that could
  collide? `is_verity_ro:99` is the RO-verity gate — probe a member on a verity-capable-but-not-enabled
  inode.
- **E3 — block-level dm-verity vs per-file fs-verity confusion.** The sealed root is block-level
  dm-verity; the pin path is per-file fs-verity on a loop ext4 `-O verity`. Does any code path treat a
  dm-verity `/usr` inode as if it carried a per-file verity digest (or vice-versa), admitting an
  unmeasured `/usr` byte as "pinned"?

### F. Capability-composition across the trust × caps matrix → W3
The matrix (`tier.rs:92-95`) + `floor` (`:111-116`) + `effective_tier = max(...)` (`:128`) is the wall.
Compose *legal* requests to find an *illegal* effective state.
- **F1 — floor vs matrix arithmetic.** For every `(trust, caps)` cell, does `effective_tier` ever fall
  below `floor(trust)`? Especially `(Pinned, RoNosec)⇒T0` vs `floor(Pinned)=T0` — is there a caps value
  that makes matrix return < floor and the `max` is the only thing saving it (confirm the `max` is not
  bypassable via an escalation term)?
- **F2 — caps ⊄ profile leakage.** `caps_exceed_profile_is_refused` proves the ⊆ check. Compose a
  request where caps look narrow to the matrix but the egress profile (`shrek-policy/egress.rs`) grants
  broader reach — does a `C-net` derivation at T1 compose with a profile that reaches beyond the sealed
  allow-list (a cross-plane authority leak: tier says contained, egress says open)?
- **F3 — unknown-tier request.** `Tier::parse` returns `None`⇒refuse. Compose a request with a garbled
  tier + a valid `Pinned` trust — confirm the refuse fires *before* any island construct (no
  half-constructed exec surface on a malformed request).
- **F4 — escalation term.** `effective_tier` includes an `escalation` max term. Can a caller supply an
  escalation that is honored *upward* (fine) but a paired path honors it *downward* on a later slice's
  code (fall-down)? Audit every reader of the escalation input.

### Deliverable of the hostile pass
A **guarantees / non-guarantees** statement (the Phase-6 contract). Non-guarantees already known and to
be *previewed* honestly (not discovered — these are design boundaries, stated so higher layers don't
assume them):
- writable grants are **bind-ro** (write-back deferred, **both** tiers);
- **T3 has no constructor** (fail-closed today);
- **`dlopen` excluded** (open-world → never pinnable, I11);
- **≥T1 pinned containment deferred** (slice-9 Fork B; `floor(Pinned)=T0` today);
- **anonymous JIT `PROT_EXEC`** governed by **classification, not runtime control** (closed-world class
  excludes interpreters/JITs; no memory-level enforcement).

Each hostile-pass finding either (a) becomes a fixed defect (with its own oracle fixture) or (b) is
demoted to an explicit **non-guarantee** in the contract. Nothing is hand-waved.

---

## 3. Doc-authority plan — HOW to reconcile (not the reconciliation)

**Decision locked by owner.** Roles:

- **CANONICAL CONTRACT (target):** `docs/architecture.md`, `docs/security-model.md`,
  `docs/threat-model.md`. After reconciliation these are the authoritative statement of what Phase-5
  *is* and *guarantees*. Higher layers cite these, not the slice docs.
- **HISTORICAL (frozen evidence):** `docs/phase5-slice{1..10}*.md`. Implementation/decision/evidence
  records. **Not edited** during reconciliation (they record what was true at ship). They are the
  *provenance* the canonical docs are reconciled *from*, not documents to keep in sync going forward.
- **DERIVATIVE (regenerated):** `docs/overview.md`, `docs/concept-to-code.md`. Regenerated *from* the
  canonical set; never a source of truth, never hand-edited to diverge from canonical.

### Reconciliation procedure (the HOW; reconciliation itself is a later cycle, gated on §2)
1. **Freeze the input.** The §1 traceability map + the §2 guarantees/non-guarantees statement are the
   *only* inputs to canonical edits. No canonical claim may be written that isn't backed by a §1 row
   (an enforcing symbol + a passing gate) or an explicit §2 non-guarantee.
2. **Per canonical doc, a claims ledger.** For `architecture` / `security-model` / `threat-model`,
   extract every *load-bearing claim* and tag it: `backed` (maps to a §1 row), `stale` (contradicted by
   as-built — e.g. any doc still saying `T-pinned` refuses, pre-slice-9), or `unbacked` (asserts a
   guarantee §2 could not stand up). `stale`/`unbacked` are the edit worklist.
3. **Reconcile stale→as-built, unbacked→non-guarantee.** Rewrite stale claims to the as-built mechanism
   (cite the slice doc as historical provenance, not as the contract). Demote unbacked guarantees to the
   explicit non-guarantee list.
4. **Regenerate derivatives.** `overview.md` + `concept-to-code.md` regenerate from the reconciled
   canonical set — a mechanical pass, diffed to confirm no independent claim leaked in.
5. **Codex-doc collision handling.** The working tree carries Codex-modified
   `architecture/security-model/threat-model` + `phase5-slice2-tier`/`phase5-slice7-trust-provenance`
   and new `overview`/`concept-to-code`/`glossary`/`trust-bands`. These are **left untouched this
   cycle** (per standing convention — never sweep Codex docs). Reconciliation must **fork from a clean
   canonical baseline**, not from the Codex-dirty working copies; the collision-resolution step
   (adopt / discard / merge each Codex delta) is itself a fork for owner decision (see F-DOC below).
6. **One selective commit at the end**, canonical + derivative only, never the historical slice docs,
   never Codex docs unless F-DOC resolves them in. AI-ref grep before commit; `gh` constant-itis.

---

## Forks — DECIDED (2026-08-19, by owner)

- **F-SEQ — as written.** §2 hostile pass first, §1 is its input, §3 (canonical reconciliation) last and
  **not started until the attack pass is dry**.
- **F-SCOPE — targeted, adjacency-bounded.** Run the six named seams A–F first; **expand only along an
  adjacent path discovered** by a live finding — no open-ended fuzzing campaign. **Require one complete
  dry rerun** (all fixtures green, no new finding) *after* the final finding/fix.
- **F-VM — oracle-first, sealed only where kernel reality matters.** Prove on genuine fs-verity in the
  oracle by default; boot the sealed VM **only** where a finding depends on sealed/kernel reality (e.g.
  lockdown, dm-verity vs per-file verity, Landlock-on-sealed-kernel). **One mandatory final composed
  sealed-VM acceptance run** before Phase-5 closeout.
- **F-DOC — fork from clean `f7d3063` HEAD.** Filenames (`architecture`/`security-model`/`threat-model`)
  are the target canonical set; **current contents are NOT automatically canonical**. Adjudicate Codex
  deltas **claim-by-claim against the §1 traceability map + §2 review** — a Codex claim is adopted only
  if it maps to an enforcing symbol + passing gate (or an explicit non-guarantee).
- **F-M4 — fold into consolidation harness hygiene**, kept **separate from security-finding patches**
  (its own hygiene change; never mixed into a W1–W4 fix commit).

---

**GO for the hostile pass only.** Attack execution follows (oracle-first). **No canonical-doc
reconciliation until the attack pass is dry.**

---

## §2 — HOSTILE PASS RESULTS (2026-08-19, first pass; A–F targeted)

Method held: source-authoritative reads of the enforcing code at each seam; execution confirmation
where reachability/kernel reality mattered (F-VM). Two findings; four seams probed and refuted. **Not
yet dry** — dry requires F1 fixed, F2 disposed, oracle coverage added for both, and one clean rerun.

### Finding F1 — `pin-verity` spike verb ships in the production binary (CONFIRMED, HIGH — ship blocker)
- **Where:** `crates/gatekeeperd/src/main.rs:396-397` dispatches `pin-verity` → `provenance_plane::
  pin_verity_cli:234` **unconditionally** — no `#[cfg(feature=…)]`, no spike guard (grep for
  `cfg/feature/spike` over `main.rs`/`provenance_plane.rs`/`Cargo.toml` returns empty).
- **Confirmed by execution:** the prebuilt `target/release/gatekeeperd` (shipped-equivalent) answers
  `gatekeeperd pin-verity` with `usage: gatekeeperd pin-verity <enable|measure> <path>` — the verb is
  live, not stripped.
- **Impact:** `enable` = `FS_IOC_ENABLE_VERITY` on an arbitrary path as **host-root** (gatekeeperd's
  privilege); `measure` = an fs-verity **digest oracle** over any readable file. The slice-8/10 docs
  assert "the shipped image has no writable verity fixtures … NOT a production verb," but that is an
  **unenforced manual checklist** — the code has no mechanism keeping it out of the shipped binary.
  This is precisely the seam-C invariant: *a writable-verity fixture verb MUST NEVER reach a shipped
  image.* Not a direct trust-upgrade by itself (a minted digest still isn't in the dm-verity-sealed
  manifest — W2 needs manifest custody), but it is unnecessary privileged attack surface that directly
  contradicts a stated security invariant, and a verity-enable/measure oracle is a composition
  primitive that does not belong in production.
- **Proposed fix (separate security patch):** `#[cfg(feature = "spike")]`-gate both the `main.rs`
  dispatch arm and `pin_verity_cli`, default-off; the oracle/VM builds opt in with
  `--features spike`. Add an oracle assertion that a **default** release build has **no** `pin-verity`
  dispatch (invocation falls through to the broker / unknown-verb). This makes "stripped before ship"
  a compile-time guarantee, not a checklist.

### Finding F2 — dynamic-path no-laundering for NEW files rests on Landlock, not MS_NOEXEC (PLAUSIBLE, MEDIUM — defense-in-depth + model accuracy)
- **Where:** `proc_plane::build_closure_island:468` forces `MS_NOEXEC` on `/usr` (`seal_subtree_noexec`)
  and on mutable grants (`seal_noexec_in_place`), but **not** on the writable island subtree
  `/run/shrek/<id>/exec[/lib]` (`island_path:238` / `island_lib_path:245`), which lives on host `/run`
  (no dedicated tmpfs mount — grep confirms) and is typically **exec-mounted**.
- **Why it is NOT exploitable today:** `install_landlock:619` is deny-all with `handled_fs_for_abi:69`
  including the full v1 low-13-bits (**MAKE_REG** among them), and **no rule grants MAKE_REG/WRITE**
  on the island dir or any exec-mounted path. So the workload cannot *create* a regular file there to
  `mmap(PROT_EXEC)`. The only place it can write — a `C-proj-rw` grant — is `MS_NOEXEC`. Both surfaces
  are covered; the closure ran and grant `mmap(PROT_EXEC)`=EPERM in oracle §8.
- **The finding:** the barrier for *workload-created* bytes is **Landlock `MAKE_REG`/`WRITE`
  deny-all** — a reliable Landlock right — **not** `MS_NOEXEC`. The §1 map (I2/I8) and the
  security-model attribute no-laundering **solely** to `MS_NOEXEC` ("*the* executable-mapping
  boundary; Landlock is only defense-in-depth on execve"). That attribution is **incomplete**:
  `MS_NOEXEC` is load-bearing for *existing* bytes (`/usr` + writable grants); for *new* bytes on any
  exec-capable mount the workload can see (host `/run` incl. the island dir, `/tmp`, `/dev/shm`) the
  co-load-bearing control is Landlock `MAKE_REG`/`WRITE` deny-all. A future change that grants
  `MAKE_REG`/`WRITE` on an exec mount, or introduces a writable exec mount without `NOEXEC`, reopens
  laundering with no `MS_NOEXEC` backstop on `/run`.
- **Proposed disposition (two parts, kept separate):** (a) **doc/model** — during §3 reconciliation,
  correct the security-model + §1 I2/I8 to name Landlock `MAKE_REG`/`WRITE` deny-all as a co-load-
  bearing no-laundering control alongside `MS_NOEXEC`; (b) **optional hardening patch** — seal the
  island `/run/shrek/<id>` subtree `MS_NOEXEC` *before* laying member binds on top (mirroring the
  `/usr` treatment: seal subtree, member binds re-add exec per-inode), so the property survives a
  future Landlock-rule regression. Recommend doing (b): it is cheap, matches an existing pattern, and
  turns a single-mechanism dependency into belt-and-suspenders.

### Probed and REFUTED (coverage; no finding)
- **A — member smuggling / caller-influenced `entry_dir`.** Sources come from the caller's entrypoint
  dir (`build_closure_island:490`), but every member is re-measured against the **sealed** manifest
  digest before AND after bind (`relocate_member:322-326`,`374-384`); a swapped inode fails the digest
  (preimage-resistant). The entrypoint itself must match a manifest `entry` digest. `$ORIGIN`/basename
  games change only *where* a bind lands, never the *content authority*. CLOSED.
- **B — canonicalize-then-bind TOCTOU.** The bind **source** is the measured fd `/proc/self/fd/N`
  (`relocate_member:357`), not a re-resolved path; the **target** is re-measured post-bind (`:374`).
  Path traversal in a manifest name is irrelevant — digest is authority, not path. CLOSED.
- **D — `exec_fd`/closure carry across the P1/P2 fork + scrub.** `sandbox_init_and_exec:737` runs
  `close_range(3, u32::MAX)` in P2 **before** Landlock/seccomp/execve; `exec_fd` is additionally
  `O_CLOEXEC` (`open_entry_rdonly:178`); the island survives as a **mount**, not an fd. No inherited
  privileged fd reaches the workload. CLOSED.
- **E — offline-bake == kernel-measure.** Any `measure_verity` failure / algo-id mismatch / non-verity
  inode ⇒ fail-closed miss (`pin_lookup_fd:191-198`, `pin_arm:219-222`, `relocate_member:323-326`).
  dm-verity (`st_dev`, `measure:150-155`) and per-file fs-verity (ioctl, `pin_arm:219`) are separate
  code paths — a dm-verity `/usr` file with no per-file verity returns `ENODATA` ⇒ pin miss; no
  confusion. CLOSED.
- **F / W4 — trust×caps matrix + cross-plane capability composition.** `recheck:251` enforces
  downward-forbidden (`requested < bound` ⇒ refuse `:261`), caps⊆profile (`:268`), T3 and T2-C-net
  fail-closed (`:275`,`:283`); egress destinations resolve from the **sealed compiled-in** table, never
  a caller-supplied host (`:300`); a `Pinned` cell escalated upward to T1 **refuses** (no ≥T1
  containment — `:408`→`:435`). No composition of legal per-plane grants exceeds the `(trust, caps)`
  tuple's authority (W4). CLOSED.

### Path to dry (F-SCOPE)
1. Land F1 fix (cfg-gate) as its own security patch + oracle "no `pin-verity` in default build" assertion.
2. Decide F2(b) hardening (recommend yes) — separate security patch; F2(a) doc correction deferred to §3.
3. Extend `pin-manifest-proof.sh` with a §-probe per finding (F1 dispatch-absent; F2, if taken, an
   island-subtree-NOEXEC assertion).
4. **One complete dry rerun** — full oracle green, no new finding — then the mandatory final composed
   sealed-VM acceptance run (F-VM) before closeout. Only then does §3 reconciliation begin.

**Hostile pass: first targeted A–F sweep COMPLETE. 2 findings (1 confirmed ship-blocker, 1 plausible
defense-in-depth/doc), 4 seams refuted.**

## §2b — FIXES APPLIED (2026-08-19; owner GO on both)

Both findings fixed; host oracle dry rerun GREEN (all A–F/W1–W4 invariant proofs + both new probes);
sealed-VM composed run pending. Fixes are separate concerns, not co-mingled.

### F1 fix — pin-verity surface compiled out of production (DONE, regression green)
- `crates/gatekeeperd/Cargo.toml`: new default-OFF `spike` feature.
- `#[cfg(feature = "spike")]` on the entire surface: `main.rs` dispatch arm, `provenance_plane::
  pin_verity_cli`, and `linux_uapi::{enable_verity, FS_IOC_ENABLE_VERITY, FsverityEnableArg}`.
  `measure_verity` (read-only, production) untouched.
- Consumers switched to opt in: `pin-manifest-proof.sh` and `build-in-container.sh` build gatekeeperd
  `--features spike` (the VM gate drives `pin-verity` at runtime; the whole gate is spike-only
  scaffolding — a ship build omits both the feature and the gate).
- **Regression:** `scripts/spike-stripped-proof.sh` — builds both ways, asserts the pin-verity usage
  literal is **absent** from a default artifact and **present** under `--features spike`. GREEN
  (2 pass). Default `cargo build` compiles with the surface gone; 57+25 unit tests green both profiles.

### F2 fix — island root NOEXEC + co-load-bearing Landlock, with a fail-closed self-check (DONE, proven)
- `mount_plane::seal_subtree_noexec_writable` — seals a subtree `MS_NOEXEC|NOSUID|NODEV` **without RO**
  (writable so bind-targets can be created under it).
- `proc_plane`: `build_closure_island` and `build_exec_island` now seal the writable island root
  `/run/shrek/<id>/exec` `MS_NOEXEC` **before** any entry/member bind; each re-verified member bind laid
  on top re-adds exec for exactly its one inode (same seal-then-reopen-per-inode pattern as `/usr`).
- `linux_uapi::path_is_noexec` (`statfs`/`ST_NOEXEC`) + `proc_plane::verify_island_exec_flags` — a
  **fail-closed self-check** run in P1: asserts the island root is `noexec` while the entry island and
  every member bind are **independently exec-capable** (their own mount lacks `noexec`); emits
  `SANDBOX-ISLAND-FLAGS parent-noexec=1 members-exec-ok=1`. A regression that drops the seal or wrongly
  NOEXECs a member aborts construction (no laundering surface opens, no silent break).
- **Proof (both directions, oracle §1 static + §8 dynamic, GREEN):** *positive* — `island-ran` under the
  now-sealed parent (the pin could not run if a member bind were noexec); *direct* — the
  `SANDBOX-ISLAND-FLAGS` statfs line (root noexec, entry+libs not-noexec). This is exactly the required
  "member mount flags remain independently exec-capable after parent NOEXEC" proof.
- **Doc correction (F2a):** §1 I2/I8 above updated to name Landlock `MAKE_REG`/`WRITE` deny-all as a
  co-load-bearing no-laundering control alongside `MS_NOEXEC`. The canonical security-model carries the
  same correction during §3 reconciliation.

**Dry rerun status: DRY.** host oracle `pin-manifest-proof.sh` ALL PASS (§1–§9, no regression + 2 new
F2 probes); `spike-stripped-proof.sh` PASS; 57+25 unit tests green both profiles.

**Composed sealed-VM acceptance run (F-VM): GREEN** (image v11, `out/vm-console.log`, enforcing Secure
Boot + dm-verity + `lockdown=integrity`, kernel cmdline `lsm=landlock,lockdown,yama,integrity,...`).
**M4 / S2 / S3 / S4 / S5 / S6 / S7 / S8 ALL PASS** — 64 emitted PASS, qemu rc=0 clean poweroff.
The two new F2 assertions passed on the sealed kernel:
- `PASS S7-island-flags (root NOEXEC, entry mount independently exec-capable)`
- `PASS S8-island-flags (root NOEXEC, all closure-member mounts independently exec-capable)`
The single raw `SHREK_GATE: FAIL` on the console is the pre-existing **benign M4 nspawn argv-echo**
(gatekeeperd echoes the nspawn argv whose `-c` probe body contains literal `SHREK_GATE: FAIL` strings;
the real gate emits `PASS gate=in-project-readable`) — the F-M4 harness-hygiene target, not a gate
failure. The gate counts only anchored `emit` verdicts, all of which are PASS.

**The hostile pass is DRY. §3 canonical-doc reconciliation is now unblocked** and forks from the clean
post-consolidation baseline (after commits A/B/C land), not `f7d3063`. F-M4 hygiene fix applied and
isolated in commit C, VM-confirmation deferred to the next image cycle (acceptance: 0 raw
`SHREK_GATE:FAIL`).
