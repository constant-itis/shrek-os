#!/usr/bin/env bash
# Render shell-v2 (ui-v2/) headless and screenshot it for fast visual iteration.
#
# Captures out/preview-v2/{closed,system,network,audio,appearance,work}.png using headless Sway with pixman and Qt's software
# backend. The Work shot uses a seeded shrek-session/1 record for visual preview only; the real
# gatekeeperd writer path is covered by scripts/desktop-session-proof.sh.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"
CACHE=out/qs-cache
[ -x "$CACHE/quickshell" ] || { echo "no cached quickshell; run scripts/qml-check.sh first"; exit 1; }
HOST_UID="$(id -u)"; HOST_GID="$(id -g)"
mkdir -p out/preview-v2

RUNNER="$(mktemp -p out preview-v2-run.XXXXXX.sh)"
cleanup() { rm -f "$RUNNER"; }
trap cleanup EXIT

cat > "$RUNNER" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null
apt-get install -y --no-install-recommends -qq \
  sway foot grim qt6-wayland qml6-module-qtquick qml6-module-qtquick-window \
  qml6-module-qtquick-layouts qml6-module-qtquick-shapes libqt6widgets6 libqt6dbus6 \
  libgl1-mesa-dri libxcb1 libpipewire-0.3-0 fonts-dejavu-core >/dev/null 2>&1 || echo "WARN pkgs"

export XDG_RUNTIME_DIR=/run/xdgr
rm -rf "$XDG_RUNTIME_DIR"
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 WLR_HEADLESS_OUTPUTS=1
export SWAYSOCK="$XDG_RUNTIME_DIR/sway-ipc.sock"

cat >/tmp/sway-v2.conf <<'EOF'
seat seat0 fallback true
output * bg #0a0d0a solid_color
default_border none
default_floating_border none
gaps outer 8
EOF
sway -c /tmp/sway-v2.conf >/tmp/sway.log 2>&1 &
for _ in $(seq 1 30); do swaymsg -t get_version >/dev/null 2>&1 && break; sleep 1; done
swaymsg -t get_version >/dev/null 2>&1 || { echo "sway failed"; sed -n '1,20p' /tmp/sway.log; exit 1; }
swaymsg output "*" resolution 1440x900 >/dev/null 2>&1 || true

WD="$(find "$XDG_RUNTIME_DIR" -maxdepth 1 -type s -name "wayland-*" -printf "%f\n" | sort | head -1)"
[ -n "$WD" ] || { echo "no wayland socket in $XDG_RUNTIME_DIR"; ls -la "$XDG_RUNTIME_DIR"; exit 1; }
OUT="$(swaymsg -t get_outputs -r | grep -oE "HEADLESS-[0-9]+" | head -1)"
[ -n "$OUT" ] || { echo "no HEADLESS output"; swaymsg -t get_outputs -r; exit 1; }
export WAYLAND_DISPLAY="$WD" QT_QPA_PLATFORM=wayland QT_QUICK_BACKEND=software
echo "preview: OUT=$OUT WAYLAND_DISPLAY=$WD SWAYSOCK=$SWAYSOCK"

SDIR=/run/xdgr/shrek-session
mkdir -p "$SDIR"
cat > "$SDIR/s0.json" <<'JSON'
{
  "schema": "shrek-session/1",
  "session": "s0",
  "state": "active",
  "subject": "dev1",
  "effective": {
    "tier": "T2",
    "trust": "T-untrust",
    "caps": "cnet",
    "profile": "cnet",
    "egress_profile": "model-anthropic",
    "egress_dst": "shrek-model-proxy:8200"
  },
  "semantic": { "available": true, "freshness": "live", "tier": "fts+semantic" }
}
JSON
export SHREK_SESSION_DIR="$SDIR"

"/work/$CACHE/quickshell" -p /work/ui-v2/shell.qml >/tmp/qs.log 2>&1 &
sleep 6
if grep -qiE "error|failed|warning:" /tmp/qs.log; then echo "=== qs.log ==="; cat /tmp/qs.log; fi
grep -E "SHREK-DESKTOP (shell surfaces|work session)" /tmp/qs.log || echo "NOTE: markers not yet in qs.log"

for n in 2 3; do swaymsg workspace number "$n" >/dev/null 2>&1 || true; done
swaymsg workspace number 1 >/dev/null 2>&1 || true
foot --title=editor sh -c "exec sleep 600" >/tmp/foot1.log 2>&1 &
foot --title=logs sh -c "exec sleep 600" >/tmp/foot2.log 2>&1 &
sleep 3

shot() {
  grim -o "$OUT" "/work/out/preview-v2/$1.png" 2>>/tmp/grim.log \
    || grim "/work/out/preview-v2/$1.png" 2>>/tmp/grim.log \
    || echo "grim $1 failed"
}
shot closed
"/work/$CACHE/quickshell" -p /work/ui-v2/shell.qml ipc call ui system >/dev/null 2>&1 || echo "ipc system failed"
sleep 1
shot system
"/work/$CACHE/quickshell" -p /work/ui-v2/shell.qml ipc call ui network >/dev/null 2>&1 || echo "ipc network failed"
sleep 1
shot network
"/work/$CACHE/quickshell" -p /work/ui-v2/shell.qml ipc call ui audio >/dev/null 2>&1 || echo "ipc audio failed"
sleep 1
shot audio
"/work/$CACHE/quickshell" -p /work/ui-v2/shell.qml ipc call ui appearance >/dev/null 2>&1 || echo "ipc appearance failed"
sleep 1
shot appearance
"/work/$CACHE/quickshell" -p /work/ui-v2/shell.qml ipc call ui close >/dev/null 2>&1 || true
"/work/$CACHE/quickshell" -p /work/ui-v2/shell.qml ipc call ui toggleWork true >/dev/null 2>&1 || echo "ipc work toggle failed"
sleep 1
shot work

echo "=== grim.log ==="; cat /tmp/grim.log 2>/dev/null || true
chown -R "${HOST_UID}:${HOST_GID}" /work/out/preview-v2 2>/dev/null || true
SCRIPT
chmod +x "$RUNNER"

docker run --rm --privileged -v "${REPO_ROOT}:/work" -w /work \
  -e CACHE="${CACHE}" -e HOST_UID="${HOST_UID}" -e HOST_GID="${HOST_GID}" \
  debian:trixie /work/"$RUNNER"
echo "shots in out/preview-v2/"
