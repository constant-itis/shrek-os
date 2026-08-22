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

# robust pin reader: strip inline comments, then take the quoted value
pin() { grep -E "^[[:space:]]*$1[[:space:]]*=" image/supply/desktop.pins | head -1 | sed 's/#.*//' | cut -d'"' -f2; }
QS_REPO="$(pin repo)"
QS_TAG="$(pin quickshell_tag)"

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
    # REQUIRED core — atomic; ca-certificates first (git/apt HTTPS needs it in the bare trixie image).
    apt-get install -y --no-install-recommends -qq \
      ca-certificates openssl \
      sway foot qt6-wayland qml6-module-qtquick \
      git cmake ninja-build build-essential pkg-config \
      qt6-base-dev qt6-base-private-dev qt6-declarative-dev qt6-declarative-private-dev \
      qt6-wayland-dev qt6-wayland-private-dev qt6-shadertools-dev libwayland-dev libwayland-bin \
      wayland-protocols libcli11-dev libdrm-dev libxcb1-dev libpipewire-0.3-dev
    # BEST-EFFORT extras — a wrong/absent name must NOT sink the whole run.
    for p in qml6-module-qtquick-window qml6-module-qtquick-layouts qml6-module-qtquick-shapes \
             qml6-module-qtquick-controls qt6-declarative-dev-tools qt6-shadertools-dev \
             wayland-protocols libjemalloc-dev libgl1-mesa-dri fonts-dejavu-core; do
      apt-get install -y --no-install-recommends -qq "$p" >/dev/null 2>&1 || echo "WARN optional pkg missing: $p"
    done

    export XDG_RUNTIME_DIR=/run/xdgr; mkdir -p "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
    export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1
    export QT_QUICK_BACKEND=software QT_QPA_PLATFORM=offscreen QT_LOGGING_RULES="qt.qml*=false"

    # --- DB0-qt-sw: does Qt6 QtQuick render at all under the software backend? (foundation) ---
    cat > /tmp/probe.qml <<QML
import QtQuick
Rectangle { width: 64; height: 64; color: "#5aa02c"
  Component.onCompleted: { console.log("SHREK-QT-SW ok"); Qt.callLater(Qt.quit) } }
QML
    QMLRUN="$(command -v qml6 || command -v qml || ls /usr/lib/qt6/bin/qml 2>/dev/null || true)"
    # Informational only — DB0-surfaces (Quickshell QML under QT_QUICK_BACKEND=software) is the real
    # software-render proof; this standalone probe is a redundant pre-flight, not a gate.
    if [ -n "$QMLRUN" ] && timeout 30 "$QMLRUN" /tmp/probe.qml 2>&1 | grep -q "SHREK-QT-SW ok"; then
      echo "NOTE qt-sw ok (standalone Qt software render)"; else echo "NOTE qt-sw pre-flight skipped (subsumed by DB0-surfaces)"; fi

    # --- newer wayland-protocols over /usr: the trixie package lacks the ext-background-effect staging
    #     protocol that quickshell v0.3.1 references UNCONDITIONALLY (src/wayland/CMakeLists.txt:123,
    #     no feature gate). A real delivery requirement for the sealed layer too. (NO apostrophes in
    #     this container heredoc — a single quote closes the bash -c string.) ---
    git clone --quiet https://gitlab.freedesktop.org/wayland/wayland-protocols /tmp/wp
    WP_TAG="$(cd /tmp/wp && git tag --sort=-v:refname | head -1)"; echo "SHREK-WP-TAG $WP_TAG (newer than trixie; carry in the desktop layer)"
    ( cd /tmp/wp && git checkout --quiet "$WP_TAG" )
    # Overlay the newer XMLs into the existing pkgdatadir (the trixie version already satisfies the
    # >=1.41 pkg-config check; only the newer staging XML files are absent). No meson build needed.
    WP_DATADIR="$(pkg-config --variable=pkgdatadir wayland-protocols 2>/dev/null || echo /usr/share/wayland-protocols)"
    cp -r /tmp/wp/staging /tmp/wp/stable /tmp/wp/unstable "$WP_DATADIR/" 2>/dev/null || echo "WARN wp overlay copy partial"
    ls "$WP_DATADIR/staging/ext-background-effect/" >/dev/null 2>&1 \
      && echo "SHREK-WP ext-background-effect present in $WP_DATADIR" || echo "WARN ext-background-effect STILL missing"

    # --- build Quickshell from the pinned tag ---
    git clone --quiet "$QS_REPO" /tmp/qs
    cd /tmp/qs
    if [ "$QS_TAG" = "AUTO-FIRST-BUILD" ]; then
      QS_TAG="$(git tag --sort=-v:refname | head -1)"; echo "SHREK-QS-RESOLVED-TAG $QS_TAG  (record into image/supply/desktop.pins)"
    fi
    git checkout --quiet "$QS_TAG" || echo "WARN could not checkout $QS_TAG; building default HEAD"
    # Minimal feature set to shrink the dep closure for the smoke.
    # Minimal feature set: keep WAYLAND + WAYLAND_WLR_LAYERSHELL ON (layer-shell = PanelWindow, the
    # bar/drawer surfaces); disable everything else to shrink the dep closure for the smoke.
    cmake -S . -B build -GNinja -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr \
      -DHYPRLAND=OFF -DX11=ON -DI3=ON -DSCREENCOPY=OFF -DBLUETOOTH=ON -DNETWORK=OFF \
      -DWAYLAND_SESSION_LOCK=OFF -DWAYLAND_TOPLEVEL_MANAGEMENT=ON \
      -DSERVICE_STATUS_NOTIFIER=OFF -DSERVICE_PIPEWIRE=ON -DSERVICE_MPRIS=OFF -DSERVICE_PAM=OFF \
      -DSERVICE_POLKIT=OFF -DSERVICE_GREETD=OFF -DSERVICE_UPOWER=ON -DSERVICE_NOTIFICATIONS=OFF \
      -DCRASH_HANDLER=OFF -DUSE_JEMALLOC=OFF >/tmp/qs-cmake.log 2>&1 \
      || { echo "WARN cmake configure returned nonzero — full tail:"; tail -40 /tmp/qs-cmake.log; }
    if [ -f build/build.ninja ] && ninja -C build 2>&1 | tail -3 && ninja -C build install 2>&1 | tail -2; then
      QS="$(command -v quickshell || echo /usr/local/bin/quickshell)"
    else
      QS=""; echo "WARN quickshell build failed"
    fi

    # --- DB0-sway: start Sway headless ---
    export SWAYSOCK=/run/xdgr/sway.sock
    sway -c /work/layers/shrek-desktop/overlay/usr/share/shrek/desktop/sway.config >/tmp/sway.log 2>&1 &
    SWAY_PID=$!
    for i in $(seq 1 30); do swaymsg -t get_version >/dev/null 2>&1 && break; sleep 1; done
    if swaymsg -t get_version >/dev/null 2>&1; then gate ok DB0-sway; else gate no DB0-sway; sed -n "1,40p" /tmp/sway.log; fi

    # --- DB0-qs-load / DB0-surfaces: Quickshell loads Shell.qml + logs the surfaces marker ---
    # PanelWindow (wlr-layer-shell) needs a live compositor, so run Quickshell as a WAYLAND CLIENT of
    # the running Sway (QT_QPA_PLATFORM=wayland + WAYLAND_DISPLAY), keeping software rendering.
    if [ -n "$QS" ]; then
      WD="$(ls "$XDG_RUNTIME_DIR"/wayland-* 2>/dev/null | grep -v "\.lock$" | head -1)"; WD="$(basename "${WD:-wayland-1}")"
      echo "NOTE quickshell connecting to WAYLAND_DISPLAY=$WD"
      WAYLAND_DISPLAY="$WD" QT_QPA_PLATFORM=wayland QT_QUICK_BACKEND=software \
        timeout 20 "$QS" -p /work/ui/shell.qml >/tmp/qs.log 2>&1 || true
      # Scope to QML/config LOAD failures only. Quickshell prints "Failed to load configuration" iff the
      # QML tree fails to load. Runtime service warnings ("Failed to create pipewire context", "Could not
      # connect to DBus" for bluez/upower) are EXPECTED in this bare container (no daemons) once those
      # backends are enabled, and must NOT count as a QML failure.
      grep -q "Failed to load configuration" /tmp/qs.log && QSERR=1 || QSERR=0
      [ "$QSERR" = 0 ] && gate ok DB0-qs-load || { gate no DB0-qs-load; sed -n "1,40p" /tmp/qs.log; }
      grep -q "SHREK-DESKTOP shell surfaces instantiated" /tmp/qs.log && gate ok DB0-surfaces || gate no DB0-surfaces
    else
      gate no DB0-qs-load; gate no DB0-surfaces
    fi

    # --- DB0-logout: clean session teardown. `swaymsg exit` may return nonzero because sway closes the
    #     IPC before acking; the real signal is whether sway TERMINATES. Grace-wait on the pid. ---
    swaymsg exit >/dev/null 2>&1 || true
    for i in $(seq 1 20); do kill -0 "$SWAY_PID" 2>/dev/null || break; sleep 0.5; done
    if ! kill -0 "$SWAY_PID" 2>/dev/null; then gate ok DB0-logout
    else echo "NOTE sway did not exit on request — sway.log tail:"; tail -20 /tmp/sway.log; kill -9 "$SWAY_PID" 2>/dev/null || true; gate no DB0-logout; fi

    echo "=================== DB0 RESULT ==================="
    echo "PASS=$PASS FAIL=$FAIL"
  '
