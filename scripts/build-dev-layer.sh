#!/usr/bin/env bash
# Build the signed shrek-dev sysext DDI (Dogfood-0 M2, docs/dogfood-0.md §M2) — the minimum Rust + C
# toolchain to build/test the shrek-os workspace IN the box. Runs mkosi in an ephemeral --privileged
# debian:trixie container (same idiom as scripts/build-desktop-layer.sh) and, because the layer carries
# real Packages=, builds it with `--base-tree <sealed-base closure> --overlay` so only the genuinely-new
# toolchain files land in the sysext (not a duplicate libc/systemd).
#
# Unlike scripts/build-desktop-layer.sh there is NOTHING to source-build or stage — the toolchain is all
# stock trixie packages (rustc 1.85 builds the edition-2021, minimal-deps workspace; verified). This
# script only produces out/layers/shrek-dev*.raw; scripts/build-layers.sh assembles it into the store and
# the sealed onion-policy (`enable shrek-dev`) makes oniond merge it.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"
HOST_UID="$(id -u)"; HOST_GID="$(id -g)"
[ -s keys/secureboot.key ] && [ -s keys/secureboot.crt ] || {
  echo "missing keys/secureboot.{key,crt} — run scripts/build-in-container.sh once first" >&2; exit 1; }

echo "=== building shrek-dev sysext (rust + C toolchain) in debian:trixie ==="
mkdir -p out/layers out/mkosi-vartmp
# mkosi assembles the overlay via overlayfs and hardcodes its workspace under /var/tmp — which in this
# container is docker's overlay2 storage, and overlayfs-on-overlayfs mounts EINVAL. Bind-mount a host
# ext4 dir OVER /var/tmp so every mkosi workspace path lands on real ext4 (same as build-desktop-layer.sh).
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

    # (1) base tree = the SEALED BASE runtime closure, so the --overlay delta contains only NEW toolchain
    #     files (mkosi 25.3 refuses Packages= in a sysext without a base tree). Mirrors the base package
    #     set used by scripts/build-desktop-layer.sh so a toolchain lib already present in the base is not
    #     re-shipped. Output under /work/out (the repo bind mount).
    echo "=== building base tree (sealed-base runtime closure) for the overlay delta ==="
    mkosi -d debian -r trixie -t directory --output basetree --output-dir /work/out/dt-out-dev --force \
      -p systemd -p udev -p dbus-broker -p libcryptsetup12 -p systemd-resolved -p systemd-container \
      -p ca-certificates -p apparmor -p apparmor-utils -p less -p iproute2 -p nftables -p e2fsprogs \
      build

    # (2) build the signed sysext DDI as an OVERLAY on the base tree. Keys stay on the CLI (config is
    #     key/path-agnostic); --base-tree + --overlay make mkosi emit only the toolchain delta.
    SIGN="--verity=yes --verity-key=/work/keys/secureboot.key --verity-certificate=/work/keys/secureboot.crt"
    cd /work/layers/shrek-dev
    mkosi --force $SIGN --base-tree /work/out/dt-out-dev/basetree --overlay build
    cd /work
    rm -rf /work/out/dt-out-dev; rm -rf /var/tmp/* 2>/dev/null || true
    echo "--- built dev layer artifact ---"; ls -l out/layers/shrek-dev* 2>/dev/null || true
    chown -R "$HOST_UID:$HOST_GID" out/layers out/mkosi-vartmp 2>/dev/null || true
  '
rmdir out/mkosi-vartmp 2>/dev/null || true
echo "done. next: scripts/build-layers.sh desktop assembles shrek-dev into the store (if present),"
echo "      then boot — the sealed onion-policy enables it and oniond merges the toolchain onto /usr."
