#!/usr/bin/env bash
# Render the Shrek shell under EACH theme mode and screenshot it — DEV ONLY, visual proof that every mode
# resolves through the semantic contract and repaints the shell (no VM needed). Reuses the cached Quickshell
# binary (build it first via scripts/qml-check.sh). For each mode it writes a throwaway theme.json, points
# the shell at it via SHREK_THEME_CONFIG, and grabs the bar + launcher (the launcher shows the scrim, panel,
# accent-tinted selected row and text roles — the palette differences are obvious there).
#
# Output: out/preview/theme-<mode>-{bar,launcher}.png
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"
CACHE=out/qs-cache
[ -x "$CACHE/quickshell" ] || { echo "no cached quickshell — run scripts/qml-check.sh first"; exit 1; }
HOST_UID="$(id -u)"; HOST_GID="$(id -g)"
mkdir -p out/preview
docker run --rm --privileged -v "${REPO_ROOT}:/work" -w /work \
  -e CACHE="${CACHE}" -e HOST_UID="${HOST_UID}" -e HOST_GID="${HOST_GID}" \
  debian:trixie bash -euo pipefail -c '
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq >/dev/null
    apt-get install -y --no-install-recommends -qq \
      sway foot grim qt6-wayland qml6-module-qtquick qml6-module-qtquick-window \
      qml6-module-qtquick-layouts qml6-module-qtquick-shapes libqt6widgets6 libqt6dbus6 \
      libgl1-mesa-dri libxcb1 libpipewire-0.3-0 fonts-dejavu-core papirus-icon-theme >/dev/null 2>&1 || echo "WARN pkgs"
    export XDG_RUNTIME_DIR=/run/xdgr; mkdir -p "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
    export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 WLR_HEADLESS_OUTPUTS=1
    export SWAYSOCK=/run/xdgr/sway.sock
    mkdir -p /usr/share/shrek/desktop
    cp /work/layers/shrek-desktop/overlay/usr/share/shrek/desktop/wallpaper.png /usr/share/shrek/desktop/wallpaper.png 2>/dev/null || true
    cp -r /work/layers/shrek-desktop/overlay/usr/share/shrek/xdg /usr/share/shrek/xdg 2>/dev/null || true
    export XDG_CONFIG_DIRS="/usr/share/shrek/xdg:/etc/xdg"
    sway -c /work/layers/shrek-desktop/overlay/usr/share/shrek/desktop/sway.config >/tmp/sway.log 2>&1 &
    for i in $(seq 1 30); do swaymsg -t get_version >/dev/null 2>&1 && break; sleep 1; done
    swaymsg -t get_version >/dev/null 2>&1 || { echo "sway failed"; sed -n 1,20p /tmp/sway.log; exit 1; }
    swaymsg output "*" resolution 1440x900 >/dev/null 2>&1 || true
    OUT="$(swaymsg -t get_outputs | grep -oE "HEADLESS-[0-9]+" | head -1)"; OUT="${OUT:-HEADLESS-1}"
    WD="$(ls "$XDG_RUNTIME_DIR"/wayland-* 2>/dev/null | grep -v "\.lock$" | head -1)"; WD="$(basename "${WD:-wayland-1}")"
    export WAYLAND_DISPLAY="$WD" QT_QPA_PLATFORM=wayland QT_QUICK_BACKEND=software
    CFGDIR=/tmp/shrek-theme-cfg; mkdir -p "$CFGDIR"
    # a hand-authored custom base palette so the "custom" mode has something to resolve
    export HOME=/root; mkdir -p "$HOME/.config/shrek"
    cat > "$HOME/.config/shrek/custom.json" <<JSON
{ "name":"Preview Custom","appearance":"dark",
  "bg":"#1a1026","surface":"#241634","surfaceAlt":"#301e46","overlay":"#2a1a3c",
  "border":"#4a2e6a","borderStrong":"#5f3d86","panelBg":"#f2241634","rowHi":"#3a2358",
  "text":"#f2e9ff","textDim":"#b8a6d6","accent":"#b45cff","accentDim":"#8a34e0","accentText":"#12061f" }
JSON
    shot() { grim -o "$OUT" "/work/out/preview/$1.png" 2>>/tmp/grim.log || grim "/work/out/preview/$1.png" 2>>/tmp/grim.log || echo "grim $1 failed"; }
    render_mode() {
      MODE="$1"; JSON="$2"
      printf "%s" "$JSON" > "$CFGDIR/$MODE.json"
      export SHREK_THEME_CONFIG="$CFGDIR/$MODE.json"
      "/work/$CACHE/quickshell" -p /work/ui/shell.qml >/tmp/qs-$MODE.log 2>&1 &
      QSPID=$!
      sleep 5
      shot "theme-$MODE-bar"
      "/work/$CACHE/quickshell" -p /work/ui/shell.qml ipc call launcher toggle >/dev/null 2>&1 || true
      sleep 2; shot "theme-$MODE-launcher"
      "/work/$CACHE/quickshell" -p /work/ui/shell.qml ipc call launcher toggle >/dev/null 2>&1 || true
      kill "$QSPID" >/dev/null 2>&1 || true
      sleep 1
    }
    render_mode shrek-dark    "{\"mode\":\"shrek-dark\"}"
    render_mode shrek-light   "{\"mode\":\"shrek-light\"}"
    render_mode high-contrast "{\"mode\":\"high-contrast\"}"
    render_mode custom        "{\"mode\":\"custom\"}"
    render_mode override      "{\"mode\":\"shrek-dark\",\"overrides\":{\"accent\":\"#e0561f\",\"rowHi\":\"#3a2015\"}}"
    swaymsg exit >/dev/null 2>&1 || true
    chown -R "$HOST_UID:$HOST_GID" /work/out/preview 2>/dev/null || true
    echo "--- theme previews ---"; ls -la /work/out/preview/theme-* 2>/dev/null; [ -s /tmp/grim.log ] && { echo "grim log:"; cat /tmp/grim.log; } || true
  '
