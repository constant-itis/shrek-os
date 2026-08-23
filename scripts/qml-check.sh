#!/usr/bin/env bash
# Fast QML iteration check for the Shrek desktop shell — DEV ONLY, not a sealed gate.
#
# The authoritative gate is scripts/desktop-smoke.sh (builds Quickshell from source with that phase's
# feature flags, headless). That recompile costs minutes per run. This script builds Quickshell ONCE
# with the FULL Slice-1 feature set into a host cache (out/qs-cache/quickshell — gitignored), then every
# run loads the real ui/shell.qml under a headless Sway using the CACHED binary: seconds, no recompile.
#
# Use it to catch QML syntax/type/import errors while iterating. Run the real desktop-smoke.sh at each
# phase boundary before committing (it validates the incremental per-phase flag set from source).
#
# Cache validity: keyed on the flag set + Quickshell tag (out/qs-cache/flags.txt). FORCE_QS=1 rebuilds.
# Quickshell statically links its QML plugins into the binary, so the self-contained executable is the
# whole cache — no plugin tree to stage (verified in Quickshell v0.3.1 CMake).
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"
HOST_UID="$(id -u)"; HOST_GID="$(id -g)"

# Fail fast on any semantic-token bypass before spending time on the headless QML load.
"$REPO_ROOT/scripts/check-tokens.sh"
pin() { grep -E "^[[:space:]]*$1[[:space:]]*=" image/supply/desktop.pins | head -1 | sed 's/#.*//' | cut -d'"' -f2; }
QS_REPO="$(pin repo)"; QS_TAG="$(pin quickshell_tag)"
CACHE=out/qs-cache
FLAGS="tag=${QS_TAG};I3;X11;TOPLEVEL;PIPEWIRE;BLUETOOTH;UPOWER;NOTIFICATIONS;STATUS_NOTIFIER;MPRIS"
BLOBS_QML=out/caelestia-blobs/qml

mkdir -p "$CACHE"
if [ "${FORCE_QS:-0}" = 1 ] || [ ! -x "$CACHE/quickshell" ] || [ "$(cat "$CACHE/flags.txt" 2>/dev/null || true)" != "$FLAGS" ]; then
  echo "=== building Quickshell cache (one-time, ~minutes): $FLAGS ==="
  docker run --rm --privileged -v "${REPO_ROOT}:/work" -w /work \
    -e QS_REPO="${QS_REPO}" -e QS_TAG="${QS_TAG}" -e CACHE="${CACHE}" \
    -e HOST_UID="${HOST_UID}" -e HOST_GID="${HOST_GID}" \
    debian:trixie bash -euo pipefail -c '
      export DEBIAN_FRONTEND=noninteractive
      apt-get update -qq >/dev/null
      apt-get install -y --no-install-recommends -qq ca-certificates openssl \
        git cmake ninja-build build-essential pkg-config \
        qt6-base-dev qt6-base-private-dev qt6-declarative-dev qt6-declarative-private-dev \
        qt6-wayland-dev qt6-wayland-private-dev qt6-shadertools-dev libwayland-dev libwayland-bin \
        wayland-protocols libcli11-dev libdrm-dev libxcb1-dev libpipewire-0.3-dev >/dev/null
      # newer wayland-protocols XMLs (quickshell references ext-background-effect staging)
      git clone --quiet https://gitlab.freedesktop.org/wayland/wayland-protocols /tmp/wp
      WP_TAG="$(cd /tmp/wp && git tag --sort=-v:refname | head -1)"; (cd /tmp/wp && git checkout --quiet "$WP_TAG")
      WP_DATADIR="$(pkg-config --variable=pkgdatadir wayland-protocols 2>/dev/null || echo /usr/share/wayland-protocols)"
      cp -r /tmp/wp/staging /tmp/wp/stable /tmp/wp/unstable "$WP_DATADIR/" 2>/dev/null || true
      git clone --quiet "$QS_REPO" /tmp/qs; cd /tmp/qs; git checkout --quiet "$QS_TAG" || true
      # FULL Slice-1 feature set so phases 2-4 all validate against one cached binary.
      cmake -S . -B build -GNinja -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr \
        -DHYPRLAND=OFF -DX11=ON -DI3=ON -DSCREENCOPY=OFF -DBLUETOOTH=ON -DNETWORK=OFF \
        -DWAYLAND_SESSION_LOCK=OFF -DWAYLAND_TOPLEVEL_MANAGEMENT=ON \
        -DSERVICE_STATUS_NOTIFIER=ON -DSERVICE_PIPEWIRE=ON -DSERVICE_MPRIS=ON -DSERVICE_PAM=OFF \
        -DSERVICE_POLKIT=OFF -DSERVICE_GREETD=OFF -DSERVICE_UPOWER=ON -DSERVICE_NOTIFICATIONS=ON \
        -DCRASH_HANDLER=OFF -DUSE_JEMALLOC=OFF >/tmp/cmake.log 2>&1 || { echo "CMAKE FAILED"; tail -40 /tmp/cmake.log; exit 1; }
      ninja -C build >/tmp/ninja.log 2>&1 || { echo "BUILD FAILED"; tail -40 /tmp/ninja.log; exit 1; }
      # qt_add_executable(quickshell) is defined in src/, so the binary lands under build/src — find it.
      QSBIN="$(find build -name quickshell -type f -perm -u+x 2>/dev/null | head -1)"
      [ -n "$QSBIN" ] || { echo "NO QS BINARY FOUND"; find build -name "quickshell*"; exit 1; }
      cp "$QSBIN" "/work/$CACHE/quickshell"
      chown "$HOST_UID:$HOST_GID" "/work/$CACHE/quickshell"
    '
  echo "$FLAGS" > "$CACHE/flags.txt"
  echo "=== cache built: $CACHE/quickshell ==="
fi

# The Caelestia Shell port uses the upstream GPLv3 Caelestia.Blobs QML module for merged shell geometry.
# Keep it built even before every surface imports it so the port foundation fails fast.
if [ ! -f "$BLOBS_QML/Caelestia/Blobs/qmldir" ] || [ "${FORCE_BLOBS:-0}" = 1 ]; then
  bash scripts/build-caelestia-blobs.sh
fi

echo "=== fast QML check (cached Quickshell, headless Sway) ==="
docker run --rm --privileged -v "${REPO_ROOT}:/work" -w /work -e CACHE="${CACHE}" -e BLOBS_QML="${BLOBS_QML}" \
  debian:trixie bash -euo pipefail -c '
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq >/dev/null
    apt-get install -y --no-install-recommends -qq \
      sway foot qt6-wayland qml6-module-qtquick qml6-module-qtquick-window qml6-module-qtquick-layouts \
      qml6-module-qtquick-shapes libqt6widgets6 libqt6dbus6 libgl1-mesa-dri libxcb1 libpipewire-0.3-0 \
      fonts-dejavu-core >/dev/null 2>&1 || echo "WARN some runtime pkgs missing"
    export XDG_RUNTIME_DIR=/run/xdgr; mkdir -p "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
    export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1
    export SWAYSOCK=/run/xdgr/sway.sock
    # start sway (its `exec quickshell` will no-op: no system quickshell installed — benign)
    sway -c /work/layers/shrek-desktop/overlay/usr/share/shrek/desktop/sway.config >/tmp/sway.log 2>&1 &
    for i in $(seq 1 30); do swaymsg -t get_version >/dev/null 2>&1 && break; sleep 1; done
    swaymsg -t get_version >/dev/null 2>&1 || { echo "QMLCHECK: FAIL (sway did not start)"; sed -n "1,30p" /tmp/sway.log; exit 1; }
    WD="$(ls "$XDG_RUNTIME_DIR"/wayland-* 2>/dev/null | grep -v "\.lock$" | head -1)"; WD="$(basename "${WD:-wayland-1}")"
    WAYLAND_DISPLAY="$WD" QT_QPA_PLATFORM=wayland QT_QUICK_BACKEND=software \
      QML_IMPORT_PATH="/work/$BLOBS_QML" QML2_IMPORT_PATH="/work/$BLOBS_QML" \
      timeout 15 "/work/$CACHE/quickshell" -p /work/ui/shell.qml >/tmp/qs.log 2>&1 || true
    WAYLAND_DISPLAY="$WD" QT_QPA_PLATFORM=wayland QT_QUICK_BACKEND=software \
      QML_IMPORT_PATH="/work/$BLOBS_QML" QML2_IMPORT_PATH="/work/$BLOBS_QML" \
      timeout 5 "/work/$CACHE/quickshell" -p /work/tests/qml/caelestia-blobs-smoke.qml >/tmp/blobs.log 2>&1 || true
    swaymsg exit >/dev/null 2>&1 || true
    echo "----- quickshell log -----"; cat /tmp/qs.log; echo "--------------------------"
    echo "----- caelestia blobs smoke log -----"; cat /tmp/blobs.log; echo "-------------------------------------"
    # Scope to QML/config LOAD failures only. Quickshell prints "Failed to load configuration" (+ "caused
    # by @file[l:c]") iff the QML tree fails to load. Runtime service warnings ("Failed to create pipewire
    # context", "Could not connect to DBus" for bluez/upower) are EXPECTED here -- this bare container runs
    # no pipewire/bluez/upower daemon -- and must NOT count as a QML failure.
    if grep -q "Failed to load configuration" /tmp/qs.log; then
      echo "QMLCHECK: FAIL (QML load error)"; grep -A6 "Failed to load configuration" /tmp/qs.log
    elif grep -q "Failed to load configuration" /tmp/blobs.log || ! grep -q "SHREK-BLOBS-SMOKE loaded" /tmp/blobs.log; then
      echo "QMLCHECK: FAIL (Caelestia.Blobs import smoke failed)"
      grep -A8 "Failed to load configuration" /tmp/blobs.log || tail -30 /tmp/blobs.log
    elif grep -q "SHREK-DESKTOP shell surfaces instantiated" /tmp/qs.log && grep -q "Configuration Loaded" /tmp/qs.log; then
      echo "QMLCHECK: PASS"
    else
      echo "QMLCHECK: NOMARKER (no load error, but surfaces/loaded markers missing)"; tail -20 /tmp/qs.log
    fi
  '
