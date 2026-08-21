#!/usr/bin/env bash
# Desktop Bootstrap-0 smoke — prove the real desktop runtime works headless (docs/desktop-bootstrap-0.md
# §Smoke). Runs the ACTUAL layer stack (Sway + Qt6 QML + source-built Quickshell) in an ephemeral
# --privileged debian:trixie container (same idiom as scripts/build-layers.sh), fully software/GPU-free.
#
# Emits SHREK_GATE-style lines and a final tally. Stages (each independent so we learn WHERE it breaks):
#   DB0-qt-sw   Qt6 QtQuick renders under the software backend (foundation, no Quickshell yet)
#   DB0-sway    Sway starts on the headless backend (swaymsg -t get_version)
#   DB0-qs-load Quickshell loads Shell.qml with no QML error
#   DB0-surfaces shell surfaces instantiate (Shell.qml load marker logged)
#   DB0-logout  swaymsg exit -> rc 0
#
# This container smoke DECOUPLES "does the desktop stack work" from "does it merge into the sealed
# image" (the latter = scripts/build-desktop-layer.sh + the KVM gate, the NEXT step).
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

QS_REPO="$(sed -n 's/^\s*repo\s*=\s*"\(.*\)"/\1/p' image/supply/desktop.pins | head -1)"
QS_TAG="$(sed -n 's/^\s*quickshell_tag\s*=\s*"\(.*\)"/\1/p' image/supply/desktop.pins | head -1)"

echo "=== Desktop Bootstrap-0 smoke (repo=${QS_REPO} tag=${QS_TAG}) in debian:trixie ==="
docker run --rm --privileged \
  -v "${REPO_ROOT}:/work" -w /work \
  -e QS_REPO="${QS_REPO}" -e QS_TAG="${QS_TAG}" \
  debian:trixie \
  bash -euo pipefail -c '
    export DEBIAN_FRONTEND=noninteractive
    PASS=0; FAIL=0
    gate() { if [ "$1" = ok ]; then echo "SHREK_GATE: PASS $2"; PASS=$((PASS+1)); else echo "SHREK_GATE: FAIL $2"; FAIL=$((FAIL+1)); fi; }

    echo "--- apt: runtime + build deps ---"
    apt-get update -qq >/dev/null
    apt-get install -y --no-install-recommends -qq \
      sway foot \
      qt6-wayland qml6-module-qtquick qml6-module-qtquick-window qml6-module-qtquick-layouts \
      qml6-module-qtquick-shapes qml6-module-qtquick-controls qml6-module-qtquick-templates \
      libgl1-mesa-dri libglx-mesa0 fonts-dejavu-core \
      git cmake ninja-build build-essential pkg-config \
      qt6-base-dev qt6-declarative-dev qt6-declarative-private-dev qt6-wayland-dev \
      qt6-shadertools-dev libwayland-dev wayland-protocols libjemalloc-dev >/dev/null 2>&1 || \
      apt-get install -y --no-install-recommends \
      sway foot qt6-wayland qml6-module-qtquick git cmake ninja-build build-essential pkg-config \
      qt6-base-dev qt6-declarative-dev qt6-declarative-private-dev qt6-wayland-dev libwayland-dev

    export XDG_RUNTIME_DIR=/run/xdgr; mkdir -p "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
    export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1
    export QT_QUICK_BACKEND=software QT_QPA_PLATFORM=offscreen QT_LOGGING_RULES="qt.qml*=false"

    # --- DB0-qt-sw: does Qt6 QtQuick render at all under the software backend? (foundation) ---
    cat > /tmp/probe.qml <<QML
import QtQuick
Rectangle { width: 64; height: 64; color: "#5aa02c"
  Component.onCompleted: { console.log("SHREK-QT-SW ok"); Qt.callLater(Qt.quit) } }
QML
    QMLRUN="$(command -v qml6 || command -v qml || true)"
    if [ -n "$QMLRUN" ] && timeout 30 "$QMLRUN" /tmp/probe.qml 2>&1 | grep -q "SHREK-QT-SW ok"; then
      gate ok DB0-qt-sw; else gate no DB0-qt-sw; fi

    # --- build Quickshell from the pinned tag (AUTO-FIRST-BUILD resolves newest, logs it) ---
    git clone --quiet "$QS_REPO" /tmp/qs
    cd /tmp/qs
    if [ "$QS_TAG" = "AUTO-FIRST-BUILD" ]; then
      QS_TAG="$(git tag --sort=-v:refname | head -1)"; echo "SHREK-QS-RESOLVED-TAG $QS_TAG  (record into image/supply/desktop.pins)"
    fi
    git checkout --quiet "$QS_TAG" || echo "WARN could not checkout $QS_TAG; building default HEAD"
    # Minimal feature set to shrink the dep closure for the smoke.
    cmake -S . -B build -GNinja -DCMAKE_BUILD_TYPE=Release \
      -DHYPRLAND=OFF -DSERVICE_STATUS_NOTIFIER=OFF -DSERVICE_PIPEWIRE=OFF -DSERVICE_MPRIS=OFF \
      -DCRASH_REPORTER=OFF 2>&1 | tail -5 || echo "WARN cmake configure returned nonzero"
    if ninja -C build 2>&1 | tail -3 && ninja -C build install 2>&1 | tail -2; then
      QS="$(command -v quickshell || echo /usr/local/bin/quickshell)"
    else
      QS=""; echo "WARN quickshell build failed"
    fi

    # --- DB0-sway: start Sway headless ---
    export SWAYSOCK=/run/xdgr/sway.sock
    ( sway -c /work/layers/shrek-desktop/overlay/usr/share/shrek/desktop/sway.config \
        >/tmp/sway.log 2>&1 & echo $! > /tmp/sway.pid ) || true
    for i in $(seq 1 30); do swaymsg -t get_version >/dev/null 2>&1 && break; sleep 1; done
    if swaymsg -t get_version >/dev/null 2>&1; then gate ok DB0-sway; else gate no DB0-sway; sed -n "1,40p" /tmp/sway.log; fi

    # --- DB0-qs-load / DB0-surfaces: Quickshell loads Shell.qml + logs the surfaces marker ---
    if [ -n "$QS" ]; then
      QML_IMPORT_PATH=/work/ui timeout 25 "$QS" -p /work/ui/shell/Shell.qml >/tmp/qs.log 2>&1 || true
      grep -qiE "error|is not a type|cannot" /tmp/qs.log && QSERR=1 || QSERR=0
      [ "$QSERR" = 0 ] && gate ok DB0-qs-load || { gate no DB0-qs-load; sed -n "1,40p" /tmp/qs.log; }
      grep -q "SHREK-DESKTOP shell surfaces instantiated" /tmp/qs.log && gate ok DB0-surfaces || gate no DB0-surfaces
    else
      gate no DB0-qs-load; gate no DB0-surfaces
    fi

    # --- DB0-logout: clean session teardown ---
    if swaymsg exit >/dev/null 2>&1; then gate ok DB0-logout; else gate no DB0-logout; fi

    echo "=================== DB0 RESULT ==================="
    echo "PASS=$PASS FAIL=$FAIL"
  '
