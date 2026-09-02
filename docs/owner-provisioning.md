# Owner-account provisioning (#2939) — identity foundation, lock-screen scope

Status: host-oracle-green (`scripts/owner-provision-proof.sh` 37/0), sealed-VM dogfood pending.
Zero Rust — config/shell/docs only, so no system-index bump. Design locked in mem #3011
(Fable GO-WITH-FIXES on Identity-Model-A + first-boot oneshot).

## The problem

The sealed base ships `dev` (uid 1000) with a **universal public password** (`shrek`) so the
S3 DMS lock screen has a credential to authenticate against (`image/mkosi.postinst`, `DEV_PW`).
Because that password is public, the lock screen — and by transitivity the uid-1000 session's
authority — means nothing. This slice replaces **only the `dev` credential** (and records a
display name) with an owner-chosen one on first boot, so the lock/idle re-entry gate becomes real.

It is deliberately **phase-0 / foundation**. It does NOT: make the console-consent ceremony
authenticate the owner (that was always kernel-attested physical-console presence over the
uid-1000 session — the credential guarding the session was the only fake part); defend
autologin `login -f` at boot; or encrypt `/home`. Those are named, deferred (see *Deferred*).

## Identity Model A

Keep the baked `dev`/uid-1000 slot **immutable** — the whole authority plane resolves the owner
via `dev_uid()`/`dev_gid()` parsing the sealed `/etc/passwd`, so uid 1000 must stay 1000. Make
only the **credential** (the `/etc/shadow` `dev:` hash) and the **display name** mutable. We do
not rename the account, do not touch `/etc/passwd` (GECOS stays sealed), and do not allocate a
new uid. (Rejected: (B) owner-chosen username — forces dynamic owner-uid resolution into the
frozen consent/bench core; (C) systemd-homed — allocates its own uids + manages home LUKS,
collides with `home.mount` + the baked uid 1000.)

## Mechanism

Vehicle: a **first-boot systemd oneshot** (`shrek-owner-provision.service` →
`/usr/lib/shrek/shrek-owner-provision`), mirroring `hosts-seed`: `Type=oneshot`, `After=home.mount`,
seed-once idempotent. NOT Calamares (deliberately stripped, #2785/#2787) and NOT desktop-first-run
(chicken-and-egg with the credential the desktop needs).

Delivery is a **bind-mount, not an `/etc` rewrite**: the sealed `/etc` is read-only dm-verity and
stock `passwd`/`chsh` `rename()` inside `/etc`. The provisioned shadow lives in a writable store on
`/home` and is bind-mounted over `/etc/shadow` (`nosuid,nodev`) before any auth. The baked
`/etc/shadow` is the lower/fallback. The bind-mount is done **inside the oneshot**, right after the
seed — one atomic provision step — so the `getty@tty1` login (which `Requires=` this unit on
provisioned variants) is **fail-closed**: a failed seed or mount fails the unit, `getty` refuses to
start, and the public `shrek` credential never guards a real session. (Impl choice: bind-mount
in-oneshot over a separate `.mount` unit, because a `.mount` would let the box boot on the
placeholder if the mount silently failed, and because the mount must not exist until the store is
seeded.)

## The 9 Fable must-fixes → where each lands

1. **Store dir root:root 0700, NEVER chowned to 1000** — helper `mkdir`+`chmod 0700`+`chown 0:0`
   on `/home/.shrek-identity`; file root:shadow `0640`. Unlike `hosts-seed` (which chowns its store
   to 1000 for `shrek-connect`): a uid-1000-owned dir would let uid 1000 unlink+replace the shadow
   with an empty-root-password file that then bind-mounts over `/etc/shadow` (#2982-class root).
   Proof asserts the modes + that the helper never chowns the store to a non-root owner.
2. **Bind-mount, not symlink** — `deliver()` does `mount --bind` + `remount,bind,nosuid,nodev`.
   A symlink would dangle pre-`home.mount` → `PAM_AUTHINFO_UNAVAIL`.
3. **No stock passwd/chsh** — the store is rewritten **in place** (`open`/`flock`/truncate on the
   same inode), never tmp+rename (a rename would also detach the bind-mount from the new inode).
4. **DOGFOOD non-interactive seed** — a blocking TUI deadlocks the headless oracle; DOGFOOD reads a
   baked fixed seed file (gitignored, DOGFOOD-only), selected via `owner-provision.env`.
5. **Splice only the `dev:` line** — `awk` re-credentials field 2 of the `dev` line and emits every
   other line by a bare `print` of the untouched `$0` → root/shrek/swamp/polkitd byte-preserved.
6. **Zero gatekeeperd churn** — no Rust touched; the consent/bench plane is untouched.
7. **Display name off /etc/passwd** — written to `/home/.shrek-identity/owner` (0644); `/etc/passwd`
   GECOS stays sealed; `profile.d/50-shrek-owner.sh` renders it in PS1 to hide `dev@`.
8. **INSTALLABLE blocking before desktop** — the oneshot is `Before=getty@tty1.service` and the
   getty drop-in `Requires=` it, so the interactive wizard blocks on tty1 before autologin; the
   payload gate asserts the wizard is enabled+interactive on the product.
9. **Stale comment fixed** — `getty@tty1.service.d/autologin.conf` no longer claims dev's password
   is "locked" (it is UNLOCKED — public `shrek`, or the owner's hash post-wizard).

## Variant matrix (`scripts/build-in-container.sh`, mirrors home.mount gating)

| variant         | wizard | mode            | seed baked |
|-----------------|--------|-----------------|------------|
| LIVE_INSTALLER  | off    | —               | no         |
| INSTALLABLE     | on     | interactive     | no         |
| DOGFOOD         | on     | non-interactive | yes (test) |
| plain-CI        | off    | —               | no         |

Enablement lives entirely in the `getty@tty1` drop-in (`50-owner-provision.conf`) + the
`owner-provision.env`, both generated + gitignored; the unit file + helper ship in the overlay
unconditionally (tracked). A/B sysupdate: the store on `/home/shrek-data` survives by construction;
seed-only-if-absent means a base placeholder never clobbers it.

## Proofs

- **Host oracle** `scripts/owner-provision-proof.sh` (no root, no VM): valid `$6$` crypt of the
  passphrase (salt round-trip), byte-preserved non-dev lines, store modes, no chown-to-1000,
  idempotent re-run, and the full variant matrix via the `SHREK_STAGE_ONLY` seam.
- **Sealed-VM dogfood** (pending): wizard runs before desktop; the new passphrase unlocks the DMS
  lock and the old `shrek` does NOT; the console-consent ceremony still resolves uid 1000 and
  completes a bench grant (zero regression vs #2986); the credential survives reboot + an A/B base
  swap.

## Deferred (named, not in this slice)

- Autologin `login -f` bypasses the passphrase at boot — physical-access-at-boot undefended.
- `/home` is unencrypted — needs FDE / encrypted-home keyed to the owner credential (separate slice).
- Consent ceremony does not yet demand the passphrase (owner deferred it this slice).
