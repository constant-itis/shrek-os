#!/usr/bin/env bash
# Shrek OS — ADR-009 v2 boot-lag plan C: host oracle for `shrek-boot-toast`, the egress warm-up
# reassurance notification. Drives the launcher logic with a FAKE notify-send (logs every call + returns
# an id) and temp state/hosts files, so no live Wayland/DBus session is needed (mirrors the shrek-connect
# SHREK_HOSTS override pattern). Proves the four decision branches:
#
#   T-nobless        weather NOT blessed  ⇒ no toast at all (nothing is auto-connecting)
#   T-alreadylive    weather blessed AND its host already in /etc/hosts ⇒ no toast (already connected)
#   T-connect-lift   weather blessed, host absent → the "Connecting…" toast fires, and when the host lands
#                    in /etc/hosts it is REPLACED IN PLACE (same id) by "Services connected"
#   T-timeout        weather blessed, host never lands ⇒ "Connecting…" then a gentle "Still connecting…"
#                    replacement (never "connected", never a hung toast)
set -uo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

PASS=0; FAIL=0
check() { if [ "$3" -eq 0 ]; then echo "  PASS $1 — $2"; PASS=$((PASS+1)); else echo "  FAIL $1 — $2"; FAIL=$((FAIL+1)); fi; }

TOAST="$REPO_ROOT/layers/shrek-desktop/overlay/usr/bin/shrek-boot-toast"
[ -x "$TOAST" ] || { echo "FAIL: $TOAST not executable"; exit 1; }
bash -n "$TOAST" || { echo "FAIL: shrek-boot-toast has a syntax error"; exit 1; }

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
STATE="$WORK/state"; HOSTS="$WORK/hosts"; LOG="$WORK/notify.log"

# A fake notify-send: append the call to the log; emit an id when --print-id is asked (so the script
# captures ID and uses --replace-id on the resolution).
cat > "$WORK/fake-notify" <<'EOF'
#!/bin/sh
echo "notify $*" >> "$NOTIFY_LOG"
case " $* " in *" --print-id "*) echo 42 ;; esac
exit 0
EOF
chmod +x "$WORK/fake-notify"

export SHREK_EGRESS_STATE="$STATE" SHREK_HOSTS="$HOSTS" NOTIFY_LOG="$LOG"
export SHREK_NOTIFY="$WORK/fake-notify" SHREK_BOOT_TOAST_SKIP_WAIT=1
BLESSED_PENDING='profile weather tier=one-click blessed=1 pins=- refreshed=- fault=-'
NOTBLESSED='profile weather tier=one-click blessed=0 pins=- refreshed=- fault=-'
WEATHER_LINE='104.16.1.1 api.open-meteo.com'

echo "=== 1. weather not blessed ⇒ silent ==="
: > "$LOG"; printf '%s\n' "$NOTBLESSED" > "$STATE"; printf 'baseline\n' > "$HOSTS"
"$TOAST"
[ ! -s "$LOG" ]
check "T-nobless" "no toast when weather is not blessed" $?

echo "=== 2. weather blessed AND already delivered ⇒ silent ==="
: > "$LOG"; printf '%s\n' "$BLESSED_PENDING" > "$STATE"; printf '%s\n' "$WEATHER_LINE" > "$HOSTS"
"$TOAST"
[ ! -s "$LOG" ]
check "T-alreadylive" "no toast when weather host already in /etc/hosts" $?

echo "=== 3. blessed + pending → connecting, then replaced in place when the host lands ==="
: > "$LOG"; printf '%s\n' "$BLESSED_PENDING" > "$STATE"; printf 'baseline\n' > "$HOSTS"
# inject the weather host mid-poll (models egressd's reconcile recompose landing during the warm-up)
( sleep 2; printf '%s\n' "$WEATHER_LINE" >> "$HOSTS" ) &
SHREK_BOOT_TOAST_POLL=1 SHREK_BOOT_TOAST_TIMEOUT=10 "$TOAST"
wait
grep -q 'Connecting to services' "$LOG" \
  && grep -q 'Services connected' "$LOG" \
  && grep -q -- '--replace-id=42.*Services connected\|Services connected' "$LOG" \
  && ! grep -q 'Still connecting' "$LOG"
check "T-connect-lift" "connecting toast fires, then replaced in place by 'connected' when host lands" $?
# the resolution must reuse the connecting toast's id (in-place replace, no pile-up)
grep 'Services connected' "$LOG" | grep -q -- '--replace-id=42'
check "T-connect-replace-id" "'connected' replaces the same notification id (--replace-id=42)" $?

echo "=== 4. blessed + never delivered ⇒ connecting, then gentle 'still connecting' ==="
: > "$LOG"; printf '%s\n' "$BLESSED_PENDING" > "$STATE"; printf 'baseline\n' > "$HOSTS"
SHREK_BOOT_TOAST_POLL=1 SHREK_BOOT_TOAST_TIMEOUT=2 "$TOAST"
grep -q 'Connecting to services' "$LOG" \
  && grep -q 'Still connecting' "$LOG" \
  && grep 'Still connecting' "$LOG" | grep -q -- '--replace-id=42' \
  && ! grep -q 'Services connected' "$LOG"
check "T-timeout" "on timeout, connecting toast is replaced in place by a gentle note (not 'connected')" $?

echo "=== $PASS passed, $FAIL failed ==="
[ "$FAIL" -eq 0 ]
