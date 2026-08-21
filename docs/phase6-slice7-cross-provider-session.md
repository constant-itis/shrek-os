# Phase-6 slice-7 — cross-provider stateful sessions (opaque handle → native `--resume`)

Status: **DESIGN-OF-RECORD (boundary owner-locked 2026-08-20).** Implementation follows this
contract. Supersedes the "stateful `--resume`" residual tracked in slice-4 §6 and slice-6 §7.

## 1. What this slice adds — and what it reuses verbatim

Slices 4/5/6 shipped two subscription-CLI providers (claude-broker, codex-broker) that each **flatten
the whole growing transcript to a one-shot CLI call every step** (stateless). A bounded coder loop of N
steps re-sends O(N²) transcript bytes and pays the model to re-read the entire conversation each turn.

This slice makes the conversation a **real session**: the coder attaches one opaque handle per session;
each broker maps that handle to its CLI's *native* session and forwards only the **new tail turn** via
native resume. Fewer tokens, closer to the CLIs' own statefulness.

Reused verbatim: the sealed egress names (`model-claude-cli`:8300, `model-codex-cli`:8301), the box→broker
plaintext messages-API wire, the codex sterile bwrap view + reader-disable gate (slice-6 §3), the
fail-closed round-trip health (never a status-cache liar, #1567), the claude argv-vector no-shell
translation (slice-4). No new egress name, no new wall hole, no widened authority.

## 2. The invariant that shapes the whole design

**Session identity is an opaque handle, NEVER derived from transcript contents, and carries NO
filesystem or network authority.** The handle only *binds conversational state*; it never selects a
mount, a host, an egress profile, or a credential. This is deliberate: deriving identity from transcript
bytes would let crafted conversation content collide with, or hijack, another session's native id — a
confused-deputy vector (agents.md §8 class). An opaque handle decouples *who this conversation is* from
*what it says*.

Corollary (correctness): **native resume is a pure optimization over a proven-correct base.** If resume
fails for ANY reason — unknown/expired native id, a transcript that doesn't extend what was forwarded, a
corrupt or missing session store, a broker restart that dropped the in-memory map — the broker falls back
to the slice-4/5/6 **flatten single-shot**, which reproduces the full conversation and is always correct.
Sessions never introduce a *wrong* answer, only a *cheaper* one.

## 3. The wire contract (coder → broker)

- **Transport:** a single request header `X-Shrek-Session: <opaque>`. The messages body stays
  byte-for-byte the Anthropic messages-API shape both brokers already parse — no body-schema change.
- **Same header for both brokers** (the unified half of the contract).
- **Absent header ⇒ stateless flatten**, exactly as slices 4/5/6 — backward compatible, and it is also
  the fallback path.
- **Coder side:** the coder mints ONE handle per `run_loop` invocation (= per session): `SHREK_SESSION`
  env if the launcher set it, else 16 bytes of `/dev/urandom` hex-encoded (opaque, unique, no uuid dep;
  if urandom is unreadable it falls back to pid⊕step but still opaque). It attaches the same handle on
  every request in that loop. The coder change is *only* this attach — no protocol/tooling change.

## 4. Broker-side session registry (designed once, implemented independently per §7-fork-4)

Each broker keeps an in-process registry `handle → SessionEntry { internal_id, native_id,
state_dir, msgs_forwarded }` behind a `Mutex` (brokers are already multi-threaded, one `handle()` per
connection). Per sessioned request with a messages array of length `N`:

- **First sighting** (handle not in map): mint a broker-owned `internal_id`, create the native session,
  forward the flatten of `messages[0..N]` (establishes context), store the entry with `msgs_forwarded = N`.
- **Continuation** (handle known, `N > msgs_forwarded`): forward only the flatten of
  `messages[msgs_forwarded..N]` (the delta — normally one user/tool-result turn) via native resume,
  update `msgs_forwarded = N`.
- **Divergence** (`N <= msgs_forwarded`, or native resume errors): drop the registry entry and take the
  flatten fallback for this call; a fresh handle re-establishes cleanly next loop.

The transcript is used ONLY to compute *what is new* (the delta payload). Identity is the handle. These
are different things and the invariant (§2) is not violated.

**Per-session serialization (owner requirement).** A native CLI session is a single mutable thread — two
overlapping resume calls on the same session would corrupt its state. Each session entry therefore carries
its own lock; requests bearing the **same** handle serialize on it, while **different** handles run
concurrently. The registry `Mutex` guards only the map (lookup/insert); the per-session lock guards the
actual CLI call so a slow turn never blocks unrelated sessions.

**Handle → broker-owned internal id (owner requirement).** The raw header text is NEVER placed in a
filesystem path or an argv. On first sighting the broker mints its own opaque `internal_id`
(broker-owned, e.g. a broker UUID / counter+random). That internal id — never the caller's handle — names
the codex on-disk state dir and keys native session naming. The caller handle is used *only* as an
in-memory lookup key, and is validated to a safe charset/bounded length before that. Claude's native id is
a broker-minted UUID derived from the internal id; codex's is CLI-minted and captured. The untrusted
header thus never reaches a path, an argv, or a credential surface.

## 5. Native binding — the part that legitimately differs per CLI

The shared abstraction is *handle → native-session mapping*. HOW each native id is obtained and WHERE
each session store lives differ, and that difference is inherent to the two CLIs (probed live, read-only):

| | claude-broker | codex-broker |
|---|---|---|
| native id | **broker-minted** `--session-id <uuid>` (CLI supports it) | **CLI-minted**; broker captures `session_meta.session_id` from the new rollout in the bound sessions dir |
| first call | `claude -p --session-id <uuid> --system-prompt <sys> "<flatten 0..N>"` | `codex exec` (sterile bwrap, sessions dir bound, **no `--ephemeral`**) with the flatten as prompt |
| continuation | `claude -p --resume <uuid> "<delta>"` | `codex exec resume <id> "<delta>"` (same bwrap, same bound per-session sessions dir) |
| session store | the real broker-side `~/.claude/projects/<cwd>/…` (claude-broker runs the CLI directly, no bwrap — persistence is free) | a **broker-owned per-Shrek-session dir**, bound RW into the sterile view as `$CODEX_HOME/sessions` — see §6 |

`codex exec resume` honors `--disable`/`-s read-only`/`-c model_providers.*` (verified in `resume --help`),
so **every slice-6 confinement + reader-disable flag is carried onto the resume call unchanged.**

## 6. Codex session state vs the sterile view (the residual, folded in)

Slice-6 sets `--ephemeral` ("run without persisting session files to disk") and gives codex a per-call
tmpfs home — so native resume was impossible (nothing recorded, and it would vanish anyway). This slice:

- For **native-session calls only**, drops `--ephemeral` and binds ONE broker-owned per-session directory
  RW into the sterile view as the codex session store (`$CODEX_HOME/sessions`, dir named by the handle
  hash, §4). Everything else in the sterile view is unchanged: `--unshare-*`, `--clearenv`,
  `--new-session`, `--tmpfs /home` **before** the runtime ro-bind (the nvm-under-/home gotcha), the
  reader-disable flags, `-s read-only`, `--ignore-user-config`, and the **RO-bound `auth.json`**.
- The new RW surface is bounded and non-authority-bearing: it holds *only* session transcript material,
  scoped to one Shrek session, broker-owned, and **lifecycle-expired** (created on first-seen handle,
  removed when the session ends / on a TTL sweep). It is a distinct mount from `auth.json` (which stays
  RO) and from the `-o` scratch.
- **auth.json refresh residual — dissolved, not patched.** Each turn is a *separate* `codex exec [resume]`
  with a *freshly constructed* bwrap that RO-binds the **current** host `auth.json`. A host-side token
  refresh **between** turns is therefore picked up automatically on the next turn. The only unreachable
  case is a refresh needed *mid-single-exec* — as short as slice-6's calls today. This is exactly why the
  design is one-exec-per-turn-via-resume rather than one long-lived codex process.

## 7. Proof plan

- **Unit (host, `cargo test --lib/--bins`):** handle charset/length validation + hash-to-path is
  traversal-safe; registry delta math (first=all, continuation=tail, divergence→fallback); absent-header
  ⇒ stateless path unchanged; claude `--session-id`/`--resume` argv shape; codex resume argv carries every
  reader-disable + `-s read-only`; session state carries no path/host/profile.
- **Host oracle (real CLIs + real bwrap, no quota):** a multi-turn session where turn ≥2 forwards ONLY the
  delta (assert the earlier turns are absent from the captured native request), the native session retains
  prior context, the codex sterile view still shows host vault/project ENOENT and the auth.json canary
  absent from request/reply/state dir/breadcrumb during a *resumed* call, and forced resume-failure falls
  back to a correct flatten answer.
- **Sealed image + VM gate (P6-2 extended):** because the coder is sealed under dm-verity, re-seal the
  coder and prove the **sealed** coder emits `X-Shrek-Session`, still bands `T-untrust`/T2, and still
  fails closed. First sealed-image touch since slice-2 (slices 4/5/6 were broker-only).
- **Live smoke (owner GO, spends quota):** `SHREK_*_LIVE=1` a 2-turn resumed conversation against the real
  logged-in claude and codex; seam correctness required, model answer informational (owner rule).

## 8. Deferred / residuals (tracked, not built)

- **Registry durability across broker restart.** v1 registry is in-memory; a broker restart drops
  handle→native_id, so the next call re-creates via the fallback. Persisting the map is a later hardening.
- **broker-core extraction.** Per owner directive, NOT now: implement the contract independently in each
  broker; extract a shared session lib only after both native-resume paths are green and the real
  commonality is observable.
- **Session TTL/GC policy tuning.** v1 uses a simple end-of-session removal + coarse TTL sweep for the
  codex state dir; a richer lifecycle is deferred.
