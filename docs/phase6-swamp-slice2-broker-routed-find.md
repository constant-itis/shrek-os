# Phase-6 (Swamp track) slice-2 — broker-routed in-sandbox `shrek find`

> **Track note.** This is the **Swamp / semantic-filesystem** Phase-6 track (`swampd`), distinct from
> the completed **coding-agent enablement** Phase-6 track (`shrek run`, the model brokers). This slice
> is the integration seam between the two: it lets a coding agent running *inside* a T2 gVisor sandbox
> query the swamp — **without** handing the sandbox a hole in its wall. It is the parked "Path-2" from
> slice-1 (`phase6-swamp-slice1-query-gate.md` §residuals), now scoped and adjudicated.
>
> **Status: DESIGN OF RECORD — boundary adjudicated + amended, BUILD GO.** Forks locked by the owner
> 2026-08-21; four hardening amendments folded in the same day (VM regression required; host-enforced
> per-veth source anti-spoofing; atomic binding create/revoke with no stale-IP reuse; swamp-broker
> classified TCB for session selection). No swampd changes; no embeddings/watcher/persistence.

## The gap (the only thing that must be built)

Slice-1 shipped the whole query wall: `swampd` resolves authority from a root-owned record keyed on a
session handle (`SO_PEERCRED = identity only, the record = authority`), and host-side `shrek find`
reaches `swampd`'s unix socket directly. A T2-sandboxed coder **cannot** reach that socket — that is
the wall doing its job. So the one missing piece is a **wall-respecting path from inside the sandbox to
`swampd`**, and the identity binding that keeps it honest.

Everything else already exists and is reused unchanged:

- the `swampd` query server + wire protocol + authority resolution (`swampd/src/{server,authority}.rs`)
- the gatekeeperd authority-record writer (`gatekeeperd/src/authority_record.rs`)
- the sealed egress model — per-sandbox `/30` veth + default-deny nftables + pinned-IP `/etc/hosts`
  (`gatekeeperd/src/{net_plane,t2_plane}.rs`, `shrek-policy/src/egress.rs`)
- the model-broker precedent: an in-sandbox workload reaching a host-side broker over a **named** sealed
  egress destination (`model-anthropic → shrek-model-proxy:8200`, `model-claude-cli → …:8300`)

## The invariant this slice must not break

```
semantic authority ≤ data authority          (architecture.md §5)
in-sandbox find authority == that sandbox's own data grants, never more.
```

The query crosses the wall, but the **authority does not widen** by crossing it. `swampd` still
intersects `session-record ∩ domain-ceiling ∩ sealed-allow-set`; the broker **cannot forge or widen a
grant** (it cannot write the root-owned record `swampd` reads). What the broker **can** do — and what
makes it trusted — is *select which session* a query resolves to (`cont_ip → session`). See
[Trusted computing base](#trusted-computing-base-tcb): the broker is **TCB for session selection**, and
is treated as such rather than assumed contained.

## Trusted computing base (TCB)

Be honest about the boundary this slice moves. The swamp-broker sits on the authority path and performs
**session selection**: it maps a connection's `cont_ip` to a session and chooses which handle to forward
to `swampd`. Therefore:

- **The broker is TCB for session selection.** A *compromised or confused* broker can forward some
  *other* live session's handle → cross-session read within the set of active sessions. This is a real
  failure mode; it is **not** "contained by construction." We mitigate it, we do not wish it away:
  - the broker is minimal, dep-free, off the sealed image, and takes untrusted bytes on exactly one
    path (the query wire) with a bounded parser;
  - it **cannot widen** authority beyond a real session's grants (`swampd` still intersects the sealed
    allow-set + ceiling + the root-owned record) — so the blast radius of a broker compromise is
    "another active session's already-granted data," never the whole filesystem;
  - the `cont_ip → session` binding it consults is **root-owned and broker-unwritable** (the broker
    reads it; only gatekeeperd writes it), so a broker bug cannot *invent* a binding, only misuse an
    existing one.
- **The broker is NOT trusted to authenticate the caller's uid** — it cannot (post-masquerade there is
  no peer uid across the veth). Caller authenticity rests on **host-enforced per-veth source
  anti-spoofing** (below), not on the broker.

The security goal of the amendments below is to shrink the broker's TCB surface to *exactly* "pick the
session bound to the wire the packet actually arrived on," and to make that wire unforgeable **at the
host**, so the broker never has to trust anything the sandbox says about who it is.

## The authority model across the hop (the crux, as adjudicated)

A routed query means `swampd`'s `SO_PEERCRED` sees the **broker's** uid, not the workload's — so peer
credentials can no longer even *identify* the caller across the hop. The resolution (Fork 1, owner-
decided): **transport identity gates handle forwarding.**

```
SHREK_SESSION            = opaque IDENTITY handle. NOT a bearer authority credential.
transport identity       = the sandbox's un-forgeable /30 source IP (a netns can route ONLY its own /30)
construction-time truth  = gatekeeperd binds  cont_ip → session  (root-owned) when it builds the sandbox
broker rule              = forward the handle to swampd ONLY IF  binding(peer cont_ip) == request handle
                           else → fail-closed empty projection (indistinguishable from no-match)
```

So a handle is worthless off its own wire: even if it leaked to another sandbox, that sandbox's source
IP maps to a *different* session, and the broker refuses to forward it. The handle stays the key
`swampd` resolves the record on (Fork 4: `swampd` unchanged); the IP binding is the gate that authorizes
forwarding it.

### Why the source IP is trustworthy — host-enforced, not merely routed (amendment 2)

A routing/default-deny argument alone ("the netns only routes its own `/30`") is **not** sufficient: a
compromised sandbox controls its own packet-crafting and could emit a frame bearing *another* sandbox's
`cont_ip` as source. If that forged source reached the broker, the whole `cont_ip → session` binding
would be defeated. So `cont_ip` authenticity is enforced **at the host, per veth**, and proven:

```
# per-sandbox nft ruleset — ingress anti-spoof on THIS sandbox's host-side veth:
chain prerouting {
    type filter hook prerouting priority -300; policy accept;
    iif "skh<idx>" ip saddr != {cont} drop        # <-- NEW: the veth may source ONLY its own cont_ip
}
```

(plus per-interface reverse-path hardening — `rp_filter=1` / equivalent — on the host veth as
defence-in-depth). A packet arriving on `skh<idx>` whose source is not that sandbox's `cont_ip` is
dropped at the host **before** routing, so a sandbox physically cannot impersonate another's binding.
The oracle proves this directly (gate **B-spoof**).

**The masquerade carve-out (Mechanism A).** The current net plane SNATs all egress: `net_plane.rs`
installs `postrouting … ip saddr {cont} masquerade`, so by the time traffic reaches a shared host
listener the source has been rewritten to the host address — every sandbox looks identical. Mechanism A
carves the swamp-broker destination out of masquerade so the (now anti-spoof-verified) `cont_ip`
survives to the broker:

```
# in the per-sandbox nft ruleset, BEFORE the masquerade rule:
chain postrouting {
    type nat hook postrouting priority srcnat; policy accept;
    ip saddr {cont} ip daddr {swamp_broker_ip} return   # <-- NEW: no SNAT for the swamp path
    ip saddr {cont} masquerade                           # everything else unchanged
}
```

The broker then reads `getpeername()` → `cont_ip` and looks up the binding. Per-profile gating comes
free: only a sandbox granted the `swamp-query` egress profile gets the FORWARD `… daddr {swamp_broker}
… accept` rule, so an ungranted sandbox is dropped before it ever reaches the broker (and never gets a
binding written either).

### Binding lifecycle — atomic, revoked, no stale reuse (amendment 3)

`/30` slots are assigned by a hash of the sandbox id (`djb2(id) & 0x3FFF`), so a `cont_ip` **is reused**
across sandbox lifecycles. A stale binding for a dead session must never authorize a *new* sandbox that
happens to land on the same `/30`. So the binding record is lifecycle-managed with the same rigor as the
authority record:

- **Atomic create, before traffic.** gatekeeperd writes `cont_ip → session` (temp → `fsync` → `rename`
  → dir `fsync`), **before** `runsc` starts, so the sandbox can never emit a query that races ahead of
  its own binding. Root-owned, workload-unreadable (`root:swamp 0640` in a `0750` dir), like the
  authority record.
- **Revoke on every teardown path.** The binding is removed when the sandbox is torn down — on the
  success path *and* every error path (mirrors the netns teardown in `t2_plane.rs`). A create for a
  reused `cont_ip` first removes any prior binding for that IP (idempotent replace), so a crashed
  predecessor cannot leave a live-looking binding.
- **Reuse regression gate (B-reuse).** Sandbox A → `cont_ip X`, session `SA`; A tears down (binding
  revoked); sandbox B reuses `X` with session `SB`. A query from `X` must resolve `SB`, never `SA`; and
  a query from `X` with **no** current binding (post-teardown, pre-reuse) must fail closed.

## Components

| # | Component | New / changed | Role |
|---|-----------|---------------|------|
| 1 | `crates/swamp-broker` | **NEW** (off the sealed image, default-members-excluded, sibling of the model brokers) | Host-side forwarder. Accepts the in-sandbox query, verifies `getpeername→cont_ip` against the binding + handle, forwards to `swampd`'s unix socket as an allowed uid, returns the projection. |
| 2 | `shrek-policy/src/egress.rs` | **+1 profile** `swamp-query → shrek-swamp-broker:tcp:8400` | Sealed named destination, distinct from `model-*` (8200/8300/8301). |
| 3 | `gatekeeperd/src/net_plane.rs` | **+2 nft rules** | Mechanism-A masquerade carve-out for the swamp-broker dst **and** per-veth ingress source anti-spoof (`iif skh<idx> saddr!=cont drop`) + per-interface rp_filter. |
| 4 | `gatekeeperd` construction/teardown | **NEW binding record** `cont_ip → session` (root-owned `/run/shrek/net-binding/<cont_ip>`, `root:swamp 0640` in `0750`) | The transport-identity truth the broker consults. **Atomic create before traffic; revoked on every teardown path; reused-IP replace removes stale.** Root-owned, workload-unwritable/unreadable. |
| 5 | `crates/coder` | **+1 tool** `swamp_find` in the model tool-loop | The in-sandbox agent can search the project's swamp; the tool dials the `swamp-query` egress carrying `$SHREK_SESSION`, returns hits to the model. |
| 6 | `crates/swampd` | **UNCHANGED** | Frozen. Still `SO_PEERCRED`-gates an allowed uid + resolves authority from the handle-keyed record. |

## Data flow (one routed query)

```
coder (in T2 sandbox, uid=workload, SHREK_SESSION=H)
  └─ model calls tool swamp_find(q)
       └─ POST http://shrek-swamp-broker:8400  { session:H, intent, scope, q }     (plaintext, sealed egress)
            └─ [veth /30: no-SNAT carve-out preserves saddr = cont_ip]
                 └─ swamp-broker (host):
                      getpeername() → cont_ip
                      binding(cont_ip) == H ?  ── no ─→ RESULT 0 (fail-closed empty)
                                             └─ yes ─→ connect /run/swamp/query.sock  (broker uid allowed)
                                                        send  QUERY 1 / session H / … / END
                                                        swampd: authority = record(H) ∩ ceiling ∩ allow-set
                                                        ← RESULT n / hit … / END
                      ← wrap hits back to the coder tool result
```

`swampd`'s projection is exactly what a *host-side* `shrek find --session H` would return — the routing
changes *reachability*, never *authority*.

## Decisions (the four forks, as adjudicated 2026-08-21)

1. **Cross-hop identity** → **transport-identity-gated forwarding (Mechanism A).** Handle is opaque
   identity, not a bearer credential. Broker forwards only after `getpeername→cont_ip` matches the
   construction-time `cont_ip→session` binding. (Rejected: handle-as-bearer — a leaked handle must not
   be sufficient.)
2. **Wall crossing** → **sealed network egress.** New `swamp-query` profile; reuse the proven veth +
   nftables + pinned-IP model. No socket bound into the sandbox (that would be a wall hole outside the
   NAME-only egress model). 
3. **Caller scope** → **coder gains a `swamp_find` tool.** The real feature: the in-sandbox agent
   searches the swamp from its model loop (not merely a routed `shrek find` binary).
4. **`swampd` change** → **none.** The proven query wall stays frozen; the broker connects as an
   allowed uid, authority stays handle-keyed on the record.

## Proof — `scripts/swamp-broker-find-proof.sh` (host-side oracle)

The wall being crossed is a **network + identity** boundary, so the oracle must exercise a real
sandbox source IP against a real broker. Planned gates (fail = FAIL, acceptance 0 FAIL):

- **B1 happy path.** A query from the bound `cont_ip` carrying the matching handle returns the same
  projection a host-side `shrek find` returns for that session (seeded app-a hit present).
- **B2 stolen-handle, wrong wire.** The *same valid handle* presented from a *different* sandbox's
  `cont_ip` → `RESULT 0` (transport binding refuses; the leak is inert).
- **B3 unbound source.** A `cont_ip` with no binding → `RESULT 0` (fail-closed, not an error; session
  existence not probeable).
- **B4 no authority widening.** A bound session whose grants exclude app-b cannot discover app-b
  tokens via the routed path (BBSECRET absent) — parity with slice-1's query gate across the hop.
- **B5 masquerade carve-out is exact.** Only the swamp-broker dst is un-SNAT'd; all other egress from
  the same sandbox is still masqueraded (a control dst still sees the host IP), and an ungranted
  sandbox cannot reach `shrek-swamp-broker:8400` at all (FORWARD drop).
- **B6 swampd frozen.** `swampd` sees an allowed uid (the broker), never the workload; the record is
  read, never a request-supplied grant.
- **B-spoof (amendment 2) source anti-spoof is host-enforced.** A sandbox emitting a frame with a
  *forged* source (another sandbox's `cont_ip`) is dropped at its host veth (`iif skh<idx> saddr!=cont
  drop`) and never reaches the broker — so it cannot impersonate another session's binding.
- **B-reuse (amendment 3) no stale-IP reuse.** After sandbox A (`cont_ip X` → `SA`) tears down, a query
  from `X` fails closed; when sandbox B reuses `X` (→ `SB`), a query from `X` resolves `SB`, never `SA`.

Unit: `swamp-broker` bins (binding lookup, handle/charset validation, fail-closed on mismatch/stale,
wire parse/wrap) + `gatekeeperd` (atomic binding create/revoke, reused-IP replace) + `shrek-policy`
(the `swamp-query` profile reaches exactly one dst, distinct from all `model-*`).

**Sealed-image / VM regression IS required (amendment 1).** This slice changes three *on-image* sealed
components — `gatekeeperd` (privileged broker, dm-verity `/usr`), `shrek-policy` (the sealed egress
table compiled into it), and `coder` (sealed into `t2-rootfs`) — so the image must be re-sealed and
re-gated, exactly like slice-7. Acceptance: rebuild via `build-in-container.sh` → `boot-vm.sh` →
**P6-2/P6-3 still green, the new mount-plane-gate assertions PASS, 0 raw `SHREK_GATE:FAIL`**. New gate
assertions: the sealed `coder` carries the `swamp_find` tool marker (re-seal not stale), and the sealed
`gatekeeperd`/policy carries the `swamp-query` egress name (sealed policy updated). The swamp-broker
crate itself stays off-image (default-members-excluded, like the model brokers); the network/identity
wall it rides is proven host-side in the oracle.

## Honest scope / residuals

- **In scope:** the routed query pipe, the transport-identity binding + carve-out, the coder
  `swamp_find` tool, the oracle. 
- **NOT in scope (deferred):** semantic/embedding tier, relationship/living graph, live fanotify
  watcher + index persistence (still slice-1's snapshot-in-`/run`), per-machine allow-set additions,
  a shared `shrek-sys` crate for the duplicated `linux_uapi` mirror.
- **Residual — handle entropy.** With Mechanism A the handle need not be a secret (the wire is the
  authenticator), but it remains construction-minted + bounded charset; we keep it high-entropy so a
  future non-carve-out transport can't regress to bearer semantics unnoticed.
- **Residual — one broker, many sandboxes.** A single `swamp-broker` serves all `swamp-query`-granted
  sandboxes; isolation rests entirely on the per-connection `getpeername` binding check, not on
  process separation. A per-sandbox broker (heavier) is the fallback if that check is ever found
  insufficient.

## Next candidates (Swamp track, after this slice)

Live fanotify watcher + index persistence (§6); semantic/embedding tier (own threat pass first, §8);
relationship/living graph; SWAMP search as a separately-installable per-domain unit (§8); factor the
shared `shrek-sys` crate.
