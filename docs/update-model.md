# Shrek OS — Update model

> An update that can't be un-done is a loaded gun. Shrek's rule: a bad update is *survivable*,
> an old update is *refused*, and the two are not the same mechanism.

This document fixes how Shrek is updated: the transport, the A/B disk model, and — the
load-bearing part — the split between **availability rollback** (recover from *your own* broken
update) and **security anti-rollback** (refuse an *attacker's* old-but-valid image). It records
the transport decision as it actually resolved when the Phase-1 spike ran, not as originally
planned.

It is a reconciliation doc. Where [`base-selection.md`](base-selection.md) §"Update transport"
left a **fork** (bootc primary / sysupdate fallback, *pending the spike*), the spike ran and the
fork resolved. This doc records the resolution and **supersedes** the "PRIMARY: bootc" framing
there. The proof is in [`phase1-s7-sysupdate.md`](phase1-s7-sysupdate.md) (one A/B update, end to
end) and [`phase1-s8-rollback.md`](phase1-s8-rollback.md) (a broken update auto-reverts); the
*security* half lives in [`security-model.md`](security-model.md) §8. This doc is the model those
three describe in pieces.

The invariant it exists to protect:

```
A failed or rolled-back update NEVER widens authority, and NEVER lands the system on an image
below the security floor. Recovery moves toward last-good, which is ≥ the anti-rollback SVN —
so availability recovery can never resurrect a version the security plane has retired.
```

---

## 1. Scope & non-goals

**This document owns the update model:** the transport choice and why, the A/B partition model,
the update flow, and the two distinct safety planes (availability rollback vs security
anti-rollback) and how they interact.

**It does not re-derive the spike.** The exact disk layout, the `mkosi` settings, the
`sysupdate` transfer files, and the verified boot traces are in
[`phase1-s7-sysupdate.md`](phase1-s7-sysupdate.md) and
[`phase1-s8-rollback.md`](phase1-s8-rollback.md). This doc is the model above those two records.

**It does not own the security primitives.** The TPM NV monotonic counter, the SVN floor, the
sealed-policy plane, and their live-verify caveat are [`security-model.md`](security-model.md)
§4/§8. This doc references them and states the *interaction* with rollback; it does not define
them.

## 2. Transport — `systemd-sysupdate` raw A/B, not bootc (the fork, resolved)

[`base-selection.md`](base-selection.md) chose a Debian base with a deliberately *undecided*
update transport: **bootc primary, `systemd-sysupdate` fallback, to be settled by the Phase-1
spike.** The spike settled it.

```
Reconnaissance (debian:trixie, at S7):
  bootc / composefs / bootupd   → NOT PACKAGED on Debian trixie
  ostree                        → 2025.2 (the layer only, not the bootc engine)

bootc on Debian = source-building bootc AND composefs (both unpackaged) — the fragile path.
Per the PRE-AUTHORIZED decision tree this is the "janky → fall back" branch.

RESOLVED:  stay on Debian · transport = systemd-sysupdate raw-partition A/B.
           bootc/composefs deferred to a later upgrade (base-selection permits it).
```

This is not a downgrade of ambition — the whole A/B + rollback stack is **native to trixie
(systemd 257)**, so Shrek owns its updater with zero out-of-distro dependencies:

| Piece | Role | Debian package |
|-------|------|----------------|
| `systemd-repart` | lays out the A/B partition slots | `systemd-repart` |
| `systemd-sysupdate` | the update engine (`list` / `update`) | `systemd-container` |
| `systemd-boot` | boots newest UKI + boot assessment | `systemd-boot` |
| `mkosi` 25.3 | versioned build, split artifacts, transfer defs | `mkosi` |

**⇒ SUPERSEDES** the "PRIMARY: bootc" line in [`base-selection.md`](base-selection.md)
§"Update transport — decided fork": the primary is now `systemd-sysupdate`; bootc is a deferred
upgrade path (§8), not the shipping transport.

## 3. The A/B disk model

One image is delivered as a **fixed-size raw partition set**, two slots (A/B), a shared ESP, and
a volatile `/var`. The seal is held at S6-proven **whole-root dm-verity** — the update mechanism
was proven by changing *only* the transport variable, not the sealing method.

```
#  GPT type       label            role
1  esp            esp              systemd-boot + BOTH A/B UKIs (shared, boot-counted)
2  root-x86-64    shrek_<v>        dm-verity DATA, slot A
3  root-verity    shrek_<v>_verity dm-verity HASH, slot A
4  root-x86-64    _empty           slot B — sysupdate installs the next version here
5  root-verity    _empty           slot B hash
                                   (no /var partition — /var is a volatile tmpfs this cycle)
```

- **Slots are fixed-size and identical**, so an update writes the *inactive* slot while the
  active slot keeps running — an update is never in-place on the running root.
- **The integrity anchor is the signed UKI.** The verity roothash is injected into the UKI
  kernel cmdline; the UKI is Secure-Boot-signed with the Shrek key. So the update's integrity
  rides the boot signature — there is no separate verity-signature partition this cycle
  (deferred, §8). An update whose UKI is not Shrek-signed does not boot under enforcing Secure
  Boot; an update whose root does not match its roothash fails `systemd-veritysetup`.
- **Version identity is in the label.** `Label=%M_%A` → `shrek_<version>`; `sysupdate`'s
  `MatchPattern` extracts and relabels the version from it. `_empty` + `NoAuto=1` marks the
  reserved inactive slot so the auto-generator never mounts it.
- **`/var` is volatile this cycle** (`systemd.volatile=state`), the minimal writable-state fix
  for the sealed root. Persistent `/var` + writable `/etc` (the particleos sealed-`/usr` model)
  is the deferred long-term shape (§8). This matters to the update model only in what survives a
  reboot (§6).

## 4. The update flow

`systemd-sysupdate` imports the next version's root, verity, and UKI into the inactive slot and
the ESP, boot-counted, without touching the running slot.

```
systemd-sysupdate update <next>
  ├─ import root  → inactive root slot     (ProtectVersion=%A never overwrites the running slot)
  ├─ import verity→ inactive verity slot
  ├─ install UKI  → ESP, BOOT-COUNTED       shrek_<next>_x86-64+3-0.efi   (TriesLeft=3)
  └─ InstancesMax=2                          keep exactly A + B; older instances pruned
```

- **The OS carries its own update policy.** The three transfer definitions ship *inside the
  image* at `/usr/lib/sysupdate.d/` (root / verity / UKI); `systemd-sysupdate --image` resolves
  them relative to the dissected image root. The version *pool* is external (a `--transfer-source`
  directory now; a `Type=url-file` network source is the productization step, §8).
- **Offline application is the same operation as on-appliance.** The spike applies updates
  offline against a disk copy (`scripts/update-in-container.sh`) so artifacts stay untouched — but
  it is the identical `systemd-sysupdate update` an appliance runs against its live disk.
- **The new UKI is installed boot-counted; the known-good v1 UKI is not.** That asymmetry is what
  makes §5's rollback land somewhere safe.

## 5. Two safety planes — rollback vs anti-rollback (do not conflate)

This is the spine. Two different failures need two different, independent mechanisms. Reversing
or merging them is the classic update-system bug.

```
AVAILABILITY ROLLBACK        "MY update is broken."          → recover to last-good
  threat: a bad build, a failed health check, a hang/panic before boot-complete.
  mechanism: systemd-boot boot-counting + a health gate (S8). Fails toward the previous version.

SECURITY ANTI-ROLLBACK       "An ATTACKER offers an OLD valid update."   → refuse it
  threat: an adversary feeds a genuinely-signed but known-vulnerable OLD image to re-open a
          patched hole (a downgrade attack).
  mechanism: a monotonic SVN floor in a TPM NV index (security-model.md §8). Refuses any image
             below the floor. This is NOT boot-counting and does NOT fail toward "older."
```

### 5a. Availability rollback (boot-counting + health gate) — S8

Native systemd Automatic Boot Assessment, no bespoke engine:

1. **Boot counting (loader).** A new UKI carries a `+tries-left-tries-done` counter in its ESP
   filename; `systemd-boot` decrements it each launch and orders `+0-N` (exhausted) last. The
   **v1 UKI is written without a counter → permanent known-good**, so it is where the loader
   lands when the new version is marked bad.
2. **Blessing (success marker).** `systemd-bless-boot` strips the counter — making an update
   permanent — but only `After=boot-complete.target` is reached.
3. **Health gate (Shrek's).** `shrek-boot-health.service` is `Before=boot-complete.target` +
   `RequiredBy=` it (baked as a `.requires` symlink in sealed `/usr`, since `/etc` is read-only).
   A non-zero exit fails the target's job → bless never runs → the counter is never stripped;
   `FailureAction=reboot` reboots so the loader decrements again. After the tries exhaust, the
   loader falls back to v1. Verified end to end in [`phase1-s8-rollback.md`](phase1-s8-rollback.md).

In production the health check is where Shrek's real greenboot-style validation lives
(control-plane liveness, verity intact, reachability). The gate runs *only on counted boots*, so
the good fallback boots straight through.

### 5b. Security anti-rollback (the SVN floor) — owned by security-model §8

A monotonic **security version number** in a TPM NV index. Boot refuses any image below the
floor. The floor advances **only on a greenboot-healthy commit** — never merely because a
version was *installed*. Recovery/rollback always targets a version **≥ the floor**. Full
definition, and the live-verify-on-target-TPM caveat, in
[`security-model.md`](security-model.md) §8 and [`base-selection.md`](base-selection.md).

### 5c. Why they don't fight — the load-bearing interaction

The two planes could contradict — availability wants to go *back*, security forbids going *too
far back*. They are reconciled by one rule: **the SVN floor advances only on healthy commit.**

```
Broken v2 never reaches boot-complete → never blessed → SVN floor STAYS at v1's level.
So rolling back to v1 is always ≥ the floor: the anti-rollback plane permits it.

An attacker's OLD image (< floor) is refused REGARDLESS of boot-counting — anti-rollback is
checked before the availability machinery ever runs.
```

So availability rollback can never brick the box (it targets last-good, which is by construction
≥ the floor, because the floor only moved when a version proved healthy), and anti-rollback can
never be tricked by dressing a downgrade up as a "recovery." The rollback in S8 and the SVN in
security-model are the *same story told on two planes*, joined at "healthy commit advances the
floor."

## 6. Policy and state across an update

An update is atomic with respect to the sealed plane and non-destructive to the mutable plane:

- **Sealed policy travels with the image.** Static Shrek policy (the swampd allow-set template,
  the never-indexable exclusions, the sealed enable-list, agent cap templates) is baked into the
  image under the dm-verity root ([`architecture.md`](architecture.md) §1/§3,
  [`security-model.md`](security-model.md) §4). So an update updates policy and code **together,
  atomically, under one signature** — there is no window where new code runs under old policy or
  vice versa, and no writable policy file to skew.
- **Mutable grants are a separate plane and survive the update.** Per-machine grants live on the
  fs-verity + TPM-NV-counter plane ([`security-model.md`](security-model.md) §4/§5,
  [`grant-protocol.md`](grant-protocol.md)), not in the image, so an update does not wipe them —
  and a rollback does not resurrect a revoked one (the NV counter forbids it).
- **Volatile `/var` this cycle.** With `systemd.volatile=state`, `/var` is fresh each boot, so no
  runtime state persists across a reboot/update in the spike. Persistent, machine-bound `/var` is
  the deferred particleos refinement (§8); until then, "what must survive an update" is exactly
  "what is on the mutable-grants plane," nothing incidental.

## 7. Failure behavior — the update path is on the availability plane

Consistent with the two-plane model ([`architecture.md`](architecture.md) §9,
[`security-model.md`](security-model.md) §7):

```
The UPDATE machinery is availability-plane. If sysupdate fails, is absent, or an update never
arrives, the running slot keeps running — the box is fully usable. A failed update fails toward
"stay on the current good version," never toward a half-written or unsigned root.

But a failed update NEVER fails toward MORE authority: the fallback is the previous SEALED image,
which is itself signed, verity-checked, and ≥ the SVN floor. Degrading the update never degrades
the wall.
```

An update is only *committed* (blessed, counter stripped, floor advanced) after it proves healthy
on a real boot. Until then it is provisional and reversible. There is no state in which a
not-yet-proven update has already retired its predecessor.

## 8. Deferred

- **composefs / bootc upgrade path.** The transport is `systemd-sysupdate` today because bootc +
  composefs are unpackaged on Debian trixie (§2). When they land (or are worth a source build),
  composefs-backed integrity and bootc's OCI transport are the upgrade — `base-selection.md`
  keeps this path open. Not a re-decision, an enhancement.
- **particleos sealed-`/usr` + persistent writable-`/`.** Replaces `systemd.volatile=state` with a
  persistent, machine-bound `/var` and a reconstructed stateless `/etc`. Drags in TPM2-LUKS,
  verity-signature keys, and first-boot provisioning — a large surface orthogonal to proving A/B,
  so deferred until read-only `/etc` / ephemeral `/var` proves limiting (§6).
- **Verity-signature partition.** Integrity currently rides the signed UKI's roothash cmdline
  (§3). A dedicated verity-sig partition (independent roothash signature) is the fuller form,
  deferred with the particleos model.
- **On-appliance network self-update.** The flow is proven offline against a disk copy (§4); the
  same transfers with a `Type=url-file` `[Source]` and a signed version index is the network
  productization step.
- **Staged / phased rollout + delta updates.** Whole-slot replacement is the v1 model. Phased
  rollout (canary rings) and binary-delta transfers are later optimizations; they must preserve
  §5's two-plane guarantee unchanged.
- **Generic health checks.** The spike's health gate is a single deterministic marker; the
  product gate is greenboot-style (`systemd-boot-check-no-failures.service` + Shrek control-plane
  liveness), noted in [`phase1-s8-rollback.md`](phase1-s8-rollback.md).

Every deferral upgrades a mechanism *inside* the two-plane model of §5 — none of them is
permitted to merge the rollback and anti-rollback planes, or to let a provisional update retire
its predecessor before a healthy commit.
