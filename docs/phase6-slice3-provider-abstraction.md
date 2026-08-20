# Phase-6 slice-3 — the model-provider abstraction (local Qwen + hosted Claude)

> The coder gains a SECOND model backend — a hosted Anthropic model — without changing the authority
> model by one bit. A "provider" is nothing more than `(sealed egress-profile NAME, wire-adapter[,
> broker-side auth-injection])`. Authority stays `sealed egress-profile ∩ grants`, bounded by the T2
> wall, over NAME-only egress. The hosted key is a secret, so it lives in a BROKER-SIDE proxy, never in
> the sandbox — the lethal-trifecta break (`untrusted-read + egress + secret`).

This slice builds on the **frozen** slice-2 coder contract (docs/phase6-slice2-coder-agent.md §8) and
does not alter it: the same bounded four-tool loop, the same T2 wall, the same integrity-from-the-seal.

## 1. What this slice adds — and what it reuses verbatim

Adds:
- **A provider seam in `crates/coder`** (`Provider { Local, Anthropic }`): the ENTIRE variance is the
  request wire and the reply extractor. Selected by `--provider` / `SHREK_PROVIDER` (default `local` =
  unchanged slice-2 behavior). Deliberately two concrete variants, **not** a plugin framework — the seam
  is exactly what these two working implementations force.
- **A sealed `model-anthropic` egress profile** (`crates/shrek-policy`): exactly ONE destination — the
  broker proxy `shrek-model-proxy:tcp:8200`, NOT `api.anthropic.com`.
- **`crates/model-proxy`** — the broker-side authenticated egress proxy (security-model.md §7): holds
  the API key, injects auth, terminates TLS to Anthropic. Runs OUTSIDE the sandbox; the box speaks
  plaintext to it.

Reuses verbatim: the coder's tool loop + grants + step cap; the `shrek run` front door; the T2/gVisor
wall; `--ingest-harness` banding; `--egress-profile` NAME-only egress; the sealed-in-rootfs coder.

## 2. The provider seam (`crates/coder`)

```
build_request(provider, model, messages)   — internal transcript → provider wire
extract_assistant_content(provider, body)  — provider reply → the tool-call text
```

These two functions are the whole seam; the loop, the tools, and the grants never learn which provider
is in play. `--provider` resolves FAIL-CLOSED (an unknown name is a hard error, never a silent backend
swap that could mismatch the sealed egress the session was built with).

| provider  | wire (request → reply)                         | default endpoint (ALWAYS plaintext http://) | sealed egress   |
|-----------|------------------------------------------------|---------------------------------------------|-----------------|
| `local`   | OpenAI `chat/completions` → `choices[0].message.content` | `shrek-model:8100` (the LAN model, direct)  | `model-local`     |
| `anthropic` | Anthropic `messages` → `content[].text`      | `shrek-model-proxy:8200` (the BROKER PROXY) | `model-anthropic` |

The Anthropic adapter lifts the `system` turn to the top-level field, keeps the `messages` array
user-first/alternating, and sets the required `max_tokens`. Crucially it **still speaks plaintext** and
sets **no `x-api-key`** — the proxy injects auth. The coder holds no secret and speaks no TLS.

## 3. The broker-side authenticated egress proxy (`crates/model-proxy`)

The crux is **secret ≤ authority** (security-model.md §7, threat-model §7.5). A hosted key is a secret;
putting it in the T2 box (which has untrusted-read + egress) would complete the lethal trifecta. So:

```
sandbox  --plaintext-->  model-proxy  --TLS + x-api-key-->  api.anthropic.com
(no key,  (model-anthropic  (holds the key,     (the only party
 no TLS)   = ONE dst)        does TLS)            with the secret)
```

- The key is read from a **broker-side file** (`SHREK_ANTHROPIC_KEY_FILE`) — never an env the sandbox
  can inherit, never in the repo. On any auth-read failure the proxy fails CLOSED (502) rather than
  forward unauthenticated. The key value is never logged.
- The proxy is **not a translator**: the coder builds the messages-API wire; the proxy forwards the body
  verbatim and only adds the `x-api-key` + `anthropic-version` headers + TLS.
- TLS is **rustls** (pure-Rust, no OpenSSL) with Mozilla `webpki-roots` (+ an optional extra CA for the
  test upstream). This is the ONE crate carrying a TLS dependency tree; it is scoped here, vendored
  in-tree for an auditable offline build, and **excluded from the sealed image** (workspace
  `default-members`) — the appliance build pulls no TLS stack and the box stays plaintext-only.
- It is **broker-side**, not sealed into the appliance image and not a control plane. A musl-static /
  self-contained single-binary variant is a later hardening (like the coder's).

## 4. Authority — nothing widens

`--provider anthropic` changes the box↔endpoint pairing and the wire, and **adds no capability**: the
egress is still one sealed NAME resolved only in the sealed policy table; the grants and the T2 wall are
unchanged; the box cannot name `api.anthropic.com` (only `shrek-model-proxy`), cannot reach it (the
allow-list pins the proxy dst:port only), and holds no secret. The confused-deputy residual (agents.md
§8) is the same as slice-2 and bounded by the same wall + project grant.

## 5. Proof plan

- **Deterministic oracle — `scripts/p6-anthropic-proxy-proof.sh`** (privileged debian:trixie, spike):
  the coder (`--provider anthropic`) solves the fixture task through `shrek run --egress model-anthropic`
  → the broker proxy → a canned Anthropic-shaped **HTTPS** responder (self-signed leaf the proxy trusts;
  SNI `api.anthropic.com`). Asserts:
  - **B1** authentic harness ⇒ `derived=T-untrust`, `construct-at=T2`, `egress=model-anthropic`; the
    coder announces `provider=anthropic`.
  - **B2** the **messages-API adapter** drove the loop (`CODER-STEP`, `write_file`, `run`).
  - **B3** real build/test: compiled ELF ran, marker+exit == pass, `CODER-DONE ok=true`.
  - **B4** write-through: fixed source + ELF on the host.
  - **B5** the **proxy injected auth** (the canned upstream SAW `x-api-key` on every call) AND the **box
    held NO secret** (in-sandbox `env` has no `SHREK_ANTHROPIC_*`).
  - **B6** the wall held: the box reaches ONLY the proxy — a DIRECT Anthropic dst and a non-model dst are
    DROPPED; vault ENOENT; host sentinel absent. Result: **23 PASS / 0 FAIL**.
- **Sealed-VM gate — `mount-plane-gate` (P6-3):** the same S3/P6B/P6-2 split. The endpoint-free VM (no
  NIC/resolver; the proxy is broker-side, never in the image) proves the sealed coder with
  `--provider anthropic` WIRES `model-anthropic` into the genuine-T2 constructor on the sealed kernel,
  then FAILS CLOSED — coder never dials, no secret/TLS ever in the box.
- **LIVE smoke (opt-in, non-gating):** point `SHREK_PROXY_UPSTREAM=api.anthropic.com:443` +
  `SHREK_ANTHROPIC_KEY_FILE=<broker path>` at the real API. Broker-side only; never committed, never
  boxed; NOT part of the deterministic gate.
- **Unit tests:** the provider parser (strict/fail-closed), the Anthropic request builder (system-lift +
  `max_tokens`), the Anthropic reply extractor, and the proxy's HTTP/hostport helpers.

## 6. Deferred / residuals (tracked, not built)

- **The LIVE Claude smoke** is wired but opt-in; a real end-to-end against `api.anthropic.com` is a
  separate, non-gating run.
- **A self-contained (musl-static, no-curl, single-binary) proxy** — v1 uses rustls+ring vendored; a
  smaller footprint is a hardening follow (like the coder's musl step).
- **A generic multi-provider / plugin framework** — deliberately NOT built. The seam is exactly the two
  concrete implementations; a third provider is a localized addition, not a framework.
- **Semantic DLP / egress tripwire at the proxy** (security-model.md §7, E1) — the proxy is the natural
  chokepoint to place it; not built here.
- **rustls supply-chain footprint** — the one dependency-tree expansion (vendored, auditable); the sealed
  planes stay dep-free and the coder keeps its single tinyjson dep.

## 7. Naming (`anthropic`) and the AI-ref scan

The provider id `anthropic` and the model id (e.g. `claude-sonnet-5`) are legitimate THIRD-PARTY PRODUCT
identifiers for a real product feature — distinct from the repo's "don't fingerprint the assistant that
built this" convention. They appear here and in `crates/{coder,model-proxy}` by design; the AI-ref scan
tripping on them is expected, not a leak.
