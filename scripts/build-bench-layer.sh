#!/usr/bin/env bash
# Build the signed shrek-bench sysext DDI (Bench-0, ADR-003 Part 2) — the rootless-container runtime
# (podman/crun/conmon/fuse-overlayfs/uidmap/catatonit) for the Bench plane. Runs mkosi in an ephemeral
# --privileged debian:trixie container (same idiom as scripts/build-dev-layer.sh) and, because the layer
# carries real Packages=, builds it with `--base-tree <sealed-base closure> --overlay` so only the
# genuinely-new runtime files land in the sysext (not a duplicate libc/systemd).
#
# All packages are stock trixie main (verified in scratchpad/bench0-prevalidate.sh). This script only
# produces out/layers/shrek-bench*.raw; scripts/build-layers.sh (INCLUDE_BENCH=1) assembles it into the
# store and the sealed onion-policy (`enable shrek-bench`) makes oniond merge it. The subuid/subgid RANGE
# for dev is baked separately into the sealed /etc by image/mkosi.postinst (a sysext cannot touch /etc).
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"
HOST_UID="$(id -u)"; HOST_GID="$(id -g)"
[ -s keys/secureboot.key ] && [ -s keys/secureboot.crt ] || {
  echo "missing keys/secureboot.{key,crt} — run scripts/build-in-container.sh once first" >&2; exit 1; }

echo "=== building shrek-bench sysext (rootless podman runtime) in debian:trixie ==="
mkdir -p out/layers out/mkosi-vartmp
# overlayfs-on-docker-overlay2 mounts EINVAL; bind a host ext4 dir over /var/tmp so mkosi's overlay
# workspace lands on real ext4 (same as build-desktop-layer.sh / build-dev-layer.sh).
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

    # (1) base tree = the SEALED BASE runtime closure, so the --overlay delta contains only NEW runtime
    #     files (mkosi 25.3 refuses Packages= in a sysext without a base tree). Mirrors the base package
    #     set used by build-dev-layer.sh so a lib already present in the sealed base is not re-shipped.
    echo "=== building base tree (sealed-base runtime closure) for the overlay delta ==="
    mkosi -d debian -r trixie -t directory --output basetree --output-dir /work/out/dt-out-bench --force \
      -p systemd -p udev -p dbus-broker -p libcryptsetup12 -p systemd-resolved -p systemd-container \
      -p ca-certificates -p apparmor -p apparmor-utils -p less -p iproute2 -p nftables -p e2fsprogs \
      build

    # (2) build the signed sysext DDI as an OVERLAY on the base tree. Keys stay on the CLI (config is
    #     key/path-agnostic); --base-tree + --overlay make mkosi emit only the runtime delta.
    SIGN="--verity=yes --verity-key=/work/keys/secureboot.key --verity-certificate=/work/keys/secureboot.crt"
    cd /work/layers/shrek-bench
    mkosi --force $SIGN --base-tree /work/out/dt-out-bench/basetree --overlay build
    cd /work
    rm -rf /work/out/dt-out-bench; rm -rf /var/tmp/* 2>/dev/null || true
    echo "--- built bench layer artifact ---"; ls -l out/layers/shrek-bench* 2>/dev/null || true
    chown -R "$HOST_UID:$HOST_GID" out/layers out/mkosi-vartmp 2>/dev/null || true
  '
rmdir out/mkosi-vartmp 2>/dev/null || true
echo "done. next: INCLUDE_BENCH=1 scripts/build-layers.sh desktop assembles shrek-bench into the store,"
echo "      then the sealed onion-policy (enable shrek-bench) makes oniond merge the runtime onto /usr."
