#!/usr/bin/env bash
# Verify the ADR-009 v2 boot-lag plan-B DMS weather retry-cadence patch, host-side/static:
# the vendored Services/WeatherService.qml raises maxRetryAttempts 3 -> 10 so DMS keeps its fast 30s
# retry cadence across the whole egress warm-up (~5 min) instead of dropping into a 60s->120s->240s
# backoff mid-window (which left weather "looking broken" until ~t=150s). ONE value change; everything
# else byte-identical to upstream. Also enforces the DRIFT guard (pinned to the dms= package version)
# and the two invariants that keep the change SAFE:
#   - retryDelay stays 30000 == minFetchInterval, so every retry clears fetchWeather's throttle
#   - the persistent exponential backoff is untouched (steady-state / genuinely-offline behavior intact)
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MKOSI=$ROOT/layers/shrek-desktop/mkosi.conf
WS=$ROOT/layers/shrek-desktop/overlay/usr/share/quickshell/dms/Services/WeatherService.qml
SWAY=$ROOT/layers/shrek-desktop/overlay/usr/share/shrek/desktop/sway.config

pass=0; fail=0
chk() { if [ "$2" = "$3" ]; then echo "  PASS $1 ($2)"; pass=$((pass+1)); else echo "  FAIL $1 exp=[$3] got=[$2]"; fail=$((fail+1)); fi; }
has() { if grep -qF "$2" "$1"; then echo "  PASS $3"; pass=$((pass+1)); else echo "  FAIL $3 (missing: $2)"; fail=$((fail+1)); fi; }

echo "== pin: read dms= from mkosi.conf =="
DMS_PIN=$(sed -n 's/^[[:space:]]*dms=\([0-9A-Za-z.]*\).*/\1/p' "$MKOSI" | head -1)
chk pin-nonempty "$([ -n "$DMS_PIN" ] && echo yes || echo no)" "yes"
echo "  (dms pin=$DMS_PIN)"

echo "== vendored file exists at DMS's real -c load path =="
chk ws-exists "$([ -f "$WS" ] && echo yes || echo no)" "yes"

echo "== the patch: maxRetryAttempts 3 -> 10 (and stock value gone) =="
chk ws-patched-count   "$(grep -c 'property int maxRetryAttempts: 10' "$WS")" "1"
chk ws-stock-absent    "$(grep -c 'property int maxRetryAttempts: 3$' "$WS")" "0"

echo "== SAFETY invariant 1: retryDelay unchanged AND == minFetchInterval (retries clear the throttle) =="
chk ws-retrydelay      "$(grep -c 'property int retryDelay: 30000' "$WS")" "1"
chk ws-minfetch        "$(grep -c 'property int minFetchInterval: 30000' "$WS")" "1"

echo "== SAFETY invariant 2: persistent exponential backoff untouched (steady-state intact) =="
has "$WS" '60000 * Math.pow(2, persistentRetryCount)' "ws-backoff-formula-intact"
has "$WS" 'root.retryAttempts < root.maxRetryAttempts' "ws-retry-gate-intact"
has "$WS" 'function handleWeatherSuccess' "ws-success-reset-intact"

echo "== WIRING: dms is launched against the patched on-disk tree (else the overlay is inert) =="
has "$SWAY" "dms run -c /usr/share/quickshell/dms" "sway-dms-points-at-patched-tree"
chk ws-at-c-path "$([ "$WS" = "$ROOT/layers/shrek-desktop/overlay/usr/share/quickshell/dms/Services/WeatherService.qml" ] && echo yes || echo no)" "yes"

echo "== drift guard: overlay header is pinned to the mkosi dms version =="
has "$WS" "dms=$DMS_PIN" "ws-header-names-pin"
has "$WS" "SHREK-OS OVERLAY" "ws-header-provenance"

echo "== body intact: the retry/timer machinery this patch depends on is all present =="
# (a naive brace count is useless here — the JS body is full of {} inside strings/format literals/URLs;
#  instead anchor on the distinctive constructs, so a truncated or mangled vendored copy fails loudly.)
has "$WS" "function fetchWeather"          "ws-body-fetchWeather"
has "$WS" "function handleWeatherFailure"  "ws-body-failure-handler"
has "$WS" "id: retryTimer"                 "ws-body-retryTimer"
has "$WS" "id: persistentRetryTimer"       "ws-body-persistentRetryTimer"
has "$WS" "id: updateTimer"                "ws-body-updateTimer"
# exactly one line references the patched property's declaration (no stray duplicate crept in)
chk ws-single-decl "$(grep -c 'property int maxRetryAttempts' "$WS")" "1"

echo
echo "== $pass passed, $fail failed =="
[ "$fail" -eq 0 ]
