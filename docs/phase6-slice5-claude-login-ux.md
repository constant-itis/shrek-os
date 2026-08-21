# Phase-6 slice-5 — the "Sign in with Claude" login UX (trusted operator path)

> Slice-4 (docs/phase6-slice4-claude-cli-broker.md) shipped a subscription provider that **assumes a
> pre-logged-in `claude` CLI**. Slice-5 closes that gap: make the subscription provider operable
> **without a preexisting manual login**. The operator runs the **official** `claude auth login` through
> a trusted terminal on the broker host; **the CLI continues to own 100% of the credential state**; Shrek
> records **only** provider-availability, never the credential. Login completion is observed as a **state
> transition**, verified by one real `claude -p` round-trip — never `claude auth status` (#1567 lies).

This slice adds **no sealed-box code** and changes **neither the coder, the api-key path, nor the sealed
egress**. It extends the broker-side `crates/claude-broker` (already off the sealed image, like
`model-proxy`) with two operator subcommands and an audit-only availability breadcrumb. Like slice-4,
**there is no VM gate delta — the broker never enters the sealed image, so the oracle is the gate.**

## 1. The hard constraints (stated twice on purpose)

- **The CLI owns all credential state.** "Sign in with Claude" = running the official `claude auth login
  --claudeai` once; the CLI writes and presents its own subscription credential (its `~/.claude`). Shrek
  never sees, parses, stores, or manages a token.
- **No token capture, ever.** No `sk-ant-*` touches Shrek storage or logs. The login subcommand hands the
  terminal to `claude` with **inherited stdio** and **never captures** its output. The breadcrumb's
  `reason` is a **fixed enum**, never raw CLI text — so it cannot become an accidental credential surface.
- **Login completion is a state transition, not a secret.** It is *observed* (a round-trip now succeeds),
  never *extracted*.

## 2. Why this is NOT the grant-protocol trusted path

`grant-protocol.md` (D1: SecureAttentionKey → gatekeeperd-owned VT) exists to stop a **sandboxed agent**
from spoofing a grant prompt to escalate its **own** authority — the trusted surface is one a sandboxed
requester has no code path to reach. **None of that applies here:**

- The `claude` CLI and its login state live on the **broker host** (reached by the sealed box only as the
  egress name `shrek-claude-cli:8300`). The sealed box has no `claude` binary and no `~/.claude`; it
  cannot run a login and is not in this loop.
- The login is a **human operator admin action** at a real broker-host console, establishing a credential
  the box never touches. There is **no sandboxed adversary in the loop** to spoof a prompt, so importing
  the SAK/VT anchor would be ceremony without a matching threat.

So the "trusted operator path" here is exactly: **the official CLI login runs in the operator's own real
terminal on the broker host, the third-party CLI owns the entire interaction, and Shrek only observes
completion.** (Owner decision, 2026-08-20: the *lean, observe+launch* shape, on a desktop-class broker
host — the environment where the OAuth browser callback completes, matching the slice-4 live seam.
Defending the broker host against its *own* compromised processes phishing the login prompt is real but
is broker-host hardening, out of this slice's scope.)

## 3. What this slice adds

Two subcommands on `claude-broker` (the no-arg / `serve` behavior is slice-4, verbatim, behind a dispatcher):

- **`claude-broker login`** — the trusted operator ceremony:
  1. **Refuse fast if not on a real terminal** (`stdin` *and* `stdout` must be a TTY). A headless run would
     hang on the browser callback (#595); instead it fails **closed** immediately with `reason=non-tty`.
  2. Hand the terminal to the official CLI: `claude auth login --claudeai`, a **fixed argv vector**
     (no caller input) with **inherited stdio** (never captured). The CLI owns the whole flow.
  3. On CLI exit ≠ 0 → `reason=login-failed`, non-zero rc, and **no round-trip is attempted**.
  4. On CLI exit 0 → **fold in the health check** (§4).
- **`claude-broker health`** — run just the round-trip probe and update the breadcrumb (a standalone
  "is the login still valid?" check; also the tail of `login`).

## 4. Health folds into login-completion (owner decision)

Login "worked" is confirmed by **one real `claude -p` round-trip** with a fixed tiny prompt, classified
through slice-4's existing invocation path and `looks_like_auth_failure` classifier — **never**
`claude auth status`. This falls out trivially from slice-4's machinery, so it is folded in here (not a
separate slice-5b):

- round-trip OK → `reason=verified`, `available=true`.
- round-trip fails, auth-classified → `reason=auth-failed`, `available=false`.
- round-trip fails otherwise → `reason=probe-failed`, `available=false`.

The probe returns only a **fixed `Reason`** derived from the classifier; the raw error text (which could
in principle echo a secret) is **dropped**, never returned or stored.

## 5. The availability breadcrumb — audit-only, never authority

A small broker-side JSON at `$SHREK_CLAUDE_STATE_DIR/availability.json` (default
`$HOME/.local/state/shrek-claude-cli/`):

```json
{ "provider": "claude-cli", "available": true, "reason": "verified", "last_verified": 1755720000 }
```

- **`reason` ∈ {`verified`, `auth-failed`, `login-failed`, `non-tty`, `probe-failed`}** — a fixed enum,
  never free text, never CLI output.
- **It informs; it never gates.** Every real coder request still round-trips live and fails closed on the
  true error (#1567: cached state lies — so a cached "available" bit is *never* trusted as authority).
  The sealed box never reads this file; it only ever discovers availability by making a real request.
- **Hardening (owner-requested):** written **atomically and owner-only** — a fresh `0600` temp file →
  write → `fsync` → atomic `rename` over the target → `fsync` the directory (`0700`). A crash can never
  leave believable partial state.

## 6. Authority — nothing widens

No new capability, no new egress, no sealed-box change. `login`/`health` are broker-host operator
commands; the box↔broker↔CLI data path and the T2 wall are exactly slice-4's. The box still holds no
secret, has no `claude`, and reaches only `shrek-claude-cli:8300`.

## 7. Proof

- **Deterministic oracle — `scripts/p6-claude-cli-login-proof.sh`** (unprivileged `debian:trixie`,
  docker default bridge, never `--network host` #2651; a fake `claude` on PATH that logs every
  invocation, uses no network/credential, and even prints a token-shaped line during login to prove
  non-capture). **20/0:**
  - **L1** successful login → `available=true`/`verified`, `0600`, no `.tmp` leftover, **no `sk-ant`** in
    the breadcrumb.
  - **L2** completion verified by a **real `claude -p` round-trip**; `auth status` **never** consulted.
  - **L3** the breadcrumb carries **no CLI output** (no `result`/`pong`/`content`).
  - **L4** failed `auth login` → `login-failed`, non-zero rc, **no round-trip**.
  - **L5** non-TTY → fail-closed fast (`non-tty`), `claude` **never exec'd**, clear operator message.
  - **L6** health classifies ok→`verified`, auth→`auth-failed`, other→`probe-failed`; never consults
    `auth status`.
- **Unit tests (`-p claude-broker --bins`, 12 total; 4 new):** the `reason` fixed-string mapping; the
  breadcrumb serializes exactly the four audit fields with no credential and no `sk-ant`; the atomic
  write is `0600` with no `.tmp` leftover; overwrite replaces cleanly and stays owner-only. (Plus the 8
  slice-4 tests, unchanged.)
- **No VM gate delta** — `claude-broker` is broker-side and never enters the sealed image (slice-4 §5).

## 8. Deferred / residuals (tracked, not built)

- **Headless broker host.** This slice targets a desktop-class console (browser callback completes). A
  fully headless broker host would need `auth login`'s paste-back OAuth-code path (to be verified live) or
  reintroduces the setup-token capture surface — deliberately out of scope.
- **Broker-host phishing defense.** A SAK/VT-anchored login ceremony that defends the *operator's* login
  prompt against a compromised broker-host process is real broker-host hardening, not this slice.
- **Proactive pre-task health.** `health` is operator-invoked; automatically probing before each task (and
  surfacing availability into a UI) is a follow.
- **Stateful `--resume` session** (slice-4 §6) is unchanged and still deferred.
