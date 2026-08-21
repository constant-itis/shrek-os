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
mkdir -p out/layers out/mkosi-vartmp
# mkosi assembles sysext/overlay images via overlayfs and hardcodes its workspace under /var/tmp — which
# in this container is docker's overlay2 storage, and overlayfs-on-overlayfs mounts EINVAL. Bind-mount a
# host ext4 dir OVER /var/tmp so every mkosi workspace path (base-tree build AND the --overlay delta)
# lands on real ext4. (The base image build sidesteps this by using disk/systemd-repart, not overlay.)
docker run --rm --privileged \
  -v "${REPO_ROOT}:/work" -w /work \
  -v "${REPO_ROOT}/out/mkosi-vartmp:/var/tmp" \
  -e HOST_UID="${HOST_UID}" -e HOST_GID="${HOST_GID}" \
  -e QS_REPO="${QS_REPO}" -e QS_TAG="${QS_TAG}" -e OVL="${OVL}" -e FORCE_QS="${FORCE_QS:-0}" \
  debian:trixie \
  bash -euo pipefail -c '
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq >/dev/null
    apt-get install -y --no-install-recommends -qq \
      ca-certificates openssl \
      mkosi systemd-ukify erofs-utils squashfs-tools dosfstools e2fsprogs systemd fdisk \
      git cmake ninja-build build-essential pkg-config \
      qt6-base-dev qt6-base-private-dev qt6-declarative-dev qt6-declarative-private-dev \
      qt6-wayland-dev qt6-wayland-private-dev qt6-shadertools-dev \
      libwayland-dev libwayland-bin wayland-protocols libcli11-dev libdrm-dev >/dev/null

    # (1) stage the QML source tree
    rm -rf "/work/$OVL/usr/share/shrek/ui"
    mkdir -p "/work/$OVL/usr/share/shrek/ui"
    cp -a /work/ui/. "/work/$OVL/usr/share/shrek/ui/"

    # (2) build + stage Quickshell (unpackaged in Debian). Staged into the overlay so mkosi copies it
    #     into the sysext /usr; its Qt6 runtime is satisfied by the layer packages.  # VERIFY: the exact
    #     install prefix + QML plugin dir land under /usr (DESTDIR stage below), matching the runtime.
    # Overlay newer wayland-protocols XMLs: quickshell references ext-background-effect staging, newer
    # than trixie ships; only the XML files are needed (the >=1.41 pkg-config check passes on trixie).
    # Reuse an already-staged Quickshell binary built from the same pinned tag on a prior run, so
    # iterating on the LAYER PACKAGING does not recompile Quickshell each time. FORCE_QS=1 forces a
    # clean source rebuild. NOTE no apostrophes in this container block — a single quote would close
    # the bash -c string.
    if [ "${FORCE_QS:-0}" != 1 ] && [ -x "/work/$OVL/usr/bin/quickshell" ]; then
      echo "SHREK-QS-REUSE: staged /work/$OVL/usr/bin/quickshell present — skipping source build (FORCE_QS=1 to rebuild)"
    else
      git clone --quiet https://gitlab.freedesktop.org/wayland/wayland-protocols /tmp/wp
      WP_TAG="$(cd /tmp/wp && git tag --sort=-v:refname | head -1)"; echo "SHREK-WP-TAG $WP_TAG"
      ( cd /tmp/wp && git checkout --quiet "$WP_TAG" )
      WP_DATADIR="$(pkg-config --variable=pkgdatadir wayland-protocols 2>/dev/null || echo /usr/share/wayland-protocols)"
      cp -r /tmp/wp/staging /tmp/wp/stable /tmp/wp/unstable "$WP_DATADIR/" 2>/dev/null || true

      git clone --quiet "$QS_REPO" /tmp/qs; cd /tmp/qs
      [ "$QS_TAG" = "AUTO-FIRST-BUILD" ] && QS_TAG="$(git tag --sort=-v:refname | head -1)" && \
        echo "SHREK-QS-RESOLVED-TAG $QS_TAG (record into image/supply/desktop.pins)"
      git checkout --quiet "$QS_TAG" || true
      # Feature set verified green by scripts/desktop-smoke.sh: keep WAYLAND + WLR layer-shell (the
      # PanelWindow surfaces); disable the rest to shrink the dep closure.
      cmake -S . -B build -GNinja -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr \
        -DHYPRLAND=OFF -DX11=OFF -DI3=OFF -DSCREENCOPY=OFF -DBLUETOOTH=OFF -DNETWORK=OFF \
        -DWAYLAND_SESSION_LOCK=OFF -DWAYLAND_TOPLEVEL_MANAGEMENT=OFF \
        -DSERVICE_STATUS_NOTIFIER=OFF -DSERVICE_PIPEWIRE=OFF -DSERVICE_MPRIS=OFF -DSERVICE_PAM=OFF \
        -DSERVICE_POLKIT=OFF -DSERVICE_GREETD=OFF -DSERVICE_UPOWER=OFF -DSERVICE_NOTIFICATIONS=OFF \
        -DCRASH_HANDLER=OFF -DUSE_JEMALLOC=OFF
      ninja -C build
      DESTDIR="/work/$OVL" ninja -C build install
    fi
    cd /work

    # (3a) base tree for the overlay build. mkosi 25.3 REFUSES to install Packages= into a sysext
    #      without a base tree ("Cannot install packages in extension images without a base tree") —
    #      an extension is a DELTA, so the package manager needs to know what is already present. We
    #      build a throwaway directory rootfs carrying the SEALED BASE runtime closure (mirrors
    #      image/mkosi.conf, minus the kernel/EFI stub the desktop layer never pulls). The layer is then
    #      built with --overlay against it, so only the genuinely-new desktop files (Sway/Qt6/Quickshell
    #      + their deps not already in the base) land in the sysext — not a duplicate libc/systemd.
    # mkosi assembles both the base tree and the --overlay delta via overlayfs under /var/tmp; the outer
    # docker run bind-mounts a host ext4 dir over /var/tmp so those overlay mounts land on real ext4
    # (overlayfs-on-overlay2 EINVALs). Output dirs stay under /work/out (the repo bind mount).
    echo "=== building base tree (sealed-base runtime closure) for the overlay delta ==="
    mkosi -d debian -r trixie -t directory --output basetree --output-dir /work/out/dt-out --force \
      -p systemd -p udev -p dbus-broker -p libcryptsetup12 -p systemd-resolved -p systemd-container \
      -p ca-certificates -p apparmor -p apparmor-utils -p less -p iproute2 -p nftables -p e2fsprogs \
      build

    # (3b) build the signed sysext DDI as an OVERLAY on the base tree. Keys stay on the CLI (config is
    #      key/path-agnostic); --base-tree + --overlay make mkosi emit only the desktop delta.
    SIGN="--verity=yes --verity-key=/work/keys/secureboot.key --verity-certificate=/work/keys/secureboot.crt"
    cd /work/layers/shrek-desktop
    mkosi --force $SIGN --base-tree /work/out/dt-out/basetree --overlay build
    cd /work
    # Throwaway base tree + the /var/tmp overlay workspace (gitignored under out/); free the disk once
    # the DDI is signed, and chown so the host can clean the bind-mounted /var/tmp dir.
    rm -rf /work/out/dt-out; rm -rf /var/tmp/* 2>/dev/null || true
    echo "--- built desktop layer artifact ---"; ls -l out/layers/shrek-desktop* 2>/dev/null || true
    chown -R "$HOST_UID:$HOST_GID" out/layers out/mkosi-vartmp "/work/$OVL" 2>/dev/null || true
  '
rmdir out/mkosi-vartmp 2>/dev/null || true
echo "done. next: merge shrek-desktop in the KVM gate (scripts/boot-vm.sh) — the sealed-boot step."
