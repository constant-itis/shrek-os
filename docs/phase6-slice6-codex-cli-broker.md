# Phase-6 slice-6 — the SECOND subscription-model provider (Codex via the logged-in CLI)

> "Sign in with Codex" for Shrek means: **log into the official `codex` CLI once, then Shrek invokes the
> logged-in CLI** — exactly the shape slice-4 established for Claude. This slice proves the
> **broker/provider pattern GENERALIZES beyond Claude**: the coder speaks the SAME plaintext messages-API
> wire; a new broker-side seam ADAPTS that request into a `codex exec` invocation and wraps the reply
> back. Authority does not change by one bit: `sealed egress-profile ∩ grants`, bounded by the T2 wall,
> over NAME-only egress. The box holds no secret, speaks no TLS, and never has a `codex` binary — it
> reaches ONLY the broker.

Builds on slice-4 (docs/phase6-slice4-claude-cli-broker.md) and slice-5 (the login/health/breadcrumb UX,
docs/phase6-slice5-claude-login-ux.md). It changes NEITHER the coder NOR the api-key/claude paths:
`crates/coder`, `crates/model-proxy`, `crates/claude-broker` are untouched. This is a THIRD, parallel
provider selected by a distinct sealed egress name.

## 1. What this slice adds — and what it reuses verbatim

Adds:
- **`crates/codex-broker`** — a broker-side sibling of `claude-broker`. Accepts the coder's Anthropic
  messages-API request over plaintext, adapts it into `codex exec` (the logged-in official CLI) **under an
  unprivileged `bubblewrap` confinement**, and wraps the CLI's final message back into the messages-API
  shape the coder already parses. Runs OUTSIDE the sandbox; excluded from the sealed image (workspace
  `default-members`), exactly like `model-proxy` / `claude-broker`.
- **A sealed `model-codex-cli` egress profile** (`crates/shrek-policy`): exactly ONE destination — the
  broker `shrek-codex-cli:tcp:8301`. A THIRD DISTINCT name (not `model-anthropic`, not `model-claude-cli`)
  — selecting Codex is an explicit, separately-sealed choice (§2).

Reuses verbatim — **zero source change**:
- `crates/coder` — the Provider::Anthropic wire, tool loop, grants, step cap. The box reaches the new
  broker purely via the existing `--model-url` override (`http://shrek-codex-cli:8301/v1/messages`); **no
  provider variant is added** — the pattern generalizes with zero coder change (the primary goal).
- `crates/model-proxy`, `crates/claude-broker` — untouched.
- The `shrek run` front door, the T2/gVisor wall, `--ingest-harness` banding, NAME-only egress.

## 2. Why a third distinct egress name — the no-silent-backend-swap invariant, generalized

`model-anthropic` → the api-key TLS proxy. `model-claude-cli` → the Claude subscription CLI broker.
`model-codex-cli` → the Codex subscription CLI broker. THREE different backends, THREE different trust
stories, THREE separately-sealed names. Reusing one name for two backends is exactly the silent swap the
slice-3 seam forbids. `all_three_provider_paths_are_mutually_distinct_profiles` makes it testable: no
profile's single destination overlaps any other's. Slice-6's real contribution is proving this invariant
GENERALIZES across an open-ended set of subscription providers.

## 3. The one thing that differs from Claude — and how it is contained

`claude -p` is a plain completion. **`codex exec` is an AGENTIC EXECUTOR**: it has its own model-driven
shell/tool surface and its own sandbox. If the broker simply shelled it, a second agent would execute
shell **on the broker host, outside the T2 wall**, and — worse — the model could `cat` the credential the
CLI needs and exfiltrate it through the reply. Two layered guards keep `codex` a pure per-turn text oracle:

**(a) Host confinement (authoritative) — an unprivileged `bubblewrap` STERILE VIEW.** The spawned `codex`
sees only:
- the node/codex runtime tree (ro) — the one `$HOME` subpath, bound ON TOP of a fresh `--tmpfs /home`;
- a ro-bound `auth.json` inside a confined `CODEX_HOME` (the CLI reads its OWN credential; **the broker
  never reads a token byte** — it wires a mount, it does not parse a secret);
- ONE rw scratch file — the `-o` final-message target, the SOLE writable host path.

No project, no `$HOME`, no vault, no other writable host path. `--clearenv` + `--new-session`; every
namespace unshared except NET (hosted inference needs it). Ordering gotcha burned in: the nvm runtime
lives UNDER `/home`, so `--tmpfs /home` MUST precede the runtime ro-bind or it shadows it.

**(b) Reader-disable (the credential guard).** Even inside the sterile view the credential is present (it
must be, to authenticate). So the model's FILE-READER tools are removed: `--disable shell_tool` (removes
the `exec_command` + `write_stdin` shell tools) `--disable unified_exec` `--disable view_image`. After
this, NO tool codex offers the model can read a file's contents, so the ro-bound `auth.json` cannot be
read and exfiltrated through the reply. `-s read-only` + `--ephemeral` + `--ignore-user-config` (drops
`~/.codex/config.toml`, incl. its MCP servers + project trust) + `--ignore-rules` are defense-in-depth.

**A load-bearing finding (codex 0.148.0).** codex's `[tools]` allowlist (`tools.default_tools_enabled`,
`tools.enabled_tools`, `tools.disabled_tools`) is **NOT honored via `-c`** — silently ignored, some forms
error. The SUPPORTED, effective mechanism is the **feature flags** above. They do not produce a *literally
empty* tools array: codex still offers `update_plan`, `request_user_input`, `apply_patch`, `tool_search`,
`web_search`. NONE of these has a filesystem-read primitive — `apply_patch` is write-only and further
blocked by `-s read-only`; `tool_search` searches tool metadata; `web_search` is web egress;
`update_plan` / `request_user_input` touch no filesystem. So the credential-read threat is fully closed
even though the array is not empty, and the gate is defined precisely as: **no reader tool present, and
only known non-reading tools present** (any new/unknown tool fails the build).

## 4. argv safety, adapt/wrap, fail-closed

- **argv safety.** Both `bwrap` and `codex` are invoked with a `std::process` argv VECTOR, never a shell
  string. The caller's model id is mapped through a fixed broker-side ALLOWLIST (`map_model`) to a
  `&'static` Codex id (default `gpt-5.5`); a raw caller string never becomes a command argument. Prompt
  and system text are single data arguments (system is prepended to the prompt — `codex exec` has no
  `--system-prompt`).
- **Adapt + wrap.** The coder sends the growing messages transcript each step; the broker flattens it to a
  role-labelled prompt for the single-shot `codex exec`, reads the final message from `-o`, and wraps it
  as `content:[{type:text,text}]`.
- **Fail-closed.** A non-zero exit, or an empty `-o` file, ⇒ a 502 to the box, never a fabricated success.
  A likely-auth failure is surfaced with a distinct `CODEX-BROKER-UPSTREAM-AUTH-FAIL` marker — grounded in
  the REAL round-trip error (the only authority on token health), never in `codex login status` /
  `codex doctor` (which read cached state and lie — #1567).
- **Dep policy.** Exactly ONE build dependency, `tinyjson` (already vendored) — same as the coder /
  claude-broker. `bwrap` and `codex` are broker-side HOST binaries (like the host `claude`), never in the
  sealed rootfs.

## 5. Login / health / availability — generalized from slice-5

`codex-broker login|health` mirror the slice-5 machinery for Codex:
- `login` refuses fast if not a TTY (never hangs on a browser callback — #595), hands the terminal to the
  official `codex login` (ChatGPT subscription OAuth; runs UNconfined so it can write the real `~/.codex`
  and complete the callback), then folds in one real `codex exec` round-trip and records the result. The
  CLI owns ALL credential state; Shrek captures nothing.
- `health` runs just the round-trip probe.
- The audit-only availability breadcrumb (`$SHREK_CODEX_STATE_DIR/availability.json`,
  `{provider:codex-cli, available, reason, last_verified}`) has a FIXED reason enum
  (`verified|auth-failed|login-failed|non-tty|probe-failed`), written atomically 0600. It INFORMS; it
  never gates — every real request still round-trips and fails closed (#1567: cached state lies).

## 6. Proof

- **Deterministic oracle — `scripts/p6-codex-cli-broker-proof.sh`** (host-side, real `codex` + real
  `bwrap` + a LOCAL fake Responses endpoint + a STUB credential; no quota, the real `~/.codex` is never
  read or bound). The box sends a messages request to the broker → the broker runs the REAL `codex exec`
  under the confinement, pointed (oracle-only) at the fake endpoint → the request codex sends is CAPTURED.
  Asserts: **C1** no file-reader tool (`exec_command`/`write_stdin`/`view_image`) in the request; **C2**
  every offered tool is in the known non-reading allowlist (an unknown tool fails the gate); **C3** the
  stub credential canary appears in NEITHER request, reply, NOR breadcrumb, and the broker used the stub
  home not the real `~/.codex`; **C4** the sterile view leaves a host vault/project secret ENOENT and
  `/home` empty; **C5** the broker adapted the request into `codex exec` and wrapped the reply back
  (`content[].text`). Result: **7 PASS / 0**. The box egress wall is unchanged from slice-4 and proven by
  `p6-claude-cli-broker-proof.sh` + the shrek-policy egress tests (`model-codex-cli` is the same one-dst
  shape) — this oracle proves what is NEW: the broker's confinement + reader-disable of the agentic CLI.
- **Opt-in LIVE smoke (non-gating):** `SHREK_CODEX_LIVE=1` runs the REAL logged-in `codex` behind the
  broker. Spends subscription quota; needs the host's login. NOT part of the deterministic gate.
- **Unit tests (`-p codex-broker --bins`, 15):** `map_model` allowlist (unknown/injection → default, raw
  never returned), transcript flatten + system lift, the confined argv (reader-disable present, sterile
  view hides `/home`/project/vault, exactly ONE writable host bind = the `-o` scratch, auth ro-bound,
  `--clearenv`/`--new-session`, namespaces, `bwrap … -- codex exec` with the prompt as a single arg,
  oracle passthroughs forwarded only when present, extra-args precede-and-cannot-override the disables),
  reply wrapping + escaping, auth classifier (Codex phrasings), breadcrumb (fixed reasons, audit-only
  fields, atomic 0600). Plus `-p shrek-policy --lib`: `model_codex_cli_reaches_only_the_broker` and
  `all_three_provider_paths_are_mutually_distinct_profiles`.

There is no VM gate delta: like `model-proxy` / `claude-broker`, `codex-broker` is broker-side and never
enters the sealed image — the oracle is the gate.

## 7. Deferred / residuals (tracked, not built)

- **Literally-empty tools array.** Not achievable in codex 0.148.0 (the `[tools]` allowlist is not honored
  via `-c`). If a future codex honors it (or exposes a `--no-tools` mode), tighten the gate from
  "no-reader + known-subset" to "empty". Tracked; the current gate already closes the credential-read
  threat.
- **Token-refresh under a read-only `auth.json`.** The credential file is ro-bound; a short inference call
  uses the existing access token, but if a refresh is ever needed it would fail. A refresh path (or a
  pre-task health probe) is a hardening follow.
- **Stateful `--resume` session.** v1 flattens the transcript per call (as slice-4). A `--resume` session
  preserving structured turns is a hardening follow — best done ONCE across both CLI brokers now that the
  pattern is proven for two providers.
- **A shared `broker-core` lib.** `claude-broker` and `codex-broker` now share the HTTP reader, messages
  parse/wrap, and breadcrumb machinery. Factoring a common lib (without merging the distinct egress
  identities) is a clean later refactor once the duplication is proven stable.

## 8. Naming (`codex`, `openai`, `gpt-*`) and the AI-ref scan

The provider id `codex`, the model ids (`gpt-5.5`, …), the crate `codex-broker`, and the `codex` CLI it
shells are legitimate THIRD-PARTY PRODUCT identifiers for a real product feature — distinct from the
repo's "don't fingerprint the assistant that built this" convention. They appear here and in
`crates/codex-broker` by design; the AI-ref scan tripping on them is expected, not a leak.
