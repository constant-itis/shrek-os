# Phase-6 slice-2 — the first real coding-agent workflow

> Slice-1a/1b built the *box*: an integrity-banded T2 gVisor session with write-through project/build
> grants (`p6-coder-proof`) and named egress (`p6-egress-proof`). Its "coding" was a hardcoded shell
> string. This slice replaces that string with a **real agent**: a model receives a bounded task,
> and a first-party binary drives `inspect → model → edit → build/test → return` — inside the exact
> same box, over the exact same wall. No new isolation primitive.

The load-bearing reframing: **`shrek run`'s WORKLOAD *is* the agent.** The front door already composes
the whole session (provenance/tier → project write-through → exec build area → T2/gVisor → named
egress → execution → teardown). So "the coding agent" is not a new plane — it is the program that
runs as the workload. This slice ships that program (`crates/coder`), the one sealed egress profile
it needs (`model-local`), a bounded task fixture, and a deterministic acceptance gate plus a live
smoke.

---

## 1. What this slice adds — and what it reuses verbatim

| Concern | Mechanism | Status |
|---|---|---|
| session composition | `shrek run --project --build --egress -- coder …` | **reused** (crates/shrek, shipped) |
| T2 containment | gatekeeperd `t2_plane` gVisor/runsc constructor | **reused** (slice-6) |
| trust banding | `--ingest-harness` ⇒ authentic runsc ⇒ `T-untrust` | **reused** (slice-1a `ingest_admit`) |
| write-through grants | `--rw-grant` (project, noexec) + `--build-grant` (exec) | **reused** (slice-1a `mount_plane`) |
| named egress | `--egress-profile NAME` ⇒ sealed nft allow-list | **reused** (slice-1b) |
| **the agent** | `crates/coder` — model-driven loop, 4 tools | **NEW** |
| **model reach** | sealed `model-local` egress profile | **NEW** (one policy entry) |
| **task fixture** | a small failing-C repo | **NEW** (test asset) |
| **acceptance** | canned-responder gate + `--live` 35B smoke | **NEW** (spike, strip before ship) |

**Non-goals (parked — reopened only if this workflow exposes a concrete blocker, none did):**
- `agentd` identity/attestation plane (agents.md §5/§6) — v1 launches exclusively through `shrek run`;
  `agentd resolve` is untouched. agentd orchestration waits for a slice that needs persistent/session
  coordination.
- The §8 taint / confused-deputy *enforcement* plane — v1 documents the residual (§5 below); it does
  not build taint-tracking. The T2 wall bounds a bad edit to the project grant regardless.
- Rust/cargo toolchain in the sealed rootfs (the coder is a single static ELF, not a toolchain).
- Swamp / `shrek find|history`, broker-routing, grant-protocol prompt, multi-agent delegation.

## 2. The coder agent (`crates/coder`)

A single first-party ELF, sealed into the T2 rootfs beside `tcc`/busybox with its dynamic closure
(`ldd`: `libc`, `ld-linux` — already sealed for `tcc` — plus `libgcc_s`), exactly the closure pattern
`seal-t2-artifacts.sh` uses for `tcc`. Its integrity rides the image dm-verity seal (§5). Build-time
deps for JSON are permitted (we do not hand-roll a JSON parser to claim dep-free) — one zero-transitive
-dependency crate (`tinyjson`); the HTTP call is a plain `POST` over `std::net::TcpStream` because the
v1 endpoint is plain HTTP (no TLS in-sandbox — see §4), so JSON is the only dependency.

> **musl-static is the intended end state** (a single closure-free file), deferred only because the
> build host lacks the musl toolchain (`rustup`/`musl-gcc` absent). Shipping the glibc closure like
> `tcc` is the proven-under-gVisor stand-in; installing `x86_64-unknown-linux-musl` later shrinks the
> sealed footprint to one inode with no behavior change.

```
coder --task "<one-line task>" [--model-url http://shrek-model:8100/v1/chat/completions]
      [--max-steps N] [--live]
```

The loop, bounded to ONE task and a hard step cap (default small, e.g. 8):

```
1. read the task (--task) and list the project (CWD = /srv/<project>, shrek run cd's here)
2. POST a `chat/completions` request to the model over the sealed egress
3. parse ONE tool-call from the reply (serde_json), execute it, append the result to the transcript
4. goto 2 until the model calls `done`, or the step cap trips (fail-closed: non-zero exit)
```

**The four tools** (the entire surface the model may drive — everything maps to std within the grants):

| tool | effect | bounded by |
|---|---|---|
| `read_file{path}` | read a project file | the rw project grant (host-noexec) |
| `write_file{path,content}` | write/replace a project file | the rw project grant (write-through) |
| `run{cmd}` | run a build/test command (`tcc …`, run the ELF) | the exec build grant + rootfs `tcc`/busybox |
| `done{ok,summary}` | end the session with a verdict | — (sets the exit code) |

`run` executes via the rootfs `/bin/sh`; compiler output is directed to `/srv/build` (the only
exec-capable grant), source edits land in `/srv/<project>` (noexec, write-through). This is exactly
the slice-1a exec split — the coder *uses* it, it does not change it.

The transcript, the model's chosen tool-calls, and each result are printed to stdout with anchored
markers (`CODER-STEP`, `CODER-TOOL`, `CODER-DONE ok=…`) so the acceptance gate greps outcomes, not
model prose.

## 3. The model endpoint & egress (`model-local`)

A new sealed profile in `shrek_policy::egress`:

```
model-local  →  { host: "shrek-model", proto: tcp, port: 8100 }   // exactly one destination
```

`shrek-model` is a sealed DNS *name*; gatekeeperd pre-resolves it to a pinned A-record and seeds
`/etc/hosts` inside the sandbox at construction (the slice-1b resolver path). So the coder dials
`http://shrek-model:8100/…` and it resolves to:

- **acceptance gate:** a deterministic canned HTTP responder the harness stands up at that address
  (mirrors `p6-egress-proof` mapping `github-https`→a local server). Fixed replies ⇒ a fixed sequence
  of tool-calls ⇒ reproducible expected output. No LLM in the gate.
- **`--live` smoke:** `shrek-model` maps to a real local 35B (`<lan-host>:8100`, LAN, no auth,
  standard `chat/completions` API). Informational / non-gating — same coder binary, same protocol
  path, only the backing server differs.

Port 8100/plain-HTTP is a first for the egress table (existing profiles are `tcp:443`); the
enforcement path is proto+dport, IPv4-only, so it is unaffected. The `model-local` profile is sealed
policy data, not a new primitive.

## 4. Trust, integrity, and the honest residual

- **The coder binary is trusted first-party**, integrity-anchored by the image dm-verity seal exactly
  like `tcc`. The **runsc harness remains the admitted inode** (`ingest_admit::derive_session(runsc)`)
  that earns `T-untrust`; this slice does not touch the admit-list. Structure is unchanged: authentic
  harness (runsc) + untrusted ingest (the project) ⇒ `T-untrust` ⇒ genuine T2.
- **But the coder *acts on* untrusted input** — the model's tool-calls and the untrusted project bytes.
  A first-party binary steered by untrusted data is the agents.md §8 confused-deputy surface. v1 does
  **not** solve it (the taint plane is deferred). The honest v1 stance, straight from agents.md §11: a
  bad/injected tool-call is **bounded by the T2 wall to the project + build grants** — worst case the
  agent writes garbage into the box it already owns; it cannot reach `$HOME`, the vault, or any host
  beyond `model-local`. The demotion "missed taint ⇒ in-profile action the wall already bounds, never a
  wall breach" is the guarantee we lean on.
- **The lethal trifecta is real and named:** untrusted-read + `model-local` egress = a genuine channel
  (ordinary readable bytes can be encoded into request params). v1 accepts it for a single-LAN,
  no-secrets model host and surfaces it here rather than pretending each capability is independently
  benign.

## 5. The bounded task fixture

A minimal failing-C repo (`tests/fixtures/coder-task/`): a freestanding `buggy.c` that is *supposed*
to print `REAL-COMPILE-RUN-OK` and exit 42 but does not (wrong marker / wrong exit). The task:

> "Make the program print REAL-COMPILE-RUN-OK and exit 42."

A real `inspect → edit → build → test → return` loop: the coder reads `buggy.c`, the model returns a
`write_file` fix, the coder compiles it with `tcc -nostdlib -static` into `/srv/build`, runs it, checks
the marker + exit == the pass criterion, and calls `done{ok:true}`. tcc-scale by construction — no
toolchain in the rootfs, so the cargo-rootfs track stays parked.

## 6. Proof plan

- **Acceptance gate — `scripts/p6-coder-agent-proof.sh`** (privileged debian:trixie oracle, spike-only,
  sibling of `p6-coder-proof`): stands up the canned responder at `shrek-model:8100`, runs the fixture
  task through `shrek run … --egress model-local -- coder --task …`, and asserts, deterministically:
  - **A1** authentic harness ⇒ `derived=T-untrust`, `construct-at=T2` (banding unchanged).
  - **A2** the agent loop ran: `CODER-STEP` ≥1, a `write_file` + a `run` tool-call executed.
  - **A3** real build/test: the edited source compiled, the ELF ran, marker+exit == pass, `CODER-DONE ok=true`.
  - **A4** write-through: the fixed `buggy.c` + compiled ELF are visible on the host afterward.
  - **A5** the wall holds during a *model-driven* session: vault ENOENT, host sentinel absent, and a
    non-`model-local` dst (e.g. `1.1.1.1:53`) is DROPPED even though egress is up.
  - **A6** fail-closed: harness digest absent ⇒ `T-hostile` ⇒ refuse, the coder never runs.
- **`--live` smoke — same script, `LIVE=1`:** maps `shrek-model`→`<lan-host>:8100`, runs the same
  binary against the real 35B, prints the transcript. Informational; a model that "solves it
  differently" is not a gate failure.
- **Unit tests (`crates/coder`):** the JSON request builder, the tool-call parser (incl. malformed ⇒
  fail-closed), and the pass-criterion checker — the pure pieces, no network.
- **Sealed-VM gate — `image/overlay/usr/lib/shrek/mount-plane-gate` (P6-2 section):** the hermetic image
  seals the coder + its ldd closure into the dm-verity T2 rootfs (`scripts/seal-t2-artifacts.sh`; the
  build is offline with `tinyjson` vendored in-tree, `.cargo/config.toml` + `CARGO_NET_OFFLINE`). On the
  Secure-Boot + dm-verity + lockdown VM, the SAME gatekeeperd argv that `shrek run --egress model-local
  -- coder` composes asserts, deterministically: the coder is **baked under dm-verity /usr**; a
  model-driven coder session derives `T-untrust` (baked admit-list authenticated the runsc harness);
  `model-local` egress **wires into the genuine-T2 constructor**; and — the endpoint-free VM having no
  resolver — the session **fails closed** so the coder never dials and never runs (no silent open
  network), leaving no residual egress plumbing. This follows the S3/P6B split: the VM proves
  *seal + wiring + fail-closed* on the sealed kernel; positive end-to-end reach (the model actually
  driving the loop) stays the oracle. Result: **94 PASS / 0 FAIL** across the full gate.

## 7. Deferred / residuals (tracked, not built)

- §8 taint / confused-deputy enforcement — the next security slice for the agent plane.
- ~~Sealed-VM gate needs the coder's build deps vendored~~ — **SHIPPED.** `tinyjson` is vendored in-tree
  (`vendor/` + `.cargo/config.toml`), STAGE 1 builds offline (`CARGO_NET_OFFLINE`), `seal-t2-artifacts.sh`
  bakes the coder + closure into the dm-verity T2 rootfs, and the P6-2 VM gate proves seal + wiring +
  fail-closed on the sealed kernel (94/94). glibc-dynamic + sealed ldd-closure (like tcc); a musl-static
  one-inode coder remains the documented next step (no rustup/musl toolchain on the build host today).
- Frontier/TLS model endpoints (needs in-sandbox TLS + secret handling) — the **model-provider
  abstraction** (Part 2) addresses this; v1 is plain HTTP to a LAN model. The authority-preserving
  enabler is a broker-side authenticated egress proxy (security-model §7): the key lives in the proxy,
  the sandbox's sealed egress dst IS the proxy, so no secret and no TLS stack ever enter the T2 box.
- Rust/cargo toolchain in the rootfs — the coder is a static ELF; multi-language build tasks await the
  separate rootfs-scaling track.

## 8. Frozen v1 workload contract

As of this commit, **`crates/coder` is the FROZEN v1 coding-agent workload contract.** The model-provider
abstraction (next slice) builds *on top of* it and MUST NOT change any of the following:

- **The authority model is the workload's environment, not the workload's concern.** Authority =
  `sealed egress-profile ∩ grants`, bounded by the T2 (gVisor) wall, over **NAME-only** egress
  (`--egress-profile model-local`; the `host:proto:port` set is authored only in the sealed policy). The
  coder never sees, names, or can widen a destination. A "provider" is nothing more than
  `(sealed egress-profile NAME, wire-adapter[, broker-side auth-injection point])` — it MUST NOT add any
  path that widens authority.
- **The bounded loop + tool surface:** `inspect → model → edit → build/test → return`, exactly four tools
  (`read_file`, `write_file`, `run`, `done`), a hard `--max-steps` cap that trips fail-closed.
- **Integrity from the seal, not the input:** the coder is first-party, sealed in the dm-verity rootfs;
  the ingest admit-list measures the *runsc harness*, so runsc stays the admitted harness and the coder
  rides in under dm-verity. The coder acts on untrusted input (model tool-calls + project bytes) — the
  §8 confused-deputy residual is documented-not-built and bounded by the T2 wall + project grant.
- **Supply chain:** exactly one vendored, zero-transitive dependency (`tinyjson`) scoped to this crate;
  the sealed planes stay dep-free. Any provider adapter keeps that discipline.
