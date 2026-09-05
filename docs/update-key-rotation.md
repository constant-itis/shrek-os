# Update-signing key: trust root, bootstrap, and rotation

Status: DESIGN / BOOTSTRAP CONTRACT. Rotation is **not implemented** yet — this document exists so the
bootstrap path is written down *before* there are machines in the field. Read it before baking the first
image that trusts an update key, and again before you ever need to change that key.

## What the key is

`SHA256SUMS.gpg` is signed by the Shrek OS **update-signing key** — an RSA-3072 GPG key, keyid
`28143FEC30F15C8C` (fpr `E11175A5560F159B6C8149A828143FEC30F15C8C`), UID
`Shrek OS Update Signing <updates@shrekos.iambu.dev>`, **no passphrase**.

This key is the cryptographic root of every future update: a sealed client installs an A/B image only if
`SHA256SUMS.gpg` verifies against a public key baked into that client's read-only image at
`/usr/lib/systemd/import-pubring.gpg`. (The Secure-Boot-signed UKI is a *second*, independent root — it
gates what actually boots — but the update *path* is gated by this GPG key.)

Two consequences follow, and they pull in opposite directions:

- **Lose the private key → you can never ship another update the current fleet accepts.** You would have to
  physically re-image every machine, or ship a transition image over some *other* trusted channel first.
- **Leak the private key → an attacker who also controls the front (or DNS) can sign a malicious manifest.**
  The SB-signed UKI still stops a *tampered* image from booting, but a leaked update key is a serious break
  and forces a rotation.

So: protect the private key like a house key, **and** make sure rotation is possible without a truck roll.

## Where the key lives (bootstrap)

- **Private keyring:** `~/vault/shrek-os/update-signing/gnupg` — the encrypted vault, **outside** the git
  checkout. Never in the repo. `.gitignore` protects against accidental *commits*; it does **not** protect
  against filesystem loss or an over-broad tool touching the working tree, which is exactly why the private
  key does not live under `keys/` anymore.
- **Offline backup:** `~/vault/shrek-os/update-signing/backup/` holds ASCII-armored exports
  (`shrek-update-secret.asc`, `shrek-update-pub.asc`) + a README. **Copy this to offline media** (USB in a
  drawer, or paper). This is the only thing standing between "lost vault" and "re-image the fleet."
- **Public key (in repo):** `keys/shrek-update-pub.gpg` — committed, public by design. This is the file the
  image bake reads to populate `/usr/lib/systemd/import-pubring.gpg`.
- **Signing:** `scripts/publish-release.sh` reads the private keyring via `--signing-key DIR` /
  `$UPDATE_GNUPGHOME` (default: the vault path above) and **fails closed** if it is absent.

## The key insight that makes rotation cheap: the keyring is a SET

`/usr/lib/systemd/import-pubring.gpg` is a GPG keyring, not a single key. systemd-sysupdate accepts a
manifest signed by **any** key in that ring. So an image can trust **N** update keys at once. That is the
whole rotation mechanism: overlap trust across one signed-image release, then narrow it.

## Rotation procedure (when the time comes)

Preconditions: the fleet is running image `V_old` whose `import-pubring.gpg` trusts **K_old**. You want to
move to **K_new** (planned rotation, or emergency because K_old leaked).

1. **Generate K_new** into a fresh vault keyring; back it up offline (same discipline as the bootstrap
   above). Export its public half to `keys/shrek-update-pub-<newid>.gpg`.

2. **Bake a TRANSITION image `V_bridge`** whose `import-pubring.gpg` contains **both** K_old and K_new:
   ```
   gpg --no-default-keyring --keyring ./import-pubring.gpg --import \
       keys/shrek-update-pub.gpg keys/shrek-update-pub-<newid>.gpg
   ```
   Sign `V_bridge`'s manifest with **K_old** (the key the *current* fleet already trusts — this is what lets
   the fielded machines install `V_bridge` at all). Roll `V_bridge` out through the normal A/B update path.

   > This is the load-bearing step. A machine can only be *pulled* forward by a manifest signed with a key it
   > *already* trusts. If you skip the overlap and jump straight to a K_new-only image signed with K_new, the
   > current fleet rejects the manifest and is stranded. Never do that while machines are in the field.

3. **Wait for the fleet to converge** onto `V_bridge` (now every machine trusts K_new too). How you confirm
   convergence is an ops question — checkin telemetry, a required-minimum-version gate, or simply time — but
   do not proceed to step 4 until you are confident no reachable machine is still on `V_old`.

4. **Cut over signing to K_new.** From here, `publish-release.sh --signing-key <K_new vault>` signs every
   manifest with K_new. `V_bridge` machines accept it (they trust both).

5. **Bake `V_final`** whose `import-pubring.gpg` contains **only K_new** (drop K_old). Sign it with K_new.
   Roll it out. Once the fleet is on `V_final`, K_old is fully retired — and if the rotation was because
   K_old leaked, the attacker's signing capability is now dead on every updated machine.

6. **Retire K_old:** move its vault keyring + backups to an `archive/` marked with the retirement date. Do
   **not** delete immediately — a machine that missed the whole `V_old → V_bridge → V_final` train and shows
   up late may still need a K_old-signed bridge. Keep it until you are certain no such stragglers exist.

## Emergency (K_old compromised) — same procedure, different urgency

The steps are identical, but:
- Do steps 1–2 immediately; the overlap image is your only lever to reach the fleet.
- The attacker can sign manifests until each machine reaches `V_final`, but **cannot** produce a bootable
  tampered image (the SB UKI root is independent and unaffected). So the practical damage window is "can
  serve a validly-signed *old or crafted-from-signed-parts* manifest," not "can boot arbitrary code."
- Consider a required-minimum-version floor so a signed *rollback* to a pre-rotation manifest is refused.

## Do NOT (anti-patterns)

- ❌ Bake a K_new-only image and sign it with K_new while machines still trust only K_old — strands the fleet.
- ❌ Reuse the Secure-Boot key as the update key, or vice versa — two independent roots is a feature; collapsing
  them means one leak breaks both boot and update trust.
- ❌ Put the private keyring back under the checkout because "it's gitignored." Gitignore ≠ filesystem safety.
- ❌ Ship any signed-image change (new trusted key, new front URL, new egress bless) as separate rollouts —
  bake trusted-key + URL + egress bless as **one** trust-policy change (see docs/update-network.md).
