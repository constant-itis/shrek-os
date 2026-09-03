#!/usr/bin/env bash
# Build the signed shrek-installer sysext DDI for INSTALL-0.
#
# This layer carries Calamares plus live-environment tools that should appear only on installer media.
# It is staged with the desktop sysext for the live USB, not enabled as a destructive agent capability.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"
HOST_UID="$(id -u)"; HOST_GID="$(id -g)"
[ -s keys/secureboot.key ] && [ -s keys/secureboot.crt ] || {
  echo "missing keys/secureboot.{key,crt} - run scripts/build-in-container.sh once first" >&2; exit 1; }

# Vendor refind_x64.efi into the overlay before the sysext build (host egress; the build container
# has none). The installer needs it to make Apple targets bootable. Not a Packages= entry — refind's
# postinst hard-fails in the mkosi chroot. See scripts/stage-refind.sh.
scripts/stage-refind.sh

# Stage the Quickshell graphical installer (ui-installer/) into the overlay. Single source of truth is
# ui-installer/; the staged copy is a build input (gitignored), same pattern as the rEFInd vendoring above.
scripts/stage-installer-ui.sh

echo "=== building shrek-installer sysext (Calamares 3.3.14-1) in debian:trixie ==="
mkdir -p out/layers out/mkosi-vartmp
docker run --rm --privileged \
  -v "${REPO_ROOT}:/work" -w /work \
  -v "${REPO_ROOT}/out/mkosi-vartmp:/var/tmp" \
  -e HOST_UID="${HOST_UID}" -e HOST_GID="${HOST_GID}" \
  debian:trixie \
  bash -euo pipefail -c '
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq >/dev/null
    apt-get install -y --no-install-recommends -qq \
      ca-certificates openssl \
      mkosi systemd-ukify erofs-utils squashfs-tools dosfstools e2fsprogs systemd fdisk >/dev/null

    echo "=== building base tree (sealed-base runtime closure) for the overlay delta ==="
    mkosi -d debian -r trixie -t directory --output basetree --output-dir /work/out/dt-out-installer --force \
      -p systemd -p udev -p dbus-broker -p libcryptsetup12 -p systemd-resolved -p systemd-container \
      -p ca-certificates -p apparmor -p apparmor-utils -p less -p iproute2 -p nftables -p e2fsprogs \
      build

    SIGN="--verity=yes --verity-key=/work/keys/secureboot.key --verity-certificate=/work/keys/secureboot.crt"
    cd /work/layers/shrek-installer
    mkosi --force $SIGN --base-tree /work/out/dt-out-installer/basetree --overlay build
    cd /work
    rm -rf /work/out/dt-out-installer; rm -rf /var/tmp/* 2>/dev/null || true
    echo "--- built installer layer artifact ---"; ls -l out/layers/shrek-installer* 2>/dev/null || true
    chown -R "$HOST_UID:$HOST_GID" out/layers out/mkosi-vartmp 2>/dev/null || true
  '
rmdir out/mkosi-vartmp 2>/dev/null || true
echo "done. next: scripts/build-layers.sh installer assembles desktop + installer sysexts into the live store."
