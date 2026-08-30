#!/usr/bin/env bash
# Build the signed shrek-apps sysext DDI (ADR-003 Part 1) — the everyday baseline applications (file
# manager, image/PDF/media viewers, archive manager, GUI editor, a real font set, everyday CLI utils) a
# sealed dm-verity /usr cannot `apt install` after the fact. Runs mkosi in an ephemeral --privileged
# debian:trixie container (same idiom as scripts/build-bench-layer.sh) and, because the layer carries real
# Packages=, builds it `--base-tree <sealed-base closure> --overlay` so only the genuinely-new app files
# land in the sysext (not a duplicate libc/systemd).
#
# All packages are stock trixie main (no OBS source). This script only produces out/layers/shrek-apps*.raw;
# scripts/build-layers.sh (INCLUDE_APPS=1) assembles it into the store and the sealed onion-policy
# (`enable shrek-apps`) makes oniond merge it onto /usr. The browser is a SEPARATE layer (shrek-browser).
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"
HOST_UID="$(id -u)"; HOST_GID="$(id -g)"
[ -s keys/secureboot.key ] && [ -s keys/secureboot.crt ] || {
  echo "missing keys/secureboot.{key,crt} — run scripts/build-in-container.sh once first" >&2; exit 1; }

echo "=== building shrek-apps sysext (baseline apps + fonts + CLI utils) in debian:trixie ==="
mkdir -p out/layers out/mkosi-vartmp
# overlayfs-on-docker-overlay2 mounts EINVAL; bind a host ext4 dir over /var/tmp so mkosi's overlay
# workspace lands on real ext4 (same as build-desktop-layer.sh / build-bench-layer.sh).
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

    # (1) base tree = the SEALED BASE runtime closure, so the --overlay delta contains only NEW app files
    #     (mkosi 25.3 refuses Packages= in a sysext without a base tree). Same base package set as
    #     build-bench-layer.sh / build-desktop-layer.sh so a lib already in the sealed base is not re-shipped.
    echo "=== building base tree (sealed-base runtime closure) for the overlay delta ==="
    mkosi -d debian -r trixie -t directory --output basetree --output-dir /work/out/dt-out-apps --force \
      -p systemd -p udev -p dbus-broker -p libcryptsetup12 -p systemd-resolved -p systemd-container \
      -p ca-certificates -p apparmor -p apparmor-utils -p less -p iproute2 -p nftables -p e2fsprogs \
      build

    # (2) build the signed sysext DDI as an OVERLAY on the base tree, and log the resolved closure for
    #     provenance (the # VERIFY discipline — the manifest names are candidates until the build resolves).
    SIGN="--verity=yes --verity-key=/work/keys/secureboot.key --verity-certificate=/work/keys/secureboot.crt"
    cd /work/layers/shrek-apps
    mkosi --force $SIGN --base-tree /work/out/dt-out-apps/basetree --overlay build
    cd /work
    rm -rf /work/out/dt-out-apps; rm -rf /var/tmp/* 2>/dev/null || true
    echo "--- built apps layer artifact ---"; ls -l out/layers/shrek-apps* 2>/dev/null || true
    chown -R "$HOST_UID:$HOST_GID" out/layers out/mkosi-vartmp 2>/dev/null || true
  '
rmdir out/mkosi-vartmp 2>/dev/null || true
echo "done. next: INCLUDE_APPS=1 scripts/build-layers.sh desktop assembles shrek-apps into the store,"
echo "      then the sealed onion-policy (enable shrek-apps) makes oniond merge the apps onto /usr."
