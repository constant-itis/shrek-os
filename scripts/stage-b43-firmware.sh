#!/usr/bin/env bash
# Stage the Broadcom b43 Wi-Fi firmware into the sealed-root overlay.
#
# The 2012 MacBookPro9,2 (and other BCM43xx machines) use the in-kernel `b43` driver, whose firmware is
# proprietary Broadcom and not redistributable in Debian main — Debian's firmware-b43-installer extracts
# it at install time from the Broadcom `wl` driver blob with `b43-fwcutter`. This script does that once at
# build time and drops the firmware into image/overlay/usr/lib/firmware/b43/, which the base image bakes
# into the sealed dm-verity /usr via ExtraTrees=overlay. The firmware lives in the ROOT, not the initrd —
# Wi-Fi is never needed to boot (root is on the install disk), so b43 loads post-boot when the PCI device
# is probed and NetworkManager comes up. No network fetch on the target, ever.
#
# The extracted blobs are non-free and gitignored; this script is the reproducible recipe. It is OPT-IN:
# a plain build without running it produces a firmware-free (universal) image. Run it before the base
# build (scripts/build-in-container.sh) when targeting real Broadcom hardware.
#
# Source: broadcom-wl 5.100.138 (the version Debian's firmware-b43-installer 1:019-14 uses for the b43,
# non-legacy chips incl. BCM4331), linux/wl_apsta.o. Mirror: the minios-linux/b43-firmware GitHub release
# that current Debian points at (lwfinger.com is defunct). The blob is fetched on the host (the build
# container has no general egress) and cut inside a throwaway debian:trixie container.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

BLOB_URL="${BLOB_URL:-https://github.com/minios-linux/b43-firmware/releases/download/b43-firmware/broadcom-wl-5.100.138.tar.bz2}"
BLOB_SHA256="f1e7067aac5b62b67b8b6e4c517990277804339ac16065eb13c731ff909ae46f"
DEST="image/overlay/usr/lib/firmware/b43"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo "=== fetching broadcom-wl blob (host egress) ==="
curl -fsSL --connect-timeout 30 -o "$WORK/blob.tar.bz2" "$BLOB_URL"
got="$(sha256sum "$WORK/blob.tar.bz2" | cut -d' ' -f1)"
[ "$got" = "$BLOB_SHA256" ] || { echo "sha256 mismatch: got $got want $BLOB_SHA256" >&2; exit 1; }
echo "sha256 OK: $got"

echo "=== cutting b43 firmware in a throwaway container ==="
docker run --rm -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" -v "$WORK:/w" debian:trixie bash -euo pipefail -c '
  sed -i "s/^Components: main$/Components: main contrib non-free non-free-firmware/" /etc/apt/sources.list.d/debian.sources
  apt-get update -qq >/dev/null
  apt-get install -y --no-install-recommends b43-fwcutter bzip2 >/dev/null
  cd /w && tar xjf blob.tar.bz2
  mkdir -p out
  b43-fwcutter -w out broadcom-wl-5.100.138/linux/wl_apsta.o
  chown -R "${HOST_UID}:${HOST_GID}" out
'

echo "=== staging into $DEST ==="
rm -rf "$DEST"; mkdir -p "$DEST"
cp "$WORK/out/b43/"* "$DEST/"
echo "staged $(ls "$DEST" | wc -l) firmware files ($(du -sh "$DEST" | cut -f1)) -> $DEST"
