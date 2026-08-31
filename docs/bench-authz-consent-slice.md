# Bench authorization — the console consent ceremony (DRAFT, pending Fable review)

> **Status: DRAFT design for review.** This is step 3 of the bench authorization slice (steps 1+2 —
> socket transport — are shipped and green). It is a **build slice of `grant-protocol.md`**, applied to the
> three authority-increasing bench verbs. Do NOT implement until this design is reviewed. The review is the
> next action; this doc is the artifact to review against the code.

## Why this exists

Steps 1+2 moved every bench verb onto gatekeeperd's authenticated socket (`/run/shrek-gk.sock`, SO_PEERCRED)
and refused the three **authority-increasing** verbs (`grant`, `network`-to-a-profile, `export`) fail-closed
with `needs-consent-ceremony`. This slice builds that ceremony: a human OK on a surface a sandboxed agent
cannot spoof, so that nothing in the desktop session (a hijacked or overreaching agent included) can expand
its own authority silently. The ceremony gates **authority configuration**, not operation: a `run` inside an
already-granted workshop never re-prompts.

## Owner hard invariants (non-negotiable)

1. **Fail closed on ANY ceremony unavailability.** No active VT, render failure, answer timeout, malformed
   request, daemon restart mid-ceremony, or client disconnect must ALL deny. No fallback to approval, no
   fallback to sudo, anywhere.
2. **Approval bound to the exact tuple** `(peer uid, pid, start-time, verb, full decoded argv,
   server-resolved target, gatekeeperd-issued nonce)`. An approved benign request cannot be swapped for a
   different operation. Re-read start-time at apply time (PID-reuse / TOCTOU defense).
3. **Ceremony gates authority CONFIGURATION only** (grant/network/export), never operations under an existing
   grant.
4. **Headless / remote authority expansion fails closed** and requires pre-authorized (pre-baked) grants
   (grant-protocol Rung-0: no console seat -> interactive escalation denied, full stop).

## Design (from the approved plan; the thing to review)

New module `crates/gatekeeperd/src/consent.rs`, in-workspace Rust (shared types with the daemon, no
serialization seam; trust-bearing UI is never QML — shell-architecture load-bearing rule).

Pending state (single in-memory `Option<Pending>`; the broker is a serial single-connection loop, so a
second authority-increasing request while one is pending auto-denies — anti-flood):
```
struct Pending { nonce: u128, uid: u32, pid: u32, starttime: u64,
                 verb: BenchVerb /*Grant|Network|Export*/, argv: Vec<String>,
                 resolved: ResolvedEffect, diff: AuthorityDiff, trifecta: bool, created: Instant }
```

Ceremony flow on an authority-increasing bench request from a **non-root** peer (root peer bypasses — boot /
proofs / reissue run as root in-process via `cli()`):
1. **Invariant pre-check FIRST** (grant-protocol D4): recompute `resolved` with the verb's own validators
   (grant: canonicalize + anchor-pin + dot-leading denylist, `bench_plane.rs:909-976`; network: sealed-profile
   `shrek_policy::egress::resolve`; export: token validators). Any failure -> deny, human NEVER asked. Confirm
   `semantic <= data authority` for the composed profile before asking.
2. **Mint a 128-bit nonce** (`/dev/urandom`), store `Pending` keyed by the bound tuple.
3. **Arm + await SecureAttentionKey** (systemd 257 logind `SecureAttentionKey` signal). No subscription / no
   signal within timeout -> deny.
4. **Take a VT** (logind `TakeControl` + `SwitchTo` + `ioctl(VT_ACTIVATE)`). No VT / TakeControl denied /
   EPERM -> deny.
5. **Render the authority DIFF** (current vs requested authority, exact scope/lifetime); agent-supplied
   reason/label as UNTRUSTED text with control + ANSI stripped; **lethal-trifecta warning** when the profile
   composes untrusted-read + net egress. Render failure -> deny.
6. **Read y/n** from the VT (raw tty). Error / EOF / non-y / inner timeout -> deny.
7. **Apply ONLY on exact tuple+nonce match**, re-reading start-time. On match, run the verb's internal fn
   against the ALREADY-RESOLVED inputs (never the raw client string); release the VT; clear `Pending`. Any
   mismatch -> deny.

Supporting changes:
- **Service unit:** `gatekeeperd.service` is `CapabilityBoundingSet=CAP_SYS_ADMIN` only (line 24) — MUST add
  `CAP_SYS_TTY_CONFIG` for `VT_ACTIVATE`. Minimal cap for the trusted path, nothing more.
- **Verb split refinement:** `network <name> none` REVOKES egress (reducing) -> ceremony-free; only
  `network <name> <profile>` is gated.
- **logind D-Bus surface:** gatekeeperd is std-only / dep-free / panic=abort with NO D-Bus client today.
  Recommended first cut: shell to `busctl` (on the sealed image) for the `SecureAttentionKey` signal +
  `TakeControl`/`SwitchTo`; harden to a hand-rolled system-bus client later. Fail-closed guards identical
  either way.
- **Renderer:** a std-only raw-ANSI VT renderer first cut (consistent with gatekeeperd's dep-free posture);
  ratatui is a later UX polish, not required by the console-VT floor.

## Explicitly deferred (non-goals this slice does NOT claim)

- **TPM NV anti-rollback** for persistent bench grants (grant-protocol D5 defers to the first persistent-grant
  impl). Bench grants live in the durable `/home` record without counter binding; an offline rollback could
  resurrect a revoked grant. Out of scope.
- **Graphical (Wayland) in-session prompt** — Phase-10; the console VT is the required floor.
- **agentd (pid,start-time) attestation for AGENT-originated grants** — this slice covers the
  human-at-terminal peer only.
- **Atomic capability-manifest approval** (one ceremony approving grant+network+export together) — a tracked
  FOLLOW-UP so a future Workshop GUI can request a bundle of authority changes in a single OK. Not this slice.
- **Interactive `bench enter` over the socket** (needs a pty).
- **Owner-account provisioning** (see `installer-0.md` — the ceremony currently binds the baked `dev` uid).

## Proof strategy (repo conventions)

- **consent.rs unit tests** (dep-free, deterministic — where the security logic is proven): the full
  fail-closed matrix (no-VT / render-fail / timeout / malformed / restart-mid-ceremony / disconnect -> all
  DENY) via a mock ceremony transport; the tuple+nonce binding (approve benign args, then attempt to apply a
  different path / rw flag / pid / stale start-time / wrong nonce -> each refused, benign effect NOT applied —
  the swap attack); untrusted-text sanitization; trifecta detection. Faked ONLY at the VT/keyboard + SAK-signal
  boundary; the entire decision/binding/apply logic is real.
- **Host oracle** (`scripts/bench-plane-proof.sh`): fail-closed-on-no-VT — a container has no seat, so an
  authority-increasing socket request must return `END 1 ceremony-*` (the reachable path headlessly).
- **Sealed VM dogfood** (`dogfood-persist-probe` + `dogfood-vm.sh` scoring, real console seat + logind):
  `SHREK-DOGFOOD BENCH-CONSENT` lines mirroring the `BENCH-SUP`/`BENCH-GRANT` pattern — real VT take/switch
  (kernel-enforced), scripted "y" APPLIES, scripted "n"/timeout DENIES with no record change. Real: VT surface
  + logind subscription + apply gate. Faked: only the human keystroke (scripted) and, if kernel SAK can't be
  driven in the VM, the SecureAttentionKey signal at the logind D-Bus boundary.

## Questions for the reviewer (the real doubts)

1. **VT ownership under an active graphical session.** logind may already hold the VT for the Wayland
   compositor. Is gatekeeperd's `TakeControl`/`SwitchTo` to its own VT robust while a desktop session is
   active, and does it return cleanly? Is there a failure mode here that is NOT a clean deny?
2. **busctl on the trust path.** Is shelling to `busctl` for the signal + VT methods acceptable as a first
   cut, or does the fork/exec + text-parse seam on the security-critical path justify the hand-rolled D-Bus
   client from day one? Any way `busctl` output could be spoofed/confused into a false "approved"?
3. **Serial broker blocks during the ceremony.** The `incoming()` loop is single-threaded, so a pending
   ceremony blocks all other socket work (including onion merges) until it resolves/denies. Acceptable, or
   fork a dedicated ceremony child that owns the VT and reports y/n over a pipe (keeping the parent
   responsive, still kernel-enforced VT)?
4. **std-ANSI renderer vs ratatui.** Is a hand-rolled raw-ANSI renderer on the owned VT sufficient and safe
   for the authority diff + trifecta warning, or is a real dep (ratatui) warranted for correctness (not just
   polish)?
5. **The tuple/nonce binding — is it airtight?** Is re-reading start-time at apply enough to defeat PID reuse
   given the window, or is there a residual TOCTOU (e.g., the peer exiting between arm and apply)? Is
   binding to the SERVER-resolved target (not the client string) fully sufficient for the swap attack?
6. **Anything more important I am missing** for a single-owner box where the human is the one driving the
   agent — is there a simpler-but-still-correct console consent than the full SAK+VT dance, WITHIN the
   kernel-enforced floor (no weaker non-VT consent)?
