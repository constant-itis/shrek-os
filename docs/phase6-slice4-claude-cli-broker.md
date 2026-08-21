# Phase-6 slice-4 — the subscription-model provider (Claude via the logged-in CLI)

> "Sign in with Claude" for Shrek means: **log into the official `claude` CLI once, then Shrek invokes
> the logged-in CLI.** Shrek handles NO subscription OAuth credential — the CLI owns its own login. The
> coder speaks the SAME plaintext messages-API wire it already speaks to the api-key proxy; a new
> broker-side seam TRANSLATES that request into a `claude -p` invocation and wraps the reply back. The
> authority model does not change by one bit: authority stays `sealed egress-profile ∩ grants`, bounded
> by the T2 wall, over NAME-only egress. The box holds no secret, speaks no TLS, and never has a
> `claude` binary — it reaches ONLY the broker.

This slice builds on the **frozen** slice-2 coder contract (docs/phase6-slice2-coder-agent.md §8) and
the slice-3 provider abstraction (docs/phase6-slice3-provider-abstraction.md). It changes NEITHER the
coder NOR the api-key path: `crates/model-proxy` is untouched and remains the credential path for API-key
users. This is a **second, parallel broker** for the subscription case, selected by a distinct sealed
egress name.

## 1. What this slice adds — and what it reuses verbatim

Adds:
- **`crates/claude-broker`** — a broker-side process that accepts the coder's Anthropic messages-API
  request over plaintext, translates it into `claude -p --output-format json` (the logged-in official
  CLI), and wraps the CLI's reply back into the messages-API shape the coder already parses. Runs OUTSIDE
  the sandbox; excluded from the sealed image (workspace `default-members`), exactly like `model-proxy`.
- **A sealed `model-claude-cli` egress profile** (`crates/shrek-policy`): exactly ONE destination — the
  broker `shrek-claude-cli:tcp:8300`. Deliberately a DISTINCT name from `model-anthropic`, never a reuse
  (see §2).

Reuses verbatim — **zero source change**:
- `crates/coder` — the Provider::Anthropic wire (messages-API, `system`-lift, `max_tokens`), the tool
  loop, grants, step cap. The box reaches the new broker purely via the existing `--model-url` override
  (`http://shrek-claude-cli:8300/v1/messages`); no provider variant is added.
- `crates/model-proxy` — untouched; the api-key path stays byte-for-byte as slice-3 shipped it.
- The `shrek run` front door, the T2/gVisor wall, `--ingest-harness` banding, `--egress-profile`
  NAME-only egress, the sealed-in-rootfs coder.

## 2. Why a distinct egress name — no silent backend swap

`model-anthropic` → the api-key TLS proxy. `model-claude-cli` → the subscription CLI broker. These are
DIFFERENT backends with DIFFERENT trust stories (one injects an API key + does TLS; the other shells a
logged-in CLI and handles no credential at all). Reusing one egress name for both — resolving it to
whichever broker happens to be listening — is exactly the silent backend swap the slice-3 seam forbids
(“an unknown name is a hard error, never a silent backend swap that could mismatch the sealed egress the
session was built with”). So the subscription path is an EXPLICIT, separately-sealed choice, and the
api-key path is provably unchanged. `subscription_and_apikey_paths_are_distinct_profiles` makes this
testable: neither profile's single destination overlaps the other's.

## 3. The broker — translation, not credential handling

```
box (T2)  --plaintext messages API-->  claude-broker  --claude -p (argv, no shell)-->  claude (owns login)
(no key,   (model-claude-cli =          (broker-side,   translate + wrap;              (the only party that
 no TLS,    ONE dst)                     off the image)  handles the subscription)       authenticates)
 no `claude`)
```

- **No credential in Shrek.** The broker reads no token and never calls `claude auth status` (which lies
  — #1567). "Sign in with Claude" is the operator running the CLI's own login once on the broker host;
  from then on the CLI presents its own credential. The lethal-trifecta break is identical to slice-3
  (box: untrusted-read + egress, NEVER the secret) — here it is even stronger: there is no secret *in
  Shrek at all*, and the box has no `claude` binary to invoke.
- **argv safety (fork-#2 decision).** The CLI is invoked with a `std::process` argv VECTOR, never a shell
  string. The caller's model id is mapped through a fixed broker-side ALLOWLIST to a `&'static` value
  (`map_model`), so a raw caller string never becomes a command argument; unknown/absent ids fall to the
  broker default. The prompt and system text are single data arguments (`-p <prompt>`,
  `--system-prompt <sys>`), never flags. `map_model_allowlists_and_never_passes_raw` proves an injection-y
  id (`--dangerously-skip-permissions`, `evil; rm -rf /`) is discarded, not forwarded.
- **Transcript flattening (v1).** The coder sends the growing `[system, user, (assistant, user)*]`
  transcript each step. `claude -p` is a single-shot call, so the broker lifts `system` to
  `--system-prompt` and renders the `messages` array to a role-labelled prompt (`User:`/`Assistant:`
  turns). The step is derivable from the transcript itself (the count of assistant turns) — no server
  state. A stateful `--resume` session preserving structured turns is a tracked follow (§6).
- **Fail-closed.** A non-zero CLI exit, an `is_error` result, or unparseable stdout ⇒ a 502 to the box,
  never a fabricated success. A likely-auth failure is surfaced with a distinct
  `CLAUDE-BROKER-UPSTREAM-AUTH-FAIL` marker — grounded in the REAL round-trip error (the only authority on
  token health), never in `claude auth status`.
- **Dep policy.** Exactly ONE dependency, `tinyjson` (pure-Rust, zero-transitive, already vendored) — the
  same single dep the coder carries. No TLS stack (that lives in `model-proxy`); the broker's outward hop
  is the `claude` process, not a socket it opens.

## 4. Authority — nothing widens

Selecting `model-claude-cli` changes the box↔endpoint pairing and adds NO capability: egress is still one
sealed NAME resolved only in the sealed policy table; grants and the T2 wall are unchanged; the box cannot
name Anthropic or the broker's child process, cannot reach anything but `shrek-claude-cli:8300`, holds no
secret, and has no `claude`. The confused-deputy residual (agents.md §8) is the same as slice-2/3 and
bounded by the same wall + project grant.

## 5. Proof

- **Deterministic oracle — `scripts/p6-claude-cli-broker-proof.sh`** (privileged debian:trixie, spike):
  the coder (`--provider anthropic --model-url http://shrek-claude-cli:8300/v1/messages`) solves the
  fixture task through `shrek run --egress model-claude-cli` → the broker → a canned **fake `claude`**
  (reads `-p`, derives the step from the transcript, emits the tool-call in `claude --output-format json`
  shape; no network, no credential). Asserts **C1** banding + egress name + coder wire + model-url points
  at the broker; **C2** the messages→CLI translation drove the loop AND the broker forwarded/shelled the
  CLI (`CLAUDE-BROKER-FWD` / `CLAUDE-BROKER-CLI-OK`); **C3** real tcc build/run (marker + exit 42,
  `CODER-DONE ok=true`); **C4** write-through both grants; **C5** the box held NO credential AND NO
  `claude` binary, and the CLI ran BROKER-SIDE (fake-claude log has invocations); **C6** the wall held —
  a non-broker port on the broker host and a non-model dst (1.1.1.1:53) DROPPED, vault ENOENT, host
  sentinel absent. Result: **ALL PASS (25/0)**.
- **Opt-in LIVE smoke (non-gating):** `SHREK_CLAUDE_LIVE=1` runs the REAL logged-in `claude` behind the
  broker HOST-SIDE and confirms a genuine reply flows back through the same messages-API seam. Spends
  subscription quota; needs the host's login. NOT part of the deterministic gate; never in the image.
- **Unit tests (`-p claude-broker --bins`, 8):** `map_model` allowlist (unknown/injection → default, raw
  string never returned), transcript flatten + system lift, empty-prompt fail-closed, `.result`
  extraction (success + `is_error` + missing-field fail-closed), reply wrapping (valid Anthropic shape,
  correct escaping), auth-failure classifier, HTTP head finder. Plus `-p shrek-policy --lib`:
  `model_claude_cli_reaches_only_the_broker` and `subscription_and_apikey_paths_are_distinct_profiles`.

There is no VM gate delta: like `model-proxy`, `claude-broker` is broker-side and never enters the sealed
image, so the endpoint-free sealed-VM path is unaffected — the oracle is the gate.

## 6. Deferred / residuals (tracked, not built)

- **Stateful `--resume` session.** ~~v1 flattens the transcript to a text prompt per call.~~ **DELIVERED
  in slice-7** ([`phase6-slice7-cross-provider-session.md`](phase6-slice7-cross-provider-session.md)): an
  opaque `X-Shrek-Session` handle maps to a broker-minted `claude --session-id`/`--resume` native session
  (fewer tokens, closer to the messages API's own statefulness), forwarding only the new tail turn, with
  the flatten path kept as the correctness fallback.
- **The LIVE smoke** is wired but opt-in; a real end-to-end against the subscription is a separate,
  non-gating, quota-spending run.
- **A self-contained / musl-static broker** — v1 is glibc-dynamic + one vendored dep, like the coder; a
  smaller footprint is a hardening follow. (Note the broker's real coupling is the host `claude` binary,
  which is why it is broker-side and out of the image regardless.)
- **Token-health probe.** The broker surfaces a real round-trip auth failure fail-closed; a proactive
  "is the login still valid" probe that round-trips a tiny call before a task is a follow (it belongs with
  the sign-in capture UX).
- **Sign-in capture UX.** Running the CLI's login on the host via the parked grant-protocol trusted-path
  is out of this slice by design — this slice assumes the CLI is already logged in.

## 7. Naming (`claude`, `anthropic`) and the AI-ref scan

The provider id `anthropic`, the model ids (`claude-sonnet-5`, …), the crate `claude-broker`, and the
`claude` CLI it shells are legitimate THIRD-PARTY PRODUCT identifiers for a real product feature —
distinct from the repo's "don't fingerprint the assistant that built this" convention. They appear here
and in `crates/claude-broker` by design; the AI-ref scan tripping on them is expected, not a leak.
