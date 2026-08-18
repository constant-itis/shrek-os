# Phase-4 spike (slice 1) — oniond owns the layer-merge policy

> Phase-4 milestone (roadmap): *routine privileged ops no longer need arbitrary root* — the control
> plane (`shrekctl`, `oniond`, `gatekeeperd`) becomes real. This slice takes the **first** step the
> checkpoint named: **oniond takes over the layer-merge policy that `shrek-onion.service` currently
> hardcodes** in the `onion-merge` shell. The dangerous low-level work stays `systemd-sysext`
> (architecture.md §3: *oniond implements no layering*).

## Thesis

Replace the fixed `onion-merge` shell with **oniond**, a real control-plane binary that:

1. reads a **trusted, sealed on-image policy** (baked under dm-verity — *never* read from the
   untrusted store, per security-model.md: static policy lives in the image);
2. applies it to the untrusted layer store to **select** which signed layers belong on this machine
   — the first genuine policy decision oniond owns, versus today's "merge everything present";
3. drives `systemd-sysext`/`-confext` to merge **exactly the selected subset** under the same baked
   `--image-policy` trust gate;
4. emits a **structured JSON audit record** of every decision (selected / merged / refused-with-reason
   / signed-but-not-enabled) to volatile `/run`;
5. exposes the result to the operator via **`shrekctl onion status`**, which reads that record.

All Phase-2 refusal guarantees survive the swap: an unsigned or tampered layer is still refused — now
as a *reasoned* audit event rather than a shell `echo`.

## What this slice is NOT (honest deferrals — later Phase-4 slices)

- **No privilege separation yet.** oniond still runs as the boot service (root) and calls
  `systemd-sysext` directly. Dropping oniond's root and routing the merge through a `gatekeeperd`
  broker — the actual *"no arbitrary root"* milestone — is a **separate later slice**. This slice
  moves the *policy* out of hardcoded shell into a real daemon with reasoned, audited decisions:
  the foundation the broker split builds on.
- **No socket/IPC daemon.** oniond is **boot-invoked** (`oniond merge`), does select+merge once, and
  writes state to `/run/shrek/onion.json`; `shrekctl` reads that file. A long-running, socket-served
  oniond + an IPC protocol is deferred.
- **No rollback-a-bad-layer.** "Which layer caused this boot failure → roll it back" (architecture.md
  §3) wires into the S8 boot-assessment and is a later slice.
- **No version/compat solver, no `.v/` vpick, no per-user activation.** Selection here is a flat
  enable-list; compatibility metadata and versioned layer selection come later.

## The policy (trusted, sealed)

A single file baked into the sealed `/usr` tree — read-only under dm-verity, so it is as trusted as
the kernel: **`/usr/lib/shrek/onion-policy`**. Deliberately *not* TOML/JSON (avoids pulling a parser
crate — the control-plane crates are dependency-free by design). Format is a line-based allow-list;
`#` comments and blank lines ignored:

```
# Shrek Onion policy — which signed layers belong on this machine.
# oniond merges ONLY layers named here, and ONLY if they also pass the signature/verity gate.
enable shrek-hello
enable shrek-conf
```

A layer present on the store but **not** listed is *signed-but-not-selected*: omitted, with reason
`not-enabled`. A layer listed but **absent** from the store is `enabled-but-absent`. A layer listed
and present but failing the trust gate is `refused`. `enable`/omit is oniond's decision; the
signature/verity gate stays `systemd-sysext --image-policy`.

## Mechanism: how "merge only the selected subset" works

`systemd-sysext merge` merges **every** extension found in its search dirs — there is no per-name
"merge only X" flag. So *selection* = oniond controls what the search dir contains, and the trust gate
stays `--image-policy`. These are orthogonal (selection = which images are considered; image-policy =
whether a considered image is trusted enough to merge).

oniond, at boot (store already mounted read-only at `/run/shrek-store` by the unit's `ExecStartPre`):

1. read the sealed policy → the enabled set `{name…}`;
2. scan `/run/shrek-store/extensions/*.raw` and `/run/shrek-store/confexts/*.raw` (untrusted);
3. into a **private, clean search dir** (`/run/extensions` for sysext, `/run/confexts` for confext —
   tmpfs, oniond-owned), expose **only** the enabled DDIs
   — *[MECHANISM — pending researcher confirmation: symlink into the search dir if `systemd-sysext`
   follows symlinks in 257; else per-file `mount --bind`. Copying is the always-safe fallback.]* ;
4. `systemd-sysext  --image-policy='usr=signed+absent:root=signed+absent'  merge`
   then `systemd-confext --image-policy='root=signed+absent'  merge` — the SAME baked trust policy as
   Phase-2's `onion-merge`, so O3 cannot regress;
5. read `systemd-sysext status` / the merge exit codes and each DDI's fate, and write
   `/run/shrek/onion.json`:

```json
{
  "version": 1,
  "policy": "/usr/lib/shrek/onion-policy",
  "enabled": ["shrek-hello", "shrek-conf"],
  "layers": [
    {"name": "shrek-hello", "kind": "sysext",  "present": true,  "decision": "merged"},
    {"name": "shrek-conf",  "kind": "confext", "present": true,  "decision": "merged"},
    {"name": "shrek-extra", "kind": "sysext",  "present": true,  "decision": "omitted", "reason": "not-enabled"},
    {"name": "shrek-bad",   "kind": "sysext",  "present": true,  "decision": "refused", "reason": "image-policy"}
  ],
  "sysext_merge_rc": 0,
  "confext_merge_rc": 0
}
```

oniond **never hard-fails the boot** (a refused/omitted layer is survivable — the verdict is observed,
not fatal), matching the Phase-2 contract. The JSON is hand-written (no serde) — trivial for this shape.

## `shrekctl onion status`

Reads `/run/shrek/onion.json` and prints a legible table (merged / omitted / refused + reason) plus the
raw path. This is the observable surface that replaces "read the serial console." `shrekctl` gains its
first real subcommand; it stays a thin reader (no privilege, no merge).

## Gates (each a go/no-go, VM-verified; fast-iterated first in the container repro)

```
O1  parity        oniond replaces onion-merge. A signed, ENABLED layer merges; the verdict now comes
                  from oniond's /run/shrek/onion.json + `shrekctl onion status`, not shell echo.
                  (Phase-2 L1/L2/L4 stay green THROUGH the daemon.)
O2  selection     store has TWO signed sysext layers; policy enables only ONE → only the enabled one
                  merges; the other is signed-but-omitted (decision=omitted, reason=not-enabled).   ← new capability
O3  refusal       unsigned + verity-sig-tampered layers still REFUSED (Phase-2 L3 must not regress),
                  now recorded as decision=refused with a reason.
O4  audit/observe every decision is a structured record in /run/shrek/onion.json; `shrekctl onion
                  status` reads it back legibly.
```

**Milestone:** the Onion's merge decision is made by a real Shrek control-plane daemon from a sealed
policy, with structured audit and an operator query surface — and every Phase-2 trust/refusal property
still holds.

## Build economy

One **root** rebuild bakes the new machinery into the sealed image (oniond already installs to
`/usr/libexec/shrek/oniond`; add `/usr/lib/shrek/onion-policy`; rewrite `shrek-onion.service` to call
oniond; `shrekctl` already installs). Thereafter each gate is a **cheap layer-store rebuild**
(`build-layers.sh` gains a second signed sysext for O2), booted with `STORE=… scripts/boot-vm.sh` and
driven by `scripts/oniond-proof.sh`, which reads the verdict from `/run/shrek/onion.json` echoed to the
serial log. Iterate the merge/selection logic first in the faithful `systemd-sysext` container repro
(#2540) — ~1 min/cycle — and VM-confirm at the end.

## Deferred (beyond this slice, tracked)

- gatekeeperd privilege-separation broker (the real "no arbitrary root" milestone).
- Long-running oniond + socket/varlink IPC for `shrekctl`.
- Roll-back-a-bad-layer on boot-health failure (S8 tie-in).
- Version/compat metadata, `.v/` `systemd-vpick` selection, per-user activation.
- Persistent writable layer store as a product (folds in with persistent-`/var`).
