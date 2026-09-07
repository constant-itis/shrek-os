#!/usr/bin/env bash
# Shrek OS — ADR-009 v2 S4: render proof for the STANDALONE "Network Access" panel (shrek-connectivity).
#
# S4 is pure QML + config (no Rust): a shrek-owned standalone Quickshell surface (the shrek-menu pattern)
# baked at /usr/share/shrek/dms/shrek-connectivity/shell.qml, toggled by Super+Shift+N. The live seat +
# the actual look are owner-verified in the dogfood VM (no Play-driving); this proof pins what a headless
# render CAN show: the surface loads clean, its embedded read model parses a representative /run
# projection (incl. an owner capability, a quarantine fault, and a pending-needs request), and the panel
# actually PAINTS (not a flat/blank frame) once toggled visible.
#
#   S4-load     the standalone surface loads (SHREK-CONNECTIVITY surface loaded), no config load error
#   S4-model    the embedded model parses the seed: profiles=5 raw=1 wants=2 available=1
#   S4-render   the toggled-visible panel paints a non-flat frame (>=200 unique colours)
#
# Mirrors scripts/desktop-connectivity-proof.sh (cached Quickshell + headless Sway + grim in debian:trixie).
set -uo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

SURFACE="layers/shrek-desktop/overlay/usr/share/shrek/dms/shrek-connectivity/shell.qml"
OUT="out/desktop-egress-adr009-s4-proof"
RUN="$OUT/run"
rm -rf "$OUT"; mkdir -p "$RUN"

# A representative projection exercising every S4 read path: baseline (status-only), a sealed one-click
# capability with card text (weather), a ceremony capability (web-browsing), an OWNER capability that was
# QUARANTINED (S3 §4.4 — source=owner, fault=quarantined + a capfault reason line), one advanced raw
# destination, plus a pending-needs inbox with two requests + one downstream event.
cat > "$RUN/state" <<'STATE'
schema shrek-egress-state/1
profile desktop-ntp tier=baseline blessed=0 pins=162.159.200.1,162.159.200.123 refreshed=- fault=-
profile desktop-updates tier=baseline blessed=0 pins=- refreshed=- fault=-
profile weather tier=one-click blessed=1 pins=104.18.5.99 refreshed=1750000000 fault=- source=sealed feature=dms:weather
profile web-browsing tier=ceremony blessed=1 pins=- refreshed=- fault=-
profile radar tier=one-click blessed=0 pins=- refreshed=- fault=quarantined source=owner feature=dms:radar
raw host=grafana.example.com proto=tcp port=443 blessed=1 pins=203.0.113.7 refreshed=1750000000
title weather Weather
purpose weather Local forecast and location search
title radar Rain Radar
purpose radar Local precipitation radar
capfault radar source=owner host `radar.example.test` became reserved by a system update
STATE
printf 'want radar 1750000001\nwant maptiles 1750000000\n' > "$RUN/wants"
printf '1750000000 bless weather 1 ip(s)\n' > "$RUN/events"

CACHE=out/qs-cache
if [ ! -x "$CACHE/quickshell" ]; then
  echo "!!! no cached Quickshell at $CACHE/quickshell — run scripts/qml-check.sh first (one-time build) !!!"
  exit 1
fi

echo "=== ADR-009 S4 Network Access render proof (cached Quickshell) in debian:trixie ==="
set +e
docker run --rm --privileged -v "${REPO_ROOT}:/work" -w /work -e CACHE="${CACHE}" -e SURFACE="${SURFACE}" \
  debian:trixie bash -euo pipefail -c '
    export DEBIAN_FRONTEND=noninteractive
    P=0; F=0
    g() { if [ "$1" = ok ]; then echo "SHREK_GATE: PASS $2"; P=$((P+1)); else echo "SHREK_GATE: FAIL $2"; F=$((F+1)); fi; }
    apt-get update -qq >/dev/null
    apt-get install -y --no-install-recommends -qq \
      sway grim imagemagick qt6-wayland qml6-module-qtquick qml6-module-qtquick-window \
      qml6-module-qtquick-layouts qml6-module-qtquick-shapes libqt6widgets6 libqt6dbus6 \
      libgl1-mesa-dri libxcb1 libpipewire-0.3-0 fonts-dejavu-core >/dev/null 2>&1 || echo "WARN some pkgs missing"

    export XDG_RUNTIME_DIR=/run/xdgr; mkdir -p "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
    export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1
    export SWAYSOCK=/run/xdgr/sway.sock
    export SHREK_EGRESS_RUN=/work/'"$RUN"'

    sway -c /work/layers/shrek-desktop/overlay/usr/share/shrek/desktop/sway.config >/tmp/sway.log 2>&1 &
    for i in $(seq 1 30); do swaymsg -t get_version >/dev/null 2>&1 && break; sleep 1; done
    swaymsg -t get_version >/dev/null 2>&1 || { echo "NOTE sway failed"; sed -n 1,30p /tmp/sway.log; }
    WD="$(ls "$XDG_RUNTIME_DIR"/wayland-* 2>/dev/null | grep -v "\.lock$" | head -1)"; WD="$(basename "${WD:-wayland-1}")"

    # Launch the standalone surface (long-lived; it starts hidden). The 2s model timer runs regardless of
    # visibility, so the load + model markers appear before we toggle.
    WAYLAND_DISPLAY="$WD" QT_QPA_PLATFORM=wayland QT_QUICK_BACKEND=software SHREK_EGRESS_RUN="$SHREK_EGRESS_RUN" \
      "/work/$CACHE/quickshell" -p "/work/$SURFACE" >/tmp/qs.log 2>&1 &
    QS_PID=$!
    for i in $(seq 1 24); do grep -q "SHREK-CONNECTIVITY surface loaded" /tmp/qs.log && break; sleep 0.5; done

    if grep -q "Failed to load configuration" /tmp/qs.log; then
      g no S4-load; grep -A8 "Failed to load configuration" /tmp/qs.log
    elif grep -q "SHREK-CONNECTIVITY surface loaded" /tmp/qs.log; then
      g ok S4-load
    else
      g no S4-load; sed -n 1,40p /tmp/qs.log
    fi

    # Let a couple of model polls run, then assert the parse of the seeded projection.
    sleep 3
    if grep -q "SHREK-CONNECTIVITY egress state profiles=5 raw=1 wants=2 available=1" /tmp/qs.log; then
      g ok S4-model
    else
      g no S4-model; grep -iE "SHREK-CONNECTIVITY|error" /tmp/qs.log | tail
    fi

    # Toggle the surface visible over IPC (same seam as the Super+Shift+N bind), then grab a frame.
    WAYLAND_DISPLAY="$WD" "/work/$CACHE/quickshell" -p "/work/$SURFACE" ipc call shrek-connectivity show \
      >/dev/null 2>&1 || echo "WARN ipc show failed"
    sleep 3

    mkdir -p "/work/'"$OUT"'/frame"
    FRAME="/work/'"$OUT"'/frame/network-access.png"
    WAYLAND_DISPLAY="$WD" grim "$FRAME" 2>/tmp/grim.log || echo "WARN grim failed: $(cat /tmp/grim.log)"
    if [ -f "$FRAME" ]; then
      COLOURS="$(convert "$FRAME" -format "%k" info: 2>/dev/null || echo 0)"
      echo "unique colours in frame: $COLOURS"
      [ "${COLOURS:-0}" -ge 200 ] && g ok S4-render || { g no S4-render; echo "frame too flat ($COLOURS colours) — panel likely did not paint"; }
    else
      g no S4-render
    fi

    kill "$QS_PID" 2>/dev/null || true
    swaymsg exit >/dev/null 2>&1 || true
    echo "----- surface log (tail) -----"; tail -25 /tmp/qs.log
    echo "=================== S4 NETWORK-ACCESS RENDER RESULT ==================="
    echo "RENDER_PASS=$P RENDER_FAIL=$F"
    [ "$F" = 0 ]
  '
RC=$?
set -e
echo "=================== ADR-009 S4 PROOF RESULT ==================="
echo "render docker rc=$RC"
[ "$RC" = 0 ]
