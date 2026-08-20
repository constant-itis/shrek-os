# Phase-4 spike (slice 2) — gatekeeperd: privilege-separate the merge

> Phase-4 milestone (roadmap): *routine privileged ops no longer need arbitrary root.* Slice 1 moved
> the layer-merge POLICY into oniond but left oniond running as root. This slice makes that milestone
> real for the Onion: **oniond drops root; a privileged, systemd-supervised `gatekeeperd` broker
> becomes the only merge-capable component**, and it independently re-checks every request against the
> sealed policy — so a compromised unprivileged oniond cannot widen the merge (isolation.md §7,
> threat-model ADV-8, security-model §4/§6/§7).

## Thesis

Split the Onion into an **unprivileged policy client** and a **privileged wall**, connected by a small
request/response protocol over a root-owned unix socket:

- **oniond** (now **unprivileged**, `User=shrek`): the policy brain. Reads the sealed enable-list,
  proposes a *desired* layer set, and asks the broker to realize it. Holds no mount privilege. Grows
  later (version/compat/per-user/rollback) — the interesting policy lives here.
- **gatekeeperd** (privileged, **long-running, socket-activated, systemd-supervised**): the wall and
  the ONLY thing that mounts/merges. For every request it **independently** re-derives the allowed set
  from the sealed `/usr/lib/shrek/onion-policy` (it trusts *nothing* from the caller), enforces the
  signature/verity gate via `--image-policy`, and performs the privileged mount+merge. Runs with
  **scoped** privilege (`CapabilityBoundingSet=CAP_SYS_ADMIN`, the merge's real need) — not arbitrary
  root. Authenticates the peer; writes the audit record.
- **shrekctl** (unprivileged operator CLI): `onion status | activate <name> | deactivate <name>` —
  a second client of the broker for the runtime API (the "full broker" the slice adds over boot-only).

The low-level layering stays `systemd-sysext` (architecture.md §3). The two-independent-checks shape
mirrors the agentd↔gatekeeperd contract exactly, so this slice is the reusable skeleton Phase 5 plugs
sandbox construction into.

## Component / privilege map

```
  unprivileged                         root-owned socket                 privileged (CAP_SYS_ADMIN only)
  ┌───────────────┐  merge/activate/   /run/shrek/gatekeeperd.sock       ┌──────────────────────────┐
  │ oniond (boot) │ ─ deactivate/ ───▶ (SocketMode 0660, group shrek) ─▶ │ gatekeeperd              │
  │ shrekctl (op) │   status requests                                    │  • mount store (ro)      │
  └───────────────┘ ◀── per-layer verdicts ──────────────────────────── │  • re-derive sealed set  │
                                                                          │  • expose enabled symlinks│
        reads /run/shrek/onion.json  ◀── writes audit ───────────────────│  • systemd-sysext merge/  │
                                                                          │    refresh under policy  │
                                                                          └──────────────────────────┘
```

All privileged FS work (mount the untrusted store, symlink the selected DDIs into `/run/extensions`
`/run/confexts`, merge/refresh) lives in gatekeeperd. oniond/shrekctl only talk to the socket.

## The wire protocol (dep-free, line-oriented)

Request = one line: `<VERB> [name…]`. Response = zero or more `RESULT` lines then one `END` line:

```
→ merge shrek-hello shrek-conf          (oniond's boot proposal; desired set)
← RESULT shrek-hello sysext merged
← RESULT shrek-conf confext merged
← RESULT shrek-extra sysext omitted not-enabled     (if the caller named it; refused by the wall)
← END 0 0                               (sysext_merge_rc confext_merge_rc)

→ activate shrek-extra                  (runtime; gatekeeperd re-checks sealed policy → refuses)
← RESULT shrek-extra sysext refused not-sealed-policy
← END 1 -

→ deactivate shrek-hello
← RESULT shrek-hello sysext deactivated
← END 0 -

→ status
← RESULT shrek-hello sysext merged
← END 0 0
```

No JSON on the wire (trivial to parse both ends with std). gatekeeperd still writes the canonical
structured audit to `/run/shrek/onion.json` (slice-1 shape) so `shrekctl onion status` and the console
proof are unchanged. Verbs: `merge`, `activate`, `deactivate`, `status`.

## The independent re-check (the invariant — locked by the threat model)

gatekeeperd treats the caller's names as a *request, never an authority*. For each requested layer it
independently confirms **(a)** the name is `enable`d in the sealed `/usr/lib/shrek/onion-policy` it
reads itself, and **(b)** the DDI passes the signature/verity gate (`--image-policy`). Anything failing
(a) → `refused not-sealed-policy`; failing (b) → `refused image-policy`. Therefore a compromised or
buggy oniond that asks for `shrek-extra` (signed and present, but not sealed-enabled) is **refused by
the wall** — the unprivileged side cannot widen the merge (ADV-8). This is G3.

## Runtime activate/deactivate (the "full broker")

- **activate `<name>`**: re-check (a)+(b); if ok, symlink the DDI into the search dir and
  `systemd-sysext refresh` (re-applies the overlay live — *[pending researcher confirmation of 257
  refresh semantics]*); return the verdict.
- **deactivate `<name>`**: remove the symlink and `refresh`; return `deactivated`.
- Both go through the same sealed re-check, so even the operator (shrekctl) cannot activate a
  non-policy or unsigned layer. Finer operator trusted-path gating (security-model: privileged grant
  ops are trusted-path class) is **deferred** — the sealed-policy+signature gate is this slice's
  wall.

## Fail model (locked by security-model §7 — two planes)

- **Layer plane fails CLOSED:** broker down / socket absent ⇒ oniond's `connect()` fails ⇒ **no layers
  merge**. oniond logs `broker unavailable — layers NOT merged (fail-closed)` and **exits 0**.
- **OS availability fails OPEN:** that failed oniond (a `Type=oneshot`, non-critical unit) must **not**
  block `multi-user.target` — the box still boots to login without its layers.
- **Supervised:** gatekeeperd is `Restart=always`, socket-activated, so a crash isn't a permanent DoS.

## Gates (G1–G4)

```
G1  privilege dropped   oniond runs non-root (prints uid); a DIRECT oniond-side merge probe is DENIED
                        (EPERM) — the privilege is really gone, the broker is the only path.
G2  broker parity       the slice-1 outcomes still hold, now executed by gatekeeperd on oniond's
                        request: shrek-hello MERGED, shrek-conf MERGED (good); shrek-extra OMITTED
                        (select); unsigned + tampered REFUSED. End-to-end through the socket.
G3  independent recheck a compromised oniond (SHREK_ONION_INJECT=shrek-extra) additionally REQUESTS a
                        signed-but-unsealed layer → gatekeeperd REFUSES it (not-sealed-policy); it does
                        NOT merge. The wall holds against a lying caller.   ← the security payoff
G4  fail-closed+boot    with the broker unreachable (no store / gatekeeperd masked), NO layers merge,
                        oniond logs fail-closed and exits 0, and the system STILL reaches login.
                        Runtime activate/deactivate round-trip verified too (the full-broker API).
```

**Milestone:** the Onion's privileged merge runs only inside a small, scoped, supervised broker that
independently re-checks the sealed policy; the policy brain holds no root; a compromised brain cannot
widen the merge; and the OS still boots when the broker is gone.

## Verification split (cheap-first, then VM)

- **Container/host repro** (seconds, no Secure Boot): the socket protocol, peer auth, the independent
  re-check refusal (G3 logic), fail-closed-on-connect-failure (G4 logic), and activate/deactivate
  `refresh` round-trips — all exercised with a fake or real `systemd-sysext` against a temp store.
- **VM** (the sealed base, enforcing Secure Boot): G1 (oniond non-root under real systemd, direct
  merge denied), G2 (real privileged merge parity through the broker), and one end-to-end G3 + a
  store-absent G4 (fail-closed + still boots).

## Build economy

One root rebuild bakes: gatekeeperd (already installs to `/usr/libexec/shrek`), `gatekeeperd.socket` +
`gatekeeperd.service` (scoped caps, `Restart=always`), the `shrek` system group, and the reworked
`shrek-onion.service` (`User=shrek`, non-critical). Thereafter the gates are cheap store swaps +
container repros. `scripts/gatekeeperd-proof.sh` drives G1–G4; `oniond-proof.sh` still covers the
merge outcomes now proxied through the broker.

## Deferred (later Phase-4 / Phase-5 slices)

- Operator trusted-path gating for runtime activate (privileged-grant policy).
- gatekeeperd's Phase-5 role: sandbox construction (pin-subtree-root + resolve-beneath mounts, tap +
  nftables egress) — this slice teaches it only the merge op, establishing the broker skeleton.
- TPM NV monotonic counter / signed grant manifest / anti-rollback (security-model §4) — policy stays
  dm-verity-static here; no mutable grants yet.
- SO_PEERCRED vs SocketMode-only auth: final decision folded in from the researcher (see below).
- Persistent writable layer store; version/compat + `.v/` vpick selection.
