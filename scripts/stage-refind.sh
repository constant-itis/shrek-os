#!/usr/bin/env bash
# Stage the rEFInd EFI loader into the shrek-installer sysext overlay.
#
# The installer needs refind_x64.efi at install time (shrek-install-target copies it onto an Apple
# target's ESP; docs/hardware-boot.md §4). We do NOT add the `refind` DEB to the sysext Packages= —
# its postinst runs `refind-install --yes` (debconf refind/install_to_esp defaults true), which needs
# a mounted ESP + efivars and hard-fails in the mkosi chroot. Instead we vendor just the binary here
# and let ExtraTrees=overlay bake it into the sealed /usr — the same pattern as stage-b43-firmware.sh.
#
# The binary is GPL and redistributable but gitignored (a build input, not source); this script is the
# reproducible recipe. build-installer-layer.sh runs it automatically before the sysext build.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

DEST="layers/shrek-installer/overlay/usr/share/refind/refind"
EFI="$DEST/refind_x64.efi"
if [ -s "$EFI" ] && [ "${FORCE:-0}" != 1 ]; then
  echo "refind already staged: $EFI ($(stat -c%s "$EFI") bytes) — FORCE=1 to re-stage"; exit 0
fi
mkdir -p "$DEST"

if [ -f /usr/share/refind/refind/refind_x64.efi ]; then
  echo "=== using host-installed refind_x64.efi ==="
  cp /usr/share/refind/refind/refind_x64.efi "$EFI"
else
  echo "=== fetching refind via apt-get download (host egress) ==="
  WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
  ( cd "$WORK" && apt-get download refind )
  deb="$(ls "$WORK"/refind_*.deb 2>/dev/null | head -1 || true)"
  [ -n "$deb" ] || { echo "apt-get download refind produced no .deb (is 'refind' in your apt sources?)" >&2; exit 1; }
  dpkg-deb -x "$deb" "$WORK/x"
  cp "$WORK/x/usr/share/refind/refind/refind_x64.efi" "$EFI"
fi

[ -s "$EFI" ] || { echo "failed to stage refind_x64.efi" >&2; exit 1; }
echo "staged: $EFI ($(stat -c%s "$EFI") bytes)"
