# Bench authorization — the console consent ceremony (BUILT + oracle-proven; VM dogfood pending)

> **Status: built, unit-tested, and host-oracle-proven.** `crates/gatekeeperd/src/consent.rs` implements this
> design; the security core (sanitizer, tuple binding / apply gate, trifecta, cooldown, confirmation code) is
> proven by 15 dep-free unit tests over a mock console, and the fail-closed-on-no-seat half is proven by the
> bench-plane host oracle (`scripts/bench-plane-proof.sh` step 3, PASS=73/0). **Still pending: the sealed-VM
> `BENCH-CONSENT` dogfood stage** (real VT + scripted "y"/"n") and its three image-side verify items (systemd
> ≥257 / SAK enablement, compositor-independent chord detection, VT scanout) — the RealConsole VT/SAK
> transport fails closed until those hold. This is step 3 of the bench authorization slice (steps 1+2 — socket
> transport — are shipped and green), a **build slice of `grant-protocol.md`** applied to the three
> authority-increasing bench verbs. The design was reviewed against the code (Fable, 2026-08-30): the review
> pivoted the VT mechanism, closed an apply-time swap hole, and surfaced a transport gap and several
> under-specified requirements — all folded in below; "Review outcomes (resolved)" is at the end.
>
> **Build note (a bug the oracle caught):** minting the nonce with `std::fs::read("/dev/urandom")` reads to
> EOF, but urandom is infinite — the `panic=abort` daemon hung and died the instant a request reached the
> ceremony. Fixed to read exactly 16 bytes (`read_exact`). The lesson: I/O-boundary bugs the unit tests can't
> see are exactly what the host oracle is for.

## Why this exists

Steps 1+2 moved every bench verb onto gatekeeperd's authenticated socket (`/run/shrek-gk.sock`, SO_PEERCRED)
and refused the three **authority-increasing** verbs (`grant`, `network`-to-a-profile, `export`) fail-closed
with `needs-consent-ceremony`. This slice builds that ceremony: a human OK on a surface a sandboxed agent
cannot spoof, so that nothing in the desktop session (a hijacked or overreaching agent included) can expand
its own authority silently. The ceremony gates **authority configuration**, not operation: a `run` inside an
already-granted workshop never re-prompts. The threat model is a single-owner box where the human drives an AI
agent inside the desktop session — the danger is that agent escalating its OWN authority, so the property we
are buying is *the session cannot spoof or suppress the consent gesture*.

## Owner hard invariants (non-negotiable)

1. **Fail closed on ANY ceremony unavailability.** No reserved VT, VT-switch that does not complete within its
   deadline, render failure, answer timeout, malformed request, daemon restart mid-ceremony, or client
   disconnect must ALL deny. No fallback to approval, no fallback to sudo, anywhere. Every syscall wait on the
   trust path is bounded (there is no unbounded blocking call left after the VT-mechanism change below).
2. **Approval bound to the exact tuple** `(peer uid, pid, start-time, verb, full decoded argv,
   server-resolved target as a pinned object, gatekeeperd-issued nonce)`. An approved benign request cannot be
   swapped for a different operation. Re-read start-time at apply (PID-reuse / TOCTOU defense) AND re-verify
   the pinned target object identity at apply (symlink-swap defense — see invariant 5).
3. **Ceremony gates authority CONFIGURATION only** (grant/network/export), never operations under an existing
   grant. `network <name> none` REVOKES egress (reducing authority) → ceremony-free.
4. **Headless / remote authority expansion fails closed** and requires pre-authorized (pre-baked) grants
   (grant-protocol Rung-0: no console seat → interactive escalation denied, full stop).
5. **The approved target is an OBJECT, not a string.** The server-resolved path is pinned as a directory fd at
   pre-check time and its `(st_dev, st_ino)` re-verified at apply. Binding to the resolved *string* alone is
   insufficient: the `dev` agent owns the whole home anchor and can replace an approved directory with a
   symlink to a different (still validator-passing) in-anchor directory between approval and apply.
6. **Every ceremony outcome is audited.** Arm / approve / deny(reason) / timeout is recorded structurally with
   the bound tuple, verb, and resolved effect through the broker's existing audit machinery. The nonce is
   NEVER written to the record, the wire, responses, or logs.

## Design (reviewed; the thing to build)

New module `crates/gatekeeperd/src/consent.rs`, in-workspace Rust (shared types with the daemon, no
serialization seam; trust-bearing UI is never QML — shell-architecture load-bearing rule).

### Transport change (prerequisite — today's socket cannot carry the binding)

`handle_conn` reads `SO_PEERCRED` (`main.rs:355`) but calls `dispatch_socket(&argv)` (`main.rs:401`) with argv
only; `dispatch_socket(argv: &[String])` (`bench_plane.rs:1373`) has no uid/pid parameter and its
`needs-consent-ceremony` refusal (`bench_plane.rs:~1460`) fires identically for every peer including root.
The `Pending{uid,pid,starttime,…}` binding therefore cannot be built on today's transport. Required changes:

- `dispatch_socket` grows the peer `Ucred` and the `UnixStream` (the stream is needed for connection-alive /
  disconnect detection during the ceremony).
- The three gated verbs route through the ceremony **for ALL socket peers, root included.** Root's only
  ceremony bypass is the in-process `cli()` boot path (`bench_plane.rs:1237`) — boot / proofs / reissue run as
  root in-process, never over the socket. One fewer privileged branch in the socket code.
- The gated verbs are restricted to the **bench user's uid** (`dev`) on the socket path. Bench is `dev`'s
  user-authority plane (anchor `/home/dev`, `bench_plane.rs:75-77`); the `shrek` peer in the connect allowlist
  (`main.rs:~499-510`) has no business in a bench-authority ceremony.

### Pending state

Single in-memory `Option<Pending>`; the broker is a serial single-connection loop, so it is structurally
impossible for two ceremonies to be pending at once (see anti-abuse — the serial loop is NOT anti-flood by
itself):
```
struct Pending { nonce: u128, uid: u32, pid: u32, starttime: u64,
                 verb: BenchVerb /*Grant|Network|Export*/, argv: Vec<String>,
                 resolved: ResolvedEffect, target_fd: Option<OwnedFd> /*O_PATH dir pin*/,
                 target_ino: (u64,u64) /*(st_dev,st_ino)*/,
                 diff: AuthorityDiff, trifecta: bool, created: Instant }
```

### Ceremony flow (non-root peer; root over the socket is refused, drives `cli()` in-process)

1. **Invariant pre-check FIRST** (grant-protocol D4): recompute `resolved` with the verb's own validators
   (grant: canonicalize + anchor-pin + dot-leading denylist, `bench_plane.rs:910-976`; network: sealed-profile
   `shrek_policy::egress::resolve`; export: token validators). Any failure → deny, human NEVER asked. Confirm
   `semantic <= data authority` for the composed profile before asking. For a path target, **pin the resolved
   directory as an `O_PATH` fd** (preferably via `openat2(RESOLVE_BENEATH|RESOLVE_NO_SYMLINKS)`) and record
   `(st_dev, st_ino)` into `Pending`.
2. **Mint a 128-bit nonce** (`/dev/urandom`), store `Pending` keyed by the bound tuple. The nonce never leaves
   the daemon; it exists only for the restart/mismatch matrix and future async work.
3. **Arm + await SecureAttentionKey** (systemd 257 logind `SecureAttentionKey` signal) via `busctl` with a
   **sender-pinned match** `sender='org.freedesktop.login1'` (+ seat check for the physical seat if the signal
   carries it). No signal within the arm timeout → deny; kill the `busctl` child on the deadline (no orphan
   `busctl monitor`). Any parse ambiguity, unexpected output, or non-zero exit → deny.
4. **Own a dedicated reserved VT with kernel ioctls** (NOT logind `TakeControl`/`SwitchTo`). gatekeeperd is
   root with `CAP_SYS_TTY_CONFIG`; it does not need logind's blessing to own a VT, and `TakeControl` would rip
   session-controller status from the Wayland compositor (one controller per session) — a non-clean-deny that
   can wedge the desktop. Sequence (this is what `openvt` does):
   - A **fixed VT above the autovt range** (tty ≥ 8, above logind `NAutoVTs`=6, so no `autovt@` getty spawns on
     top of the consent screen) is reserved in the sealed image and its fd held open from daemon start.
   - `VT_GETSTATE` (save the currently-active VT) → `VT_ACTIVATE(N)` + **bounded** `VT_WAITACTIVE(N)` poll
     against a deadline (a compositor in `VT_PROCESS` mode that never `VT_RELDISP`s must NOT wedge the serial
     broker — on deadline: switch back, deny) → termios raw + `tcflush(TCIFLUSH)`.
   - Any ioctl error / EPERM / deadline → restore the saved VT, deny.
5. **Render the authority DIFF** (current vs requested authority, exact scope/lifetime) on the owned VT with a
   std-only raw-ANSI renderer:
   - **Sanitize at the output boundary** — `pct_decode` already does `from_utf8_lossy` (`bench_plane.rs:186`),
     so arbitrary bytes arrive as text. Allowlist printable ASCII + explicit newlines; escape everything else,
     **including all non-ASCII codepoints**, as visible `\u{…}`. This kills CSI/OSC/C1 escape injection AND
     Unicode visual spoofing (U+202E RLO reordering, homoglyph paths e.g. Cyrillic-с) in one move. The grant
     validators only constrain the *leaf* (`fs_leaf`→`valid_bench_name`, `bench_plane.rs:144-147`); intermediate
     path components can be arbitrary non-dot-leading Unicode, so the rendered diff is where spoofing must die.
   - Sanitize **everything rendered** — paths, argv, profile names — not just the agent's reason/label. For
     `export`, render the full workload argv verbatim (escaped): approving an export mints a durable
     ceremony-free launcher that replays `exp.cmd` forever (`run_export`, `bench_plane.rs:1141-1156`).
   - **Never silently truncate** the server-resolved effect (approving a visible prefix of a hidden suffix is a
     consent failure). Let the VT hard-wrap; total request size is already bounded (argc ≤ 64, `main.rs:381`).
   - **Lethal-trifecta warning** when the composed profile is untrusted-read + net egress — see predicate below.
   - Reset the VT to a known state (SGR reset, clear, `KD_TEXT`) before rendering. Render failure → deny.
6. **Read the answer** from the VT (raw tty): `tcflush(TCIFLUSH)` again immediately before the read (drop
   anything queued before the render existed), then require **`y` + Enter** minimum. For the higher-authority
   verbs (`grant --rw`, `export`) require typing a **short confirmation code displayed on the VT itself** (a
   single stray `y` is too close to whatever the human was mid-typing when the screen flipped). Error / EOF /
   wrong code / non-affirmative / inner timeout → deny.
7. **Apply ONLY on exact tuple+nonce match, re-verifying object identity.** Re-read start-time from
   `/proc/<pid>/stat`; re-`fstat` the pinned `target_fd` and require `(st_dev, st_ino)` unchanged vs
   `Pending.target_ino` (symlink-swap defense); confirm the connection is still alive. On full match, run the
   verb's internal fn against the **ALREADY-RESOLVED, pinned inputs** (never the raw client string; grant
   applies against the pinned fd via `/proc/self/fd`, not a re-`canonicalize` of `path_str`). Restore the saved
   VT, release/`VT_DISALLOCATE` the reserved VT, clear `Pending`. Any mismatch → deny, restore VT.

### Anti-abuse (the serial loop is NOT anti-flood)

"A second request while one is pending auto-denies" is vacuous under a serial blocking loop — there are never
two pending. The real attack is **sequential**: the agent spams authority requests back-to-back, each burning
a full SAK-arm timeout, starving `merge`/`status` and training SAK fatigue (grant-protocol's Anti-abuse
section demands rate-limit/coalesce/auto-deny). Add a **post-deny cooldown per `(uid, verb-family)`** — no new
ceremony armed for N seconds after a deny — with repeated denials **escalating** the cooldown. Document the
bounded worst-case: a pending ceremony stalls all other socket work (including onion merges) for up to the
overall ceremony deadline; on a sealed appliance runtime merges are rare, so this availability cost is
accepted for this slice rather than forking a ceremony child (which would add a pipe protocol — a new
serialization seam on the trust path, exactly what the "no serialization seam" rule forbids).

### Timeouts (load-bearing; pinned so the unit tests can assert them)

SAK-arm 60s · VT-switch deadline 5s · answer 45s · overall ceremony 120s. Every one is a deny on expiry.

### Trifecta predicate (concrete, for bench semantics)

Warn when `(existing grants in rec.grants ∪ requested effect)` contains **≥1 fs grant AND ≥1 net egress
profile** — i.e. approving `network <profile>` on a bench that already has any fs grant, OR approving any
`grant` on a bench that already has egress. Both directions. Compose from `Grant::parse` over `rec.grants`
(`bench_plane.rs:124-131`).

### Supporting changes

- **Service unit** (`image/overlay/usr/lib/systemd/system/gatekeeperd.service`): line 24 is
  `CapabilityBoundingSet=CAP_SYS_ADMIN` — MUST add `CAP_SYS_TTY_CONFIG` (needed for `VT_ACTIVATE` /
  `VT_DISALLOCATE`; the bounding set currently excludes it). Minimal cap for the trusted path, nothing more.
  Leave the no-mount-namespace posture intact — none of the VT ioctls need one.
- **Reserved VT** (tty ≥ 8) provisioned in the sealed image, fd held from daemon start (blocks autovt getty).
- **`none` profile shadowing:** `network <name> none` is the ceremony-free revoke, keyed on the literal string
  (`bench_plane.rs:989`). Assert/lint that the sealed egress profile table can NEVER contain a profile named
  `none` (it would be unreachable/ambiguous).
- **logind D-Bus surface:** gatekeeperd is std-only / dep-free / panic=abort with NO D-Bus client today. First
  cut shells to `busctl` for the `SecureAttentionKey` signal ONLY (the VT is pure kernel ioctls now, so busctl
  has exactly one call site). Sender-pinned match, deny-on-anomaly, child-killed-on-deadline. Hand-rolled
  system-bus client is a tracked fast-follow, not a day-one blocker; fail-closed guards identical either way.
- **Renderer:** std-only raw-ANSI is correct — the security is in the sanitizer, not the layout, and no TUI
  crate does the sanitizing for you. ratatui stays later UX polish, not required by the console-VT floor.

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
- **Hand-rolled D-Bus client** — busctl-with-guards first; native client a fast-follow.

### Honest residual (state it so nobody mistakes the ceremony for more than it is)

A path grant under the `dev`-writable anchor is a **semantic** grant, not an information-flow guarantee: after
the grant lands, the agent can move any of `dev`'s non-dotdir data *into* the approved directory. The
dot-leading denylist protects the credential/config dirs; the trifecta warning covers the exfil composition.
Those two carry the weight the object-pinning does not — the ceremony proves *the human approved this object*,
not *this object can never contain sensitive data later*.

## Proof strategy (repo conventions)

- **consent.rs unit tests** (dep-free, deterministic — where the security logic is proven): the full
  fail-closed matrix (no-VT / VT-switch-deadline / render-fail / answer-timeout / malformed / restart-mid-
  ceremony / disconnect → all DENY) via a mock ceremony transport; the tuple+nonce binding (approve benign
  args, then attempt to apply a different path / rw flag / pid / stale start-time / wrong nonce → each refused,
  benign effect NOT applied — the swap attack) INCLUDING the **symlink-redirect swap** (approve a dir, replace
  it with a symlink to a different validator-passing in-anchor dir, apply must deny on `(st_dev,st_ino)`
  mismatch); the **sanitizer** against a CSI / OSC / C1 / U+202E / homoglyph corpus; trifecta detection; the
  post-deny cooldown + escalation. Faked ONLY at the VT/keyboard + SAK-signal boundary; the entire
  decision/binding/apply logic is real.
- **Host oracle** (`scripts/bench-plane-proof.sh`): fail-closed-on-no-VT — a container has no seat, so an
  authority-increasing socket request must return `END 1 ceremony-*` (the reachable path headlessly).
- **Sealed VM dogfood** (`dogfood-persist-probe` + `dogfood-vm.sh` scoring, real console seat + logind):
  `SHREK-DOGFOOD BENCH-CONSENT` lines mirroring the `BENCH-SUP`/`BENCH-GRANT` pattern — real VT take/switch
  (kernel-enforced), scripted "y"+code APPLIES, scripted "n"/timeout DENIES with no record change. Assert
  additionally: the **previously-active VT is restored** after approve AND after every deny path, **no getty**
  is present on the ceremony VT, and the **compositor is alive** afterward (the VT-restore leg is in the
  fail-closed matrix but only the sealed VM can exercise it — mocked-VT unit tests and the host oracle cannot).
  Real: VT surface + logind subscription + apply gate. Faked: only the human keystroke (scripted) and, if
  kernel SAK can't be driven in the VM, the SecureAttentionKey signal at the logind D-Bus boundary.

### Pre-implementation verify items (confirm on the sealed image before coding)

- systemd is actually ≥ 257 and how `SecureAttentionKey` is enabled in logind config on this image.
- logind's SAK chord detection is **compositor-independent** (seat input devices, not compositor-mediated) —
  this is what makes SAK unsuppressible; if it were compositor-mediated it would only be fail-closed, not
  reliable. The dogfood script asserts real signal delivery.
- kernel has `CONFIG_VT` + fbcon so a text VT actually scans out when the compositor's DRM master is paused;
  the compositor is a proper logind session client so the DRM/input handoff on VT switch happens.

### Verified on the sealed image (2026-08-31 — DOGFOOD base off `f53f7d4`, read-only inspection of root + ESP)

All three gates PASS; the RealConsole VT/SAK ceremony is clear to implement (harness design still pending a
Fable pass).

- **systemd ≥ 257 + SAK — CONFIRMED.** `systemd 257.13-1~deb13u1`; `systemd-logind` carries the
  `SecureAttentionKey` path (`HandleSecureAttentionKey`, `Login.HandleSecureAttentionKey`). **Caveat +
  impl note:** the image ships logind's *compiled default* — `/etc/systemd/logind.conf` has
  `#HandleSecureAttentionKey=secure-attention-key` (commented). Bake an *explicit*
  `HandleSecureAttentionKey=secure-attention-key` drop-in (like the other sealed logind settings) so the
  trust trigger can't silently regress if the upstream default ever changes.
- **Compositor-independent detection — CONFIRMED (static).** SAK is handled by `systemd-logind` (a system
  service reading seat evdev), not the compositor, so detection survives a compositor holding DRM master.
  Runtime signal *delivery* remains the dogfood's own assertion (the faked-boundary note above).
- **kernel `CONFIG_VT` + fbcon — CONFIRMED.** kernel `6.12.107+deb13-amd64` (config read from the matching
  `linux-image` package — the UKI carries no IKCONFIG): `CONFIG_VT=y`, `CONFIG_VT_CONSOLE=y`,
  `CONFIG_FRAMEBUFFER_CONSOLE=y`, `CONFIG_FRAMEBUFFER_CONSOLE_DETECT_PRIMARY=y`, and crucially
  `CONFIG_DRM_FBDEV_EMULATION=y` — the fbdev-emulation path that lets a text VT scan out under KMS when the
  compositor's DRM master is paused.

## Review outcomes (resolved)

The six pre-build doubts, decided:

1. **VT ownership under an active session** → *changed the mechanism.* Drop logind `TakeControl`/`SwitchTo`
   (session-controller theft = non-clean-deny, can wedge the desktop); use raw kernel VT ioctls on a reserved
   VT (tty ≥ 8) with a bounded `VT_WAITACTIVE`. Every failure is now a bounded-deadline deny or ioctl-error
   deny, and the desktop survives. Keep logind only for the SAK signal.
2. **busctl on the trust path** → *acceptable for the SAK signal ONLY,* with a **sender-pinned match**
   (`sender='org.freedesktop.login1'` — the bus daemon fills sender authoritatively; unpinned, any peer incl.
   the `dev` agent can forge a SAK signal and surprise-flip the screen). No spoof-into-false-*approval* path
   exists because "approved" is only ever the `y`+code read from the root-owned VT fd; busctl sits upstream of
   the decision. Hand-rolled D-Bus client = fast-follow.
3. **Serial broker blocks during the ceremony** → *accept it.* The serial loop is the mutex the design relies
   on; a ceremony child adds a pipe seam on the trust path. Bound every wait; accept the documented worst-case
   stall for availability-plane verbs. (Anti-flood handled separately by the post-deny cooldown.)
4. **std-ANSI vs ratatui** → *hand-rolled std-ANSI.* Security is in the sanitizer, not the renderer; ratatui
   adds zero security and stays polish.
5. **Tuple/nonce binding airtight?** → *peer half fine, target half had a hole — now closed.* PID/start-time
   re-read defeats PID reuse; nonce crosses no trust boundary (kept for restart/async). The hole: apply-time
   `canonicalize` of the raw string follows a swapped symlink → bind to a pinned `O_PATH` fd + re-verify
   `(st_dev,st_ino)` at apply.
6. **Anything more important missing?** → the simpler-but-still-correct variant IS answer 1 (SAK + raw VT
   ioctls, subtract logind from VT control). Do NOT simplify past SAK to "switch to tty8 yourself" — that puts
   the switch through the compositor's VT handling, which a hijacked compositor can veto forever. SAK is the
   one gesture the session cannot intercept; it is load-bearing, not theater.
