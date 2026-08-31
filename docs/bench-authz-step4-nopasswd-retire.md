# Bench authorization — step 4: retire the `dev` NOPASSWD placeholder

> **Status: reviewed (Fable 5 = GO-WITH-FIXES) and built.** Step 4 (final step) of the bench authorization
> slice. Steps 1+2 (socket transport) and step 3 (console consent ceremony) are shipped and green. This
> step retires the broad passwordless-root placeholder from the shipped product. All five Fable must-fixes
> are folded (see the companion-fixes + DOGFOOD-decision + residual sections below).

## What step 4 is (and the checkpoint's wrong premise, corrected)

The wake pointer (#2941) framed step 4 as *"retire/narrow NOPASSWD (only `shrek-bench-run` + proofs used
it; both now socket clients)"* — implying nothing still needs it, so it could simply be deleted. **That
premise is wrong.** Reading the tree:

- **The proofs do NOT depend on `dev` NOPASSWD.** `scripts/bench-plane-proof.sh` and
  `image/overlay/usr/lib/shrek/dogfood-persist-probe` run as **root** and drop to `dev` via
  `runuser -u dev` / `su dev` (which need no sudoers). The oracle even asserts *"dev drove `shrek bench
  destroy` over the socket (rc0, **no sudo**)"*. Correct.
- **`shrek-bench-run` is off sudo.** It is a socket client (`crates/shrek-bench-run/src/main.rs`). Correct.
- **The desktop is off `dev` sudo.** Power / suspend / `*-system` mounts / `switch-profile` go through
  **polkit** (`polkitd` in `layers/shrek-desktop/mkosi.conf`; baked `allow_active` rules in
  `image/mkosi.postinst`), not `sudo`.
- **BUT the live installer hard-depends on `dev` NOPASSWD.** `layers/shrek-installer/overlay/usr/bin/`
  `shrek-install-calamares` runs `sudo -n … /usr/bin/calamares …` (and `sudo -n sh -c '… > /dev/kmsg'`),
  and `shrek-live-welcome` runs `sudo -n … gparted` (+ the same kmsg log). The live session has **no
  polkit agent and no owner account**, so passwordless `sudo` is its only escalation path.

The installer layer is a **`Format=sysext`** (`layers/shrek-installer/mkosi.conf`) — it extends `/usr`
only and **cannot ship an `/etc/sudoers.d` file** (sysext + sealed RO `/etc`, per #2925). So the live
installer inherits its sudoers from the **sealed base `/etc`**, the same file the installed system carries.

## The clean lever: the base is already built per-variant

The base image is built three ways from one tree via `scripts/build-in-container.sh` env flags, and these
are **distinct `.raw` artifacts**:

| Variant           | Artifact (per the installer scripts)          | Role                                | Needs `dev` NOPASSWD? |
|-------------------|-----------------------------------------------|-------------------------------------|-----------------------|
| `LIVE_INSTALLER=1`| `out/shrek-installer-base.raw` (USB p2/p3)     | boots the USB, runs calamares       | **yes**               |
| `INSTALLABLE=1`   | `out/shrek-install-base.raw` (USB p8 payload) | dd'd to the target = **the product**| **no**                |
| `DOGFOOD=1`       | dogfood VM base                               | the proof of the product            | **no** (see decision) |
| plain CI          | `out/shrek_1_x86-64.raw`                       | `desktop-sealed-proof.sh` byte-clean| **no**                |

(Every variant actually *emits* the same filename `out/shrek_1_x86-64.raw`; the distinct artifact names
above exist only via a manual `cp` after each build — `docs/installer-0.md:63-69`, `build-installer-usb.sh:33`.
That is exactly why the security property must be gated on the sealed *root*, not the filename — see Proof.)

`build-installer-usb.sh` already introspects a built base's sealed root to verify its variant ("GATE A":
LIVE_INSTALLER uniquely masks `var-lib-swamp.mount` + ships no `home.mount` enable). So gating a file on
the variant is an established, testable pattern.

The fix is therefore **not** an installer refactor (calamares/gparted keep `sudo -n`; the live medium is
an ephemeral, single-purpose whole-disk-erase tool with no persistent state — broad passwordless root
there is acceptable and unavoidable pre-owner-account). The fix is: **make `dev-nopasswd` a
variant-conditional file, present ONLY in `LIVE_INSTALLER` builds.**

## Design

Mirror the existing gitignored-generated-overlay pattern (the service enable symlinks / masks that
`build-in-container.sh` writes per variant, all gitignored so the tree stays byte-clean):

1. **Un-bake the file by default.** `git rm image/overlay/etc/sudoers.d/dev-nopasswd`. Move the rule text
   to a tracked, non-baked template `image/live/sudoers.d/dev-nopasswd` (auditable source; NOT under
   `image/overlay/`, so mkosi never bakes it on its own). Header rewritten to say **LIVE INSTALLER ONLY —
   never shipped on the installed system**.
2. **Place it only for LIVE_INSTALLER.** In `build-in-container.sh`:
   - `LIVE_INSTALLER=1` branch: `install -D -m0440 image/live/sudoers.d/dev-nopasswd
     image/overlay/etc/sudoers.d/dev-nopasswd`.
   - every other branch (DOGFOOD, INSTALLABLE, plain): `rm -f image/overlay/etc/sudoers.d/dev-nopasswd`
     (idempotent; the empty `sudoers.d/` dir is harmless — `@includedir` tolerates it).
3. **Keep the tree clean.** `.gitignore` gains `/image/overlay/etc/sudoers.d/dev-nopasswd` (the generated
   artifact), exactly like the service-symlink entries.

Result: the product (`INSTALLABLE`) and its proof (`DOGFOOD`) ship a `dev` account with **no passwordless
root** — no raw-root desktop user. Legitimate admin still flows through polkit (system actions), the
gatekeeper socket + `SO_PEERCRED` gate (bench control), and the console consent ceremony
(authority-increasing bench verbs). The live installer keeps exactly what it needs.

> **Residual, NOT closed by this step (Fable finding).** `dev`'s password is **unlocked** — an sha512 of
> the public string `shrek` (`image/mkosi.postinst:24-32`), turned on in Sprint S3 because the DMS lock
> screen authenticates `dev` via `pam_unix`. So after step 4 the console/lock-screen seat the consent
> ceremony trusts still rests on a **universal public credential** until the owner-provisioning wizard
> (#2939). Step 4 removes the trivial `dev→root` escalation; it does not, and cannot, fix seat identity.
> That is the owner wizard's job.

### DOGFOOD decision — drop, with a debug-shell (resolved)

DOGFOOD **drops** `dev` NOPASSWD along with INSTALLABLE — and this is load-bearing, not just tidy: with
`dev` sudo present a caller could `sudo /usr/libexec/shrek/gatekeeperd bench grant …` and **bypass the
console consent ceremony entirely**, so a DOGFOOD image that kept it would not honestly prove that
authority cannot be silently expanded (the whole point of steps 1-3).

The cost (Fable finding): a dropped-NOPASSWD DOGFOOD VM is **adminless** — root autologin was removed
(`image/mkosi.conf:40-41`), no `RootPassword` is set, `dev` is in no admin group, no debug-shell. So the
DOGFOOD branch additionally enables systemd's **`debug-shell.service`** (root shell on tty9, reachable via
Ctrl+Alt+F9 in the graphical dogfood VM). That is an out-of-band *physical-console* facility, not a
`dev`-session escalation, so it preserves hands-on debuggability **without** reopening the consent bypass.
It is DOGFOOD-only, gitignored, and removed on every other build.

The product's "no dev passwordless root" property is asserted where it matters — on the **sealed artifact**
(see Proof) — not by the running VM's session behavior.

## Companion fixes folded in (Fable must-fixes)

- **`shrek-connect` must still work with no sudo (must-fix 1).** The name→address binding store
  `/home/.shrek-system/hosts` was seeded **root:root** and `shrek-connect` edited it via `sudo`; post-step-4
  that path is gone, which would leave **no way to wire a model provider** on the product.
  `image/overlay/usr/lib/shrek/hosts-seed` now (re)owns the store to the box owner (uid 1000) on every boot
  (upgrade-safe), and `shrek-connect` edits it directly as the owner — 0644 so root's `getaddrinfo` still
  reads it. Same authority `dev` always had, one less mechanism.
- **Gate the sealed artifact, not staging (must-fix 5).** `scripts/build-installer-payload.sh` packaged the
  product base with no variant check. It now hard-gates (loop-mount RO, mirror of GATE A): refuses to
  package a base whose sealed root still carries `/etc/sudoers.d/dev-nopasswd`, or that isn't the
  INSTALLABLE build (home.mount enabled, swamp not masked). This is the fail-closed proof of the security
  property, on the shipped bytes.
- Doc/comment truth-ups: `image/mkosi.postinst:29` and `docs/dogfood-0.md` no longer say the product's
  `dev` sudo is NOPASSWD; `docs/installer-0.md` retire note updated to "grant removed, account remains".

## Proof (host-checkable now; sealed-VM confirmation batches with the step-3 consent dogfood)

- `visudo -cf image/live-installer/sudoers.d/dev-nopasswd` — parses OK. Mode 0440 is asserted on the
  *staged* copy (git can't store 0440/root-owner on the tracked template); `install -m0440` in the build
  guarantees it.
- Variant matrix assertion (`scripts/nopasswd-variant-proof.sh`): after `build-in-container.sh` for each
  flag, `image/overlay/etc/sudoers.d/dev-nopasswd` is **present iff LIVE_INSTALLER=1**, and the DOGFOOD
  debug-shell enable is **present iff DOGFOOD=1**. Pure staging check, no image build, no root.
- `scripts/build-installer-payload.sh`'s new sealed-root gate is the product-side assertion (above).
- The sealed-VM dogfood already asserts the desktop + bench + agent flows; with DOGFOOD dropping the file,
  a green dogfood **is** the acceptance that the product works with no `dev` passwordless root. (Runs only
  on a booted sealed image — batched with the deferred step-3 BENCH-CONSENT dogfood stage, #2941 item 1.)

## Adjacent latent bugs — now FIXED (owner approved the follow-up)

Both were flagged by Fable as out-of-scope for the sudoers change and deferred; the owner approved the
follow-up in the same session step 4 shipped, so they are now closed:

1. **`var-lib-swamp.mount` mask leak — FIXED.** The LIVE_INSTALLER-only mask
   `image/overlay/etc/systemd/system/var-lib-swamp.mount` was (a) not in `.gitignore` (it sat untracked in
   the worktree) and (b) never `rm`'d in the DOGFOOD branch of `build-in-container.sh` — so a DOGFOOD build
   after a LIVE build could bake a masked swamp mount. Fixed exactly like `home.mount`: `.gitignore` now
   ignores the generated mask, and the DOGFOOD branch `rm -f`s any stale copy (LIVE_INSTALLER stages it,
   every other variant removes it). The untracked worktree artifact itself is left in place (per the
   standing "leave that file" note) — it is now gitignored, so it no longer shows as untracked, and each
   build restages it per-variant. `nopasswd-variant-proof.sh` still snapshots + restores it, non-destructive.
2. **Vestigial `ui/` `sudo -n systemctl` power calls — FIXED.** `ui/state/Menus.qml` and
   `ui/surfaces/system/SystemDrawer.qml` called `sudo -n systemctl` for reboot/power-off. The `ui/` tree is
   no longer staged into any layer (the shipped shell is DMS + shrek-menu), so this was harmless today, but
   a future `ui/` re-stage would have silently reintroduced a `dev`-passwordless-root dependency the product
   no longer satisfies. Both now call plain `systemctl` → logind (`login1.reboot`/`power-off` default
   `allow_active=yes` for the active local seat: no polkit agent, no sudo), matching the shipped shell.

## Doc refinements to fold (owner, from #2941)

Independent of the sudoers change, land these tracked clarifications in `grant-protocol.md` /
`bench-authz-consent-slice.md`:

1. **Ceremony gates configuration, not operation** — already stated (consent-slice invariant 3); keep.
2. **ATOMIC capability-manifest approval** — tracked follow-up: a *single* ceremony could approve a bundle
   (grant + network + export) for a future Workshop GUI, instead of one ceremony per verb. Note as a
   post-MVP design item, not built here.
3. **Headless authority expansion fails closed** — already stated (invariant 4: no console seat ⇒ no
   interactive escalation; needs pre-baked grants); keep, and cross-link the retired-placeholder note so
   the identity foundation (owner-account provisioning, #2939 / installer-0.md) is the single source.

## Not in scope

Owner-account provisioning (the first-boot wizard, #2939). Full retirement of the *placeholder identity*
(the baked `dev` account itself, and binding the consent subject to a real owner) awaits that wizard. Step
4 removes the **passwordless-root grant** from the product; the account remains as the dogfood-0 stand-in.
