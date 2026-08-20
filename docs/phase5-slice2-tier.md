# Phase-5 slice-2 — the (trust × caps) → tier decision plane

Slice-1 built the **mount plane**: given a resolved sandbox spec, gatekeeperd constructs a
capability-enforced T1 sandbox in which a granted-out path is ABSENT (ENOENT), not merely
unreadable. Tier selection was stubbed to a hardcoded T1.

Slice-2 removes that stub. It implements the deterministic `(trust × caps) → tier` **decision**
(isolation.md §4–§5.2) and the **independent privileged re-check** (isolation.md §7), and wires them
so that anything the current runtime cannot yet safely build **fails closed** instead of silently
downgrading. It adds no new isolation technology — the one constructor is still slice-1's T1 nspawn.

## What this slice is (and is NOT)

At slice-2 time the matrix output T0/T1/T2/T3 but only T1 was constructible. Later slices changed
that: slice 3 added T1 egress, slice 4 added a genuine T0 Landlock constructor, and slice 6 added
the T2 gVisor constructor. Read this file as the **decision + floor + fail-closed refusal** slice;
[`phase5-slice4-t0.md`](phase5-slice4-t0.md), [`phase5-slice6-t2.md`](phase5-slice6-t2.md), and
[`phase5-slice7-trust-provenance.md`](phase5-slice7-trust-provenance.md) are the current sources for
constructor and provenance state.

**In scope:** the pure decision crate; agentd's resolver; gatekeeperd's independent re-check +
constructibility gate; empirical proof on host, oracle, and the sealed VM.

**Out of scope at slice-2 time, by explicit decision:** trust-band inference / provenance sourcing
(OPEN B1, owed to agents.md); the T2 (gVisor) / T3 (libkrun) constructors; a real T0 (Landlock)
constructor; the egress plane (nftables/tap); the crypto seal + socket transport (slice-5). T1
egress, real T0, T2, and B1 provenance have since landed; T3 and the socket/crypto seal remain
deferred.

## The decision — `crates/shrek-policy` (`tier` module)

A pure, total, dependency-free crate compiled into BOTH daemons, so the matrix + floor are baked
into every binary and sealed by dm-verity when shipped in `/usr`. (Renamed from `shrek-tier` in
slice-3 when the sealed egress-profile table joined it as a second `egress` policy module.)

- **Axes** (isolation.md §4): `TrustBand ∈ {First, Pinned, Untrust, Hostile}`; `CapsProfile ∈
  {RoNosec, ProjRw, Net, Broad}` — declaration order is the danger order, so `caps ⊆ profile`
  is `caps <= profile`.
- **`matrix(trust, caps)`** — isolation.md §5, verbatim (16 cells).
- **`floor(trust)`** — `First→T0, Pinned→T0, Untrust→T2, Hostile→T2` (§5.1).
- **`effective_tier = max(matrix, floor, escalation)`** over `T0 < T1 < T2 < T3` (§5.2). There is
  deliberately **no `min`** anywhere: caps never lower the wall; escalation only ever raises it.
- **Fail-high parse** (security-model.md §6/B1): an unrecognized trust label ⇒ `Hostile`, an
  unrecognized caps label ⇒ `Broad`. A spoofed/garbled label can only RAISE the wall. An
  unrecognized *tier* is `None` — the caller fails the request closed (it is a malformed request,
  not a weak wall).

## The contract — agentd resolves, gatekeeperd re-checks

Per isolation.md §7, transported over the slice-1 CLI seam (the socket verb + crypto seal are
slice-5):

```
agentd resolve  (unprivileged)      gatekeeperd sandbox --tier … (privileged broker)
  step 1: caps ⊆ granted profile      step 3: recompute bound = max(matrix, floor) from the
  step 2: tier = max(matrix,floor,esc)         COMPILED-IN (verity-sealed) table — trusts NONE
  emits:  the construction-request            of agentd's numbers
          argv on stdout                step 4: refuse if requested < bound      (downgrade)
                                        step 5: refuse if caps ⊄ profile         (caps-exceed)
                                        gate:   refuse if effective ≥ T2         (no constructor)
                                        gate:   refuse if caps need egress       (no egress plane)
                                        else:   construct at T1 (T0 folds up — legal upward esc.)
```

Two independent checks mean a bug or compromise in the unprivileged resolver **cannot** widen a
sandbox. The independence proven here is **arithmetic/floor** independence (gatekeeperd recomputes
the tier rather than trusting agentd's number). Integrity-sourcing the trust band *itself* is the
separate upstream OPEN (B1); here `trust`/`caps` still ride in with the request, and the fail-high
parse guarantees a garbled band only ever raises the wall.

Refusal exit codes: `10` downgrade-below-floor · `11` caps-exceed-profile · `12` no-constructor
(≥T2) · `13` no-egress-plane · `14` bad-request-tier.

## The constructible set today

At slice-2 commit time, only `effective ∈ {T0,T1}` **and** `caps ∈ {C-ro-nosec, C-proj-rw}` built
(at T1). That was 4 of 16 cells; the other 12 failed closed:

```
              C-ro-nosec   C-proj-rw   C-net        C-broad
 T-first      build(T1)    build(T1)   egress✗      egress✗
 T-pinned     build(T1)    build(T1)   egress✗      ≥T2✗
 T-untrust    ≥T2✗         ≥T2✗        ≥T2✗         ≥T2✗
 T-hostile    ≥T2✗         ≥T2✗        ≥T2✗         ≥T2✗
```

A stronger-tier requirement is NEVER satisfied by a weaker box just because the target constructor is
missing — that is the load-bearing negative (security-model.md §7: "no unconfined fallback, ever").
Current state differs only by adding real T0, T1 egress, and no-network T2 constructors.

## Gates

- **G1** (host unit): all 16 matrix cells + floor + escalation + ordering + fail-high parse —
  `cargo test -p shrek-policy` (10 tier tests; the crate now also carries 7 slice-3 egress tests).
  Slice-1's 7 gatekeeperd tests unchanged.
- **Host decision repro** — `scripts/tier-plane-repro.sh`: the resolver + every refusal code + the
  cleared path, no privilege needed (refusals return before the mount namespace).
- **Oracle construction proof** — `scripts/tier-plane-proof.sh` (privileged debian:trixie):
  **G2** a cleared request constructs at T1 with the caps property intact; **G3\*** a ⇒T2 request
  emits ZERO in-sandbox gate lines — the workload never runs.
- **G6 / VM gate** — folded into the slice-1 M4 gate (`image/overlay/usr/lib/shrek/mount-plane-gate`):
  on the sealed, enforcing-Secure-Boot, dm-verity image the compiled-in matrix is immutable, so the
  re-check has no writable input; the cleared/refused behaviour is identical to the oracle.

## Spike artifacts to strip before ship

`scripts/tier-plane-repro.sh`, `scripts/tier-plane-proof.sh`, and the slice-2 block appended to
`image/overlay/usr/lib/shrek/mount-plane-gate` (removed together with the slice-1 M4 gate, its unit,
and the `multi-user.target.wants` symlink).
