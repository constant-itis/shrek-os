#!/usr/bin/env bash
# Build the signed shrek-desktop sysext DDI (the SEALED delivery path for Desktop Bootstrap 0,
# docs/desktop-bootstrap-0.md §Delivery). Runs mkosi in an ephemeral --privileged debian:trixie
# container (same idiom as scripts/build-layers.sh) and:
#   1. stages the ui/ QML tree into the layer overlay  -> /usr/share/shrek/ui
#   2. builds Quickshell from the pinned tag and stages the binary + its QML plugins into the overlay
#   3. runs `mkosi build` for layers/shrek-desktop (installs the trixie runtime packages + copies the
#      overlay), signing the verity roothash with the throwaway Shrek key (as build-layers.sh does).
#
# The staged overlay artifacts (ui/, quickshell binary/plugins) are EPHEMERAL and gitignored — ui/ is
# the source of record; the binary is a build product. This script is the integration point the base
# scripts/build-layers.sh can later call; kept separate for Bootstrap 0 so it cannot destabilise the
# existing L1-L4 layer gates.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"
HOST_UID="$(id -u)"; HOST_GID="$(id -g)"
[ -s keys/secureboot.key ] && [ -s keys/secureboot.crt ] || {
  echo "missing keys/secureboot.{key,crt} — run scripts/build-in-container.sh once first" >&2; exit 1; }

pin() { grep -E "^[[:space:]]*$1[[:space:]]*=" image/supply/desktop.pins | head -1 | sed 's/#.*//' | cut -d'"' -f2; }
QS_REPO="$(pin repo)"
QS_TAG="$(pin quickshell_tag)"
OVL=layers/shrek-desktop/overlay

echo "=== building shrek-desktop sysext (quickshell ${QS_TAG}) in debian:trixie ==="
mkdir -p out/layers
docker run --rm --privileged \
  -v "${REPO_ROOT}:/work" -w /work \
  -e HOST_UID="${HOST_UID}" -e HOST_GID="${HOST_GID}" \
  -e QS_REPO="${QS_REPO}" -e QS_TAG="${QS_TAG}" -e OVL="${OVL}" \
  debian:trixie \
  bash -euo pipefail -c '
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq >/dev/null
    apt-get install -y --no-install-recommends -qq \
      ca-certificates openssl \
      mkosi systemd-ukify erofs-utils squashfs-tools dosfstools e2fsprogs systemd fdisk \
      git cmake ninja-build build-essential pkg-config \
      qt6-base-dev qt6-declarative-dev qt6-declarative-private-dev qt6-wayland-dev libwayland-dev >/dev/null

    # (1) stage the QML source tree
    rm -rf "/work/$OVL/usr/share/shrek/ui"
    mkdir -p "/work/$OVL/usr/share/shrek/ui"
    cp -a /work/ui/. "/work/$OVL/usr/share/shrek/ui/"

    # (2) build + stage Quickshell (unpackaged in Debian). Staged into the overlay so mkosi copies it
    #     into the sysext /usr; its Qt6 runtime is satisfied by the layer packages.  # VERIFY: the exact
    #     install prefix + QML plugin dir land under /usr (DESTDIR stage below), matching the runtime.
    git clone --quiet "$QS_REPO" /tmp/qs; cd /tmp/qs
    [ "$QS_TAG" = "AUTO-FIRST-BUILD" ] && QS_TAG="$(git tag --sort=-v:refname | head -1)" && \
      echo "SHREK-QS-RESOLVED-TAG $QS_TAG (record into image/supply/desktop.pins)"
    git checkout --quiet "$QS_TAG" || true
    cmake -S . -B build -GNinja -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr \
      -DHYPRLAND=OFF -DSERVICE_STATUS_NOTIFIER=OFF -DSERVICE_PIPEWIRE=OFF -DSERVICE_MPRIS=OFF -DCRASH_REPORTER=OFF
    ninja -C build
    DESTDIR="/work/$OVL" ninja -C build install
    cd /work

    # (3) build the signed sysext DDI
    SIGN="--verity=yes --verity-key=/work/keys/secureboot.key --verity-certificate=/work/keys/secureboot.crt"
    cd /work/layers/shrek-desktop
    mkosi --force $SIGN build
    cd /work
    echo "--- built desktop layer artifact ---"; ls -l out/layers/shrek-desktop* 2>/dev/null || true
    chown -R "$HOST_UID:$HOST_GID" out/layers "/work/$OVL" 2>/dev/null || true
  '
echo "done. next: merge shrek-desktop in the KVM gate (scripts/boot-vm.sh) — the sealed-boot step."
