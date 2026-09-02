#!/usr/bin/env bash
# Render the static installer scaffold (ui-installer/) headless and screenshot every screen for sign-off.
#
# Reuses the cached Quickshell binary from scripts/qml-check.sh (out/qs-cache/quickshell). Each screen is a
# fresh quickshell process with SHREK_INSTALLER_SCREEN set (the scaffold selects the screen from that env);
# the first-run fault state is driven by SHREK_INSTALLER_FAULT=1. Captures out/preview-installer/*.png and
# fails if any screen's QML tree fails to load.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"
CACHE=out/qs-cache
[ -x "$CACHE/quickshell" ] || { echo "no cached quickshell; run scripts/qml-check.sh first"; exit 1; }
HOST_UID="$(id -u)"; HOST_GID="$(id -g)"
mkdir -p out/preview-installer

RUNNER="$(mktemp -p out preview-installer-run.XXXXXX.sh)"
trap 'rm -f "$RUNNER"' EXIT

cat > "$RUNNER" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null
apt-get install -y --no-install-recommends -qq \
  sway grim qt6-wayland qml6-module-qtquick qml6-module-qtquick-window \
  qml6-module-qtquick-layouts qml6-module-qtquick-shapes libqt6widgets6 libqt6dbus6 \
  libgl1-mesa-dri libxcb1 libpipewire-0.3-0 fonts-dejavu-core >/dev/null 2>&1 || echo "WARN pkgs"

export XDG_RUNTIME_DIR=/run/xdgr; rm -rf "$XDG_RUNTIME_DIR"; mkdir -p "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 WLR_HEADLESS_OUTPUTS=1
export SWAYSOCK="$XDG_RUNTIME_DIR/sway-ipc.sock"

cat >/tmp/sway.conf <<'EOF'
seat seat0 fallback true
output * bg #101014 solid_color
default_border none
default_floating_border none
EOF
sway -c /tmp/sway.conf >/tmp/sway.log 2>&1 &
for _ in $(seq 1 30); do swaymsg -t get_version >/dev/null 2>&1 && break; sleep 1; done
swaymsg -t get_version >/dev/null 2>&1 || { echo "sway failed"; sed -n '1,20p' /tmp/sway.log; exit 1; }
swaymsg output "*" resolution 1280x800 >/dev/null 2>&1 || true

WD="$(find "$XDG_RUNTIME_DIR" -maxdepth 1 -type s -name "wayland-*" -printf "%f\n" | sort | head -1)"
[ -n "$WD" ] || { echo "no wayland socket"; ls -la "$XDG_RUNTIME_DIR"; exit 1; }
OUT="$(swaymsg -t get_outputs -r | grep -oE "HEADLESS-[0-9]+" | head -1)"
[ -n "$OUT" ] || { echo "no HEADLESS output"; swaymsg -t get_outputs -r; exit 1; }
export WAYLAND_DISPLAY="$WD" QT_QPA_PLATFORM=wayland QT_QUICK_BACKEND=software
echo "preview: OUT=$OUT WAYLAND_DISPLAY=$WD"

QS=/work/out/qs-cache/quickshell
CFG=/work/ui-installer/shell.qml
FAIL=0

shot() {
  local name="$1" screen="$2" fault="${3:-}"
  SHREK_INSTALLER_SCREEN="$screen" SHREK_INSTALLER_FAULT="$fault" \
    "$QS" -p "$CFG" >"/tmp/qs-$name.log" 2>&1 &
  local pid=$!
  sleep 4
  grim -o "$OUT" "/work/out/preview-installer/$name.png" 2>>/tmp/grim.log \
    || grim "/work/out/preview-installer/$name.png" 2>>/tmp/grim.log \
    || echo "grim $name failed"
  kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true
  if grep -q "Failed to load configuration" "/tmp/qs-$name.log"; then
    echo "  LOAD FAIL ($name):"; grep -A6 "Failed to load configuration" "/tmp/qs-$name.log"; FAIL=1
  elif grep -q "SHREK-INSTALLER surfaces instantiated" "/tmp/qs-$name.log"; then
    echo "  ok: $name"
  else
    echo "  NOMARKER ($name)"; tail -6 "/tmp/qs-$name.log"; FAIL=1
  fi
}

shot welcome        welcome
shot locale         locale
shot name           name
shot disk           disk
shot erase          erase
shot progress       progress
shot done           done
shot firstrun       firstrun
shot firstrun-fault firstrun 1

echo "=== grim.log ==="; cat /tmp/grim.log 2>/dev/null || true
chown -R "${HOST_UID}:${HOST_GID}" /work/out/preview-installer 2>/dev/null || true
if [ "$FAIL" = 0 ]; then echo "INSTALLER-PREVIEW: PASS"; else echo "INSTALLER-PREVIEW: FAIL"; exit 1; fi
SCRIPT
chmod +x "$RUNNER"

docker run --rm --privileged -v "${REPO_ROOT}:/work" -w /work \
  -e HOST_UID="${HOST_UID}" -e HOST_GID="${HOST_GID}" \
  debian:trixie /work/"$RUNNER"
echo "shots in out/preview-installer/"
