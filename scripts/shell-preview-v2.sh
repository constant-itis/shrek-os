#!/usr/bin/env bash
# Render shell-v2 (ui-v2/) headless and SCREENSHOT it — DEV ONLY, for fast visual iteration with no VM.
#
# Deliberately minimal vs scripts/shell-preview.sh: NO Caelestia blobs, NO GL/llvmpipe packages. Boots a
# headless Sway on the pixman software renderer and loads ui-v2/shell.qml under QT_QUICK_BACKEND=software
# — the same no-GPU path the sealed VM uses (NOGL). Reuses the cached Quickshell binary; build it first
# via scripts/qml-check.sh if out/qs-cache/quickshell is absent.
#
# Captures out/preview-v2/{closed,open}.png (panel closed, then toggled open over the IPC socket).
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"
CACHE=out/qs-cache
[ -x "$CACHE/quickshell" ] || { echo "no cached quickshell — run scripts/qml-check.sh first"; exit 1; }
HOST_UID="$(id -u)"; HOST_GID="$(id -g)"
mkdir -p out/preview-v2
docker run --rm --privileged -v "${REPO_ROOT}:/work" -w /work \
  -e CACHE="${CACHE}" -e HOST_UID="${HOST_UID}" -e HOST_GID="${HOST_GID}" \
  debian:trixie bash -euo pipefail -c '
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq >/dev/null
    apt-get install -y --no-install-recommends -qq \
      sway foot grim qt6-wayland qml6-module-qtquick qml6-module-qtquick-window \
      qml6-module-qtquick-layouts qml6-module-qtquick-shapes libqt6widgets6 libqt6dbus6 \
      libgl1-mesa-dri libxcb1 libpipewire-0.3-0 fonts-dejavu-core >/dev/null 2>&1 || echo "WARN pkgs"

    export XDG_RUNTIME_DIR=/run/xdgr; mkdir -p "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
    export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 WLR_HEADLESS_OUTPUTS=1
    export SWAYSOCK=/run/xdgr/sway.sock

    # Minimal headless Sway config: outer gaps so the desktop frame border is visible, borderless windows,
    # and NO exec (we launch quickshell ourselves as a wayland client so we control timing + IPC).
    cat > /tmp/sway.config <<EOF
output * bg #0a0d0a solid_color
gaps outer 8
default_border pixel 1
EOF
    sway -c /tmp/sway.config >/tmp/sway.log 2>&1 &
    for i in $(seq 1 30); do swaymsg -t get_version >/dev/null 2>&1 && break; sleep 1; done
    swaymsg -t get_version >/dev/null 2>&1 || { echo "sway failed"; sed -n 1,20p /tmp/sway.log; exit 1; }
    swaymsg output "*" resolution 1440x900 >/dev/null 2>&1 || true
    OUT="$(swaymsg -t get_outputs | grep -oE "HEADLESS-[0-9]+" | head -1)"; OUT="${OUT:-HEADLESS-1}"
    WD="$(ls "$XDG_RUNTIME_DIR"/wayland-* 2>/dev/null | grep -v "\.lock$" | head -1)"; WD="$(basename "${WD:-wayland-1}")"
    export WAYLAND_DISPLAY="$WD" QT_QPA_PLATFORM=wayland QT_QUICK_BACKEND=software

    "/work/$CACHE/quickshell" -p /work/ui-v2/shell.qml >/tmp/qs.log 2>&1 &
    sleep 6
    if grep -qiE "error|failed|warning:" /tmp/qs.log; then echo "=== qs.log ==="; cat /tmp/qs.log; fi

    # Spawn a couple of real windows so the reserved rail + click-through frame are visible in the shots.
    foot --title=editor sh -c "exec sleep 600" >/tmp/foot1.log 2>&1 &
    foot --title=logs   sh -c "exec sleep 600" >/tmp/foot2.log 2>&1 &
    sleep 3

    shot() { grim -o "$OUT" "/work/out/preview-v2/$1.png" 2>>/tmp/grim.log || grim "/work/out/preview-v2/$1.png" 2>>/tmp/grim.log || echo "grim $1 failed"; }
    shot closed
    "/work/$CACHE/quickshell" -p /work/ui-v2/shell.qml ipc call ui togglePanel true >/dev/null 2>&1 || echo "ipc toggle failed"
    sleep 1
    shot open

    echo "=== qs.log (full) ==="; cat /tmp/qs.log || true
    chown -R "${HOST_UID}:${HOST_GID}" /work/out/preview-v2 2>/dev/null || true
  '
echo "shots in out/preview-v2/"
