# INSTALL-0 — Calamares boundary

INSTALL-0 uses Debian Trixie's packaged Calamares, not a Shrek partition editor:

- `calamares=3.3.14-1`
- `calamares-settings-debian=13.0.13-1`

Calamares owns the human UI for language, keyboard, user creation, disk selection, destructive warning,
partition workflow, summary, and the final confirmation. Shrek owns the deployment job only.

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
- Calamares job module `/usr/lib/calamares/modules/shrekdeploy`
- `/usr/libexec/shrek/shrek-install-target`

Installer media stages `shrek-installer` as a signed sysext beside the normal `shrek-desktop` DMS layer
with:

- `scripts/build-installer-layer.sh`
- `scripts/build-layers.sh installer`

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

The current `shrek-install-target` intentionally stops before writing the disk. The next step is the VM
blank-disk fixture that confirms Calamares' selected target disk is exposed reliably through global
storage, then enables the destructive writer against that device.
