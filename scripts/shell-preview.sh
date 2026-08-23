#!/usr/bin/env bash
# Render the Shrek shell headless and SCREENSHOT it — DEV ONLY, for visual iteration without a VM
# cold-boot. Reuses the cached Quickshell binary (build it first via scripts/qml-check.sh). Boots a
# headless Sway, loads ui/shell.qml, and grabs PNGs of the bar, launcher, Work/System drawers, and
# dashboard into out/preview/. Software render (pixman/llvmpipe), so it matches the VM's NOGL path.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"
CACHE=out/qs-cache
[ -x "$CACHE/quickshell" ] || { echo "no cached quickshell — run scripts/qml-check.sh first"; exit 1; }
BLOBS_QML=out/caelestia-blobs/qml
[ -f "$BLOBS_QML/Caelestia/Blobs/qmldir" ] || bash scripts/build-caelestia-blobs.sh
HOST_UID="$(id -u)"; HOST_GID="$(id -g)"
mkdir -p out/preview
docker run --rm --privileged -v "${REPO_ROOT}:/work" -w /work \
  -e CACHE="${CACHE}" -e BLOBS_QML="${BLOBS_QML}" -e HOST_UID="${HOST_UID}" -e HOST_GID="${HOST_GID}" \
  debian:trixie bash -euo pipefail -c '
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq >/dev/null
    apt-get install -y --no-install-recommends -qq \
      sway foot grim qt6-wayland qml6-module-qtquick qml6-module-qtquick-window \
      qml6-module-qtquick-layouts qml6-module-qtquick-shapes libqt6widgets6 libqt6dbus6 \
      libgl1-mesa-dri libglx-mesa0 libegl1 libgles2 libxcb1 libpipewire-0.3-0 \
      fonts-dejavu-core papirus-icon-theme >/dev/null 2>&1 || echo "WARN pkgs"
    export XDG_RUNTIME_DIR=/run/xdgr; mkdir -p "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
    export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 WLR_HEADLESS_OUTPUTS=1
    export SWAYSOCK=/run/xdgr/sway.sock
    # stage the wallpaper at the absolute path the sealed config references, so the preview shows it
    mkdir -p /usr/share/shrek/desktop
    cp /work/layers/shrek-desktop/overlay/usr/share/shrek/desktop/wallpaper.png /usr/share/shrek/desktop/wallpaper.png 2>/dev/null || true
    # icon theme resolution (same as the sealed shrek-desktop wrapper): gtk settings in /usr via XDG_CONFIG_DIRS
    cp -r /work/layers/shrek-desktop/overlay/usr/share/shrek/xdg /usr/share/shrek/xdg 2>/dev/null || true
    export XDG_CONFIG_DIRS="/usr/share/shrek/xdg:/etc/xdg"
    sway -c /work/layers/shrek-desktop/overlay/usr/share/shrek/desktop/sway.config >/tmp/sway.log 2>&1 &
    for i in $(seq 1 30); do swaymsg -t get_version >/dev/null 2>&1 && break; sleep 1; done
    swaymsg -t get_version >/dev/null 2>&1 || { echo "sway failed"; sed -n 1,20p /tmp/sway.log; exit 1; }
    swaymsg output "*" resolution 1440x900 >/dev/null 2>&1 || true
    OUT="$(swaymsg -t get_outputs | grep -oE "HEADLESS-[0-9]+" | head -1)"; OUT="${OUT:-HEADLESS-1}"
    WD="$(ls "$XDG_RUNTIME_DIR"/wayland-* 2>/dev/null | grep -v "\.lock$" | head -1)"; WD="$(basename "${WD:-wayland-1}")"
    export WAYLAND_DISPLAY="$WD" QT_QPA_PLATFORM=wayland   # grim + quickshell both need the display
    # Caelestia blobs are a custom QSGMaterial shader — the Qt Quick "software" backend CANNOT paint
    # custom shaders (they render as nothing). Use the RHI/OpenGL backend over llvmpipe (software GL,
    # no GPU hardware needed) so the blob surface actually renders. QSG_RENDER_LOOP=basic avoids the
    # llvmpipe threaded-render-loop segfault. This matches the sealed-image shrek-desktop wrapper.
    export QSG_RHI_BACKEND=opengl LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe QSG_RENDER_LOOP=basic
    export QML_IMPORT_PATH="/work/$BLOBS_QML${QML_IMPORT_PATH:+:$QML_IMPORT_PATH}"
    export QML2_IMPORT_PATH="/work/$BLOBS_QML${QML2_IMPORT_PATH:+:$QML2_IMPORT_PATH}"
    "/work/$CACHE/quickshell" -p /work/ui/shell.qml >/tmp/qs.log 2>&1 &
    sleep 10   # llvmpipe GL init is slower than the software backend
    shot() { grim -o "$OUT" "/work/out/preview/$1.png" 2>>/tmp/grim.log || grim "/work/out/preview/$1.png" 2>>/tmp/grim.log || echo "grim $1 failed"; }
    # spawn real windows (as wayland clients) so the taskbar pills + titlebar chrome show in the shots
    foot --title=editor sh -c "exec sleep 600" >/tmp/foot1.log 2>&1 &
    foot --title=logs   sh -c "exec sleep 600" >/tmp/foot2.log 2>&1 &
    sleep 4
    shot bar
    "/work/$CACHE/quickshell" -p /work/ui/shell.qml ipc call edge rightOpen >/dev/null 2>&1 || true
    sleep 1; shot quickdock
    "/work/$CACHE/quickshell" -p /work/ui/shell.qml ipc call edge close >/dev/null 2>&1 || true
    "/work/$CACHE/quickshell" -p /work/ui/shell.qml ipc call railpopout open system 120 >/dev/null 2>&1 || true
    sleep 1; shot rail-popout
    "/work/$CACHE/quickshell" -p /work/ui/shell.qml ipc call railpopout close >/dev/null 2>&1 || true
    "/work/$CACHE/quickshell" -p /work/ui/shell.qml ipc call launcher toggle >/dev/null 2>&1 || true
    sleep 2; shot launcher
    "/work/$CACHE/quickshell" -p /work/ui/shell.qml ipc call launcher toggle >/dev/null 2>&1 || true
    "/work/$CACHE/quickshell" -p /work/ui/shell.qml ipc call work toggle >/dev/null 2>&1 || true
    sleep 2; shot work
    "/work/$CACHE/quickshell" -p /work/ui/shell.qml ipc call work toggle >/dev/null 2>&1 || true
    "/work/$CACHE/quickshell" -p /work/ui/shell.qml ipc call system toggle >/dev/null 2>&1 || true
    sleep 2; shot system
    "/work/$CACHE/quickshell" -p /work/ui/shell.qml ipc call system toggle >/dev/null 2>&1 || true   # close the system drawer
    "/work/$CACHE/quickshell" -p /work/ui/shell.qml ipc call menu open >/dev/null 2>&1 || true
    sleep 2; shot menu
    "/work/$CACHE/quickshell" -p /work/ui/shell.qml ipc call dashboard toggle >/dev/null 2>&1 || true
    sleep 2; shot dashboard
    "/work/$CACHE/quickshell" -p /work/ui/shell.qml ipc call dashboard toggle >/dev/null 2>&1 || true
    swaymsg exit >/dev/null 2>&1 || true
    chown -R "$HOST_UID:$HOST_GID" /work/out/preview 2>/dev/null || true
    echo "--- quickshell preview log ---"; sed -n "1,120p" /tmp/qs.log
    echo "--- previews ---"; ls -la /work/out/preview; [ -s /tmp/grim.log ] && { echo "grim log:"; cat /tmp/grim.log; } || true
  '
