# INSTALL-0 — Calamares boundary

INSTALL-0 uses Debian Trixie's packaged Calamares as a thin GUI shell, not a Shrek partition editor:

- `calamares=3.3.14-1`
- `calamares-settings-debian=13.0.13-1`

INSTALL-0 is a **whole-disk image writer**, so Shrek owns target-disk selection rather than deriving it
from Calamares' generic partition module. In interactive testing that module rendered an empty
storage-device dropdown on a fresh blank target (KPMCore surfaced no selectable disk), and the locale
page showed broken `* C.utf8 (C) ()` text — and neither the locale, keyboard, nor user selections are
consumed by the deployment (the sealed base image already ships the owner account; `shrek-install-target`
only needs the disk). So the Calamares interactive surface is reduced to **confirm → deploy → finished**,
and disk selection is a Shrek-owned picker:

- `/usr/libexec/shrek/shrek-list-disks` enumerates erasable whole disks, excluding the live medium, the
  layer store, the read-only payload, any disk carrying a Shrek/ESP label, read-only media, and empty
  zero-size slots. `--diagnose` dumps the raw `lsblk`/`findmnt` topology into the install log.
- `shrek-install-calamares` presents those disks in a `zenity` radiolist with an explicit erase
  confirmation, then hands the choice to the deployment job via the `SHREK_TARGET_DISK` environment
  variable. With no `zenity` present it auto-selects a sole eligible disk and otherwise refuses to guess.
- `shrekdeploy` reads `SHREK_TARGET_DISK` (falling back to the legacy Calamares `partitions` value only if
  absent) and passes it straight to `shrek-install-target`.

Calamares still owns branding, the destructive summary/confirmation page, install progress, and the
finished page. Locale/keyboard/user pages are omitted for INSTALL-0.

The authoritative Shrek layout remains the existing mkosi/systemd definitions:

- `image/mkosi.repart/00-esp.conf`
- `image/mkosi.repart/10-root.conf`
- `image/mkosi.repart/11-root-verity.conf`
- `image/mkosi.repart/20-root-empty.conf`
- `image/mkosi.repart/21-root-verity-empty.conf`
- `image/mkosi.repart/30-swamp-state.conf`
- `image/overlay/usr/lib/sysupdate.d/*.transfer`
- `image/overlay/usr/lib/systemd/system/var-lib-swamp.mount`

The installer layer adds:

- `layers/shrek-installer/mkosi.conf`
- `/usr/bin/shrek-install-calamares`
- `/usr/share/shrek/installer/calamares/settings.conf`
- `/usr/share/shrek/installer/sway-live.config`
- Calamares job module `/usr/lib/calamares/modules/shrekdeploy`
- `/usr/libexec/shrek/shrek-list-disks`
- `/usr/libexec/shrek/shrek-install-target`
- `zenity` (target-disk picker UI)

Installer media stages `shrek-installer` as a signed sysext beside the normal `shrek-desktop` DMS layer
with:

- `scripts/build-installer-layer.sh`
- `scripts/build-layers.sh installer`

`scripts/build-layers.sh` writes a mode-specific copy of the store:

- `out/layer-store-installer.raw` is the live installer store and includes `shrek-installer`.
- `out/layer-store-desktop.raw` is the installed Desktop store and omits `shrek-installer`.

The live installer and the installed-system payload intentionally use different base artifacts:

- `out/shrek_1_x86-64.raw` is the live installer boot disk when built with `LIVE_INSTALLER=1`.
  It masks the headless proof gates and installed-state mounts, and it does not require
  `/dev/disk/by-label/shrek-data` before installation.
- `out/shrek-install-base.raw` is the installed-system base, built with `INSTALLABLE=1`.
  It keeps installed persistence mounts such as `/home` enabled for the target disk layout.

`scripts/build-installer-payload.sh` packages `out/shrek-install-base.raw` plus
`out/layer-store-desktop.raw` into `out/shrek-install-payload.raw` with filesystem label
`shrek-payload`. The live installer consumes that payload read-only, copies the sealed base image to the
selected target disk, appends `shrek-layers` and `shrek-data`, copies the Desktop layer store to
`shrek-layers`, and formats `shrek-data` for persistent owner state.

Brand assets are Shrek-owned source assets under `brand/`. `scripts/stage-branding.sh` stages the minimal
installed subset under `/usr/share/shrek/branding/`:

- `shrek-os-logo.svg`
- `shrek-os-logo-256.png`
- `shrek-os-logo-512.png`
- `shrek-os-fastfetch.txt`
- `shrek-os-fastfetch-green.txt`
- `fastfetch.jsonc`
- `shrek-os-wallpaper.jpg`
- `palette.json`
- `tokens.css`
- `tokens.qml`

Calamares branding links to the Shrek logo and wallpaper from `/usr/share/shrek/branding/`. `fastfetch` is
not part of the current Desktop/Developer/Headless package set; its config and ASCII assets are staged as
the packaging seam, but installing/enabling `fastfetch` is deferred until a profile deliberately includes
it.

The current desktop wallpaper source is `brand/wallpapers/shrek-os-swamp.jpg`, generated from photographic
media with `brand/optimize-media.sh jpg`. It is staged as
`layers/shrek-desktop/overlay/usr/share/shrek/desktop/wallpaper.jpg`. Sway is the supported desktop
wallpaper seam today; DMS keeps its background transparent and reads only the future wallpaper-derived
palette file.

`shrek-install-target` refuses unexpected disk paths, refuses loop devices unless
`SHREK_INSTALL_ALLOW_LOOP=1` is set by the proof harness, verifies the payload checksums, and refuses to
overwrite the payload disk. The Shrek `zenity` picker provides the human-facing disk selection and
destructive confirmation; the picker never surfaces the payload/live/store disks in the first place.

The live installer uses an installer-only Sway config instead of including the normal desktop session. That
keeps INSTALL-0 focused on Sway + Calamares: no DMS bar, no desktop portal activation, and no unrelated
portal crash can block the installer proof. The Calamares welcome and partition pages are omitted for
INSTALL-0: the welcome page's storage preflight is partition-oriented and blocks a fresh blank whole-disk
target, and the partition page's KPMCore device list came up empty in interactive testing. Shrek-owned
disk selection (`shrek-list-disks` + the `zenity` picker) replaces both.

## Proofs

`scripts/install0-diskpick-proof.sh` is the non-GUI proof for the target-disk filter. It drives
`shrek-list-disks` against a synthesized live-VM block topology (sealed medium, layer store, read-only
payload, blank target) via `SHREK_LSBLK`/`SHREK_FINDMNT` fixture overrides and asserts that only the blank
target is offered — and that nothing is offered when no blank disk is attached. It runs anywhere; no VM or
root required.

`scripts/install0-writer-proof.sh` is the non-GUI proof for the destructive deployment path. It creates a
fresh disposable target disk under `out/`, attaches the existing `out/shrek-install-payload.raw`
read-only, runs `/usr/libexec/shrek/shrek-install-target` with `SHREK_INSTALL_ALLOW_LOOP=1`, and verifies:

- the payload checksums pass before writing;
- the target disk has appended `shrek-layers` and `shrek-data` partitions;
- the installed layer store contains `shrek-desktop` and omits `shrek-installer`.

The proof intentionally refuses to overwrite an existing target image; pass `TARGET=out/name.raw` for a
fresh run. In containerized loop-device proofs, udev may not materialize `/dev/loopXp7` and
`/dev/loopXp8` even after the kernel reports those partitions through `lsblk`. The writer has a loop-only
fallback that creates missing partition device nodes from the kernel-reported major/minor numbers. Normal
installer targets (`/dev/sd*`, `/dev/vd*`, NVMe, MMC) still use the ordinary kernel/udev path.

`scripts/install0-live-boot-proof.sh` is the GUI/live-media proof. It boots the live installer base with
`out/layer-store-installer.raw`, the read-only payload disk, and a disposable blank target disk, then
captures serial output and a screenshot. The current green proof is:

- `out/install0-live-boot-20260824e.log`
- `out/install0-live-boot-20260824e.png`
- `out/install0-live-target-20260824e.raw`

It verifies the installer sysext is merged, `graphical.target` is reached, the `dev` autologin session
opens, `shrek-install-calamares` emits its serial launch marker, no `xdg-desktop-portal` SIGTRAP occurs,
and the screenshot is captured. The matching screenshot shows Calamares on the Location page, with the bad
welcome storage warning gone.
