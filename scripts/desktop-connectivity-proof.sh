#!/usr/bin/env bash
# ADR-007 S3 — desktop-egress bless UX proof.
#
# Proves the two halves of the S3 bless UX, split because one needs a real socket and the other a real
# compositor:
#
#   HOST (wired): the exact `egressd ask` client the UI execs actually drives the S2 supervisor over the
#   socket, and the intent-first bless + the legible /run/shrek/egress/state projection behave —
#   including the "blessed, waiting for network" state a first-run bless leaves when the resolver is
#   unreachable. Uses the natively-built egressd (oracle-env) + a temp store/socket.
#
#   RENDER (container): the real ui-v2 Connectivity panel + the Egress service load and paint under a
#   headless Sway + the cached Quickshell, reading a SEEDED state projection (no daemon needed for the
#   read path). We open the panel over IPC, grep the load-bearing markers, and grim a real frame.
#
# Gates (each independent so we learn WHERE it breaks):
#   CONN-ask-status   `egressd ask status` round-trips OK against a live supervisor
#   CONN-intent-first a bless whose resolve fails leaves blessed=1 pins=- fault=resolve-fail (pending)
#   CONN-ask-deny     `egressd ask bless web-browsing` is refused at the socket (ceremony tier)
#   CONN-surfaces     the shell instantiates with the seeded projection present
#   CONN-state        the Egress service parses the projection (profiles=4) file -> service -> panel
#   CONN-panel        opening the Connectivity panel over IPC raises no QML load error
#   CONN-render       a real frame paints (unique-colour count over threshold)
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

BASE="out/desktop-connectivity-proof"
RUN="$BASE/run"
rm -rf "$BASE"; mkdir -p "$RUN"

PASS=0; FAIL=0
gate() { if [ "$1" = ok ]; then echo "SHREK_GATE: PASS $2"; PASS=$((PASS+1)); else echo "SHREK_GATE: FAIL $2"; FAIL=$((FAIL+1)); fi; }

# ── HOST (wired): the real client ↔ supervisor socket path ────────────────────────────────────────
echo "=== building egressd (release, oracle-env) ==="
CARGO_NET_OFFLINE=true cargo build --release -p egressd --features oracle-env >/dev/null 2>&1 || \
  cargo build --release -p egressd --features oracle-env
BIN="target/release/egressd"

HSTORE="$BASE/host-store"; HRUN="$BASE/host-run"
export SHREK_EGRESS_STORE="$(readlink -f "$BASE")/host-store"
export SHREK_EGRESS_RUN="$(readlink -f "$BASE")/host-run"
export SHREK_EGRESS_SOCK="$(readlink -f "$BASE")/host-run/sock"
export SHREK_EGRESS_DESKTOP_UID="$(id -u)"
mkdir -p "$HRUN"
"$BIN" store init >/dev/null

# A live supervisor. nft applies fail here (no baked table / netns) — irrelevant to the wired assertions,
# which are about the socket round-trip + the intent-first store/projection behaviour.
"$BIN" daemon >"$BASE/daemon.log" 2>&1 &
DPID=$!
cleanup() { kill "$DPID" 2>/dev/null || true; wait "$DPID" 2>/dev/null || true; }
trap cleanup EXIT
for i in $(seq 1 60); do [ -S "$SHREK_EGRESS_SOCK" ] && break; sleep 0.05; done

echo "--- CONN-ask-status ---"
# Capture then grep: `ask` maps ERR->exit 1, and pipefail would misread a matched grep as a failure.
STATUS_OUT="$("$BIN" ask status 2>&1 || true)"
if echo "$STATUS_OUT" | grep -q "^OK status"; then gate ok CONN-ask-status; else gate no CONN-ask-status; echo "  got: $STATUS_OUT"; fi

echo "--- CONN-intent-first (bless with resolver unreachable -> pending, not lost) ---"
# The sealed resolver IPs are unreachable from this box for a real DoT handshake in most CI/dev nets, so
# the bless persists intent + parks resolve-fail. (If the resolve unexpectedly succeeds, the state shows
# blessed=1 with pins present — still a pass for "blessed, not lost".)
"$BIN" ask bless weather >/dev/null 2>&1 || true
sleep 0.3
STATE_LINE="$(grep '^profile weather ' "$HRUN/state" 2>/dev/null || true)"
echo "  weather: $STATE_LINE"
if echo "$STATE_LINE" | grep -q "blessed=1"; then gate ok CONN-intent-first; else gate no CONN-intent-first; fi

echo "--- CONN-ask-deny (web-browsing is ceremony-tier, refused at the socket) ---"
DENY_OUT="$("$BIN" ask bless web-browsing 2>&1 || true)"
if echo "$DENY_OUT" | grep -q "^ERR denied"; then gate ok CONN-ask-deny; else gate no CONN-ask-deny; echo "  got: $DENY_OUT"; fi

cleanup; trap - EXIT

# ── seed the RENDER projection (deterministic; no daemon needed for the read path) ────────────────
# A rich, representative state: baseline on (ntp with its sealed literal IPs), weather blessed + LIVE,
# web-browsing ceremony/unblessed. Plus one downstream event line for the "last activity" surface.
cat > "$RUN/state" <<'STATE'
schema shrek-egress-state/1
profile desktop-ntp tier=baseline blessed=0 pins=162.159.200.1,162.159.200.123 refreshed=- fault=-
profile desktop-updates tier=baseline blessed=0 pins=- refreshed=- fault=-
profile weather tier=one-click blessed=1 pins=104.18.5.99 refreshed=1750000000 fault=-
profile web-browsing tier=ceremony blessed=0 pins=- refreshed=- fault=-
STATE
printf '1750000000 bless weather 1 ip(s)\n' > "$RUN/events"

CACHE=out/qs-cache
if [ ! -x "$CACHE/quickshell" ]; then
  echo "!!! no cached Quickshell at $CACHE/quickshell — run scripts/qml-check.sh first (one-time build) !!!"
  echo "PASS=$PASS FAIL=$((FAIL+1)) (render half skipped)"
  exit 1
fi

# ── RENDER (container): real ui-v2 panel under headless Sway + cached Quickshell ──────────────────
echo "=== Connectivity render proof (cached Quickshell) in debian:trixie ==="
set +e
docker run --rm --privileged -v "${REPO_ROOT}:/work" -w /work -e CACHE="${CACHE}" \
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
    export SHREK_EGRESS_RUN=/work/out/desktop-connectivity-proof/run

    sway -c /work/layers/shrek-desktop/overlay/usr/share/shrek/desktop/sway.config >/tmp/sway.log 2>&1 &
    for i in $(seq 1 30); do swaymsg -t get_version >/dev/null 2>&1 && break; sleep 1; done
    swaymsg -t get_version >/dev/null 2>&1 || { echo "NOTE sway failed"; sed -n 1,30p /tmp/sway.log; }
    WD="$(ls "$XDG_RUNTIME_DIR"/wayland-* 2>/dev/null | grep -v "\.lock$" | head -1)"; WD="$(basename "${WD:-wayland-1}")"

    # launch the shell (long-lived), then drive it over IPC (a second quickshell invocation)
    WAYLAND_DISPLAY="$WD" QT_QPA_PLATFORM=wayland QT_QUICK_BACKEND=software SHREK_EGRESS_RUN="$SHREK_EGRESS_RUN" \
      "/work/$CACHE/quickshell" -p /work/ui-v2/shell.qml >/tmp/qs.log 2>&1 &
    QS_PID=$!
    for i in $(seq 1 20); do grep -q "SHREK-DESKTOP shell surfaces instantiated" /tmp/qs.log && break; sleep 0.5; done

    grep -q "SHREK-DESKTOP shell surfaces instantiated" /tmp/qs.log && g ok CONN-surfaces \
      || { g no CONN-surfaces; sed -n 1,40p /tmp/qs.log; }

    # open the Connectivity panel over IPC and let it paint — this lazily instantiates ConnectivityPage
    # (and therefore the Egress singleton), so the state marker + the panel render appear only now.
    WAYLAND_DISPLAY="$WD" QT_QPA_PLATFORM=wayland \
      "/work/$CACHE/quickshell" -p /work/ui-v2/shell.qml ipc call ui connectivity >/dev/null 2>&1 \
      || echo "WARN ipc connectivity failed"
    sleep 3

    grep -q "SHREK-DESKTOP connectivity egress state profiles=4" /tmp/qs.log && g ok CONN-state \
      || { g no CONN-state; grep -iE "connectivity|egress|error|SHREK-DESKTOP" /tmp/qs.log | head; }

    if grep -q "Failed to load configuration" /tmp/qs.log; then
      g no CONN-panel; grep -A6 "Failed to load configuration" /tmp/qs.log
    else
      g ok CONN-panel
    fi

    mkdir -p /work/out/desktop-connectivity-proof/frame
    FRAME=/work/out/desktop-connectivity-proof/frame/connectivity.png
    WAYLAND_DISPLAY="$WD" grim "$FRAME" 2>/tmp/grim.log || echo "WARN grim failed: $(cat /tmp/grim.log)"
    if [ -f "$FRAME" ]; then
      COLOURS="$(convert "$FRAME" -format "%k" info: 2>/dev/null || echo 0)"
      echo "unique colours in frame: $COLOURS"
      [ "${COLOURS:-0}" -ge 200 ] && g ok CONN-render || { g no CONN-render; echo "frame too flat ($COLOURS colours) — panel likely did not paint"; }
    else
      g no CONN-render
    fi

    kill "$QS_PID" 2>/dev/null || true
    swaymsg exit >/dev/null 2>&1 || true
    echo "----- shell log (tail) -----"; tail -25 /tmp/qs.log
    echo "=================== CONNECTIVITY RENDER RESULT ==================="
    echo "RENDER_PASS=$P RENDER_FAIL=$F"
    [ "$F" = 0 ]
  '
RENDER_RC=$?
set -e

echo "=================== CONNECTIVITY PROOF RESULT ==================="
echo "host-wired: PASS=$PASS FAIL=$FAIL ; render docker rc=$RENDER_RC"
[ "$FAIL" = 0 ] && [ "$RENDER_RC" = 0 ]
