#!/usr/bin/env bash
# Verify the shrek-owned keyboard-layout control (fills the gap left by DMS's niriOnly Keyboard tab,
# which writes niri config sway ignores). Host-side/static, with a fake swaymsg + temp store/vconsole:
#   CLI (shrek-keyboard):
#     K-show-default   with no override, `--show` reports the provisioned default (vconsole XKBLAYOUT)
#     K-set            `shrek-keyboard de` persists "de" to the uid-1000 store AND live-applies via swaymsg
#     K-set-list       an xkb list `us,de` is accepted (comma in the closed charset)
#     K-override-wins  once set, the stored override wins over the provisioned default in `--show`
#     K-reject         a shell-injection / garbage layout is refused (exit 2), store + swaymsg untouched
#     K-reset          `--reset` drops the override and live-applies the provisioned default
#   LAUNCHER (shrek-desktop) + sway.config:
#     L-reads-override the launcher reads the uid-1000 override file and exports XKB_DEFAULT_LAYOUT
#     L-charset-guard  it guards the same closed charset (never eval/source the file)
#     L-no-sway-xkb    sway.config sets NO xkb_layout override (so the env stays authoritative)
set -uo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

PASS=0; FAIL=0
check() { if [ "$3" -eq 0 ]; then echo "  PASS $1 — $2"; PASS=$((PASS+1)); else echo "  FAIL $1 — $2"; FAIL=$((FAIL+1)); fi; }

KB="$REPO_ROOT/layers/shrek-desktop/overlay/usr/bin/shrek-keyboard"
LAUNCHER="$REPO_ROOT/layers/shrek-desktop/overlay/usr/bin/shrek-desktop"
SWAY="$REPO_ROOT/layers/shrek-desktop/overlay/usr/share/shrek/desktop/sway.config"
[ -x "$KB" ] || { echo "FAIL: $KB not executable"; exit 1; }
sh -n "$KB" || { echo "FAIL: shrek-keyboard syntax error"; exit 1; }
sh -n "$LAUNCHER" || { echo "FAIL: shrek-desktop syntax error"; exit 1; }

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
STORE="$WORK/kb-layout"; VCON="$WORK/vconsole.conf"; SWAYLOG="$WORK/swaymsg.log"
printf 'XKBLAYOUT=us\n' > "$VCON"

# fake swaymsg: log `input ...` applies; answer `-t get_inputs` with a canned readout.
cat > "$WORK/swaymsg" <<'EOF'
#!/bin/sh
if [ "${1:-}" = "-t" ]; then printf '%s\n' '[{"xkb_active_layout_name": "English (US)"}]'; exit 0; fi
echo "swaymsg $*" >> "$SWAY_LOG"
exit 0
EOF
chmod +x "$WORK/swaymsg"

export SHREK_KB_STORE="$STORE" SHREK_VCONSOLE="$VCON" SHREK_SWAYMSG="$WORK/swaymsg" SWAY_LOG="$SWAYLOG"
run() { "$KB" "$@"; }

echo "=== CLI: shrek-keyboard ==="
: > "$SWAYLOG"; rm -f "$STORE"
# capture first: `grep -q` would SIGPIPE the multi-line --show under pipefail
out="$(run --show)"; printf '%s\n' "$out" | grep -q 'provisioned default: us'
check "K-show-default" "no override ⇒ show reports the provisioned default (us)" $?

: > "$SWAYLOG"
run de >/dev/null
{ [ "$(cat "$STORE")" = "de" ] && grep -q 'swaymsg input type:keyboard xkb_layout de' "$SWAYLOG"; }
check "K-set" "set 'de' persists to the store AND live-applies via swaymsg" $?

: > "$SWAYLOG"
run "us,de" >/dev/null
{ [ "$(cat "$STORE")" = "us,de" ] && grep -q 'swaymsg input type:keyboard xkb_layout us,de' "$SWAYLOG"; }
check "K-set-list" "an xkb layout list 'us,de' is accepted + applied" $?

out="$(run --show)"; printf '%s\n' "$out" | grep -q 'stored layout:      us,de'
check "K-override-wins" "the stored override wins over the provisioned default in --show" $?

: > "$SWAYLOG"; printf 'de\n' > "$STORE"
rc=0; run 'us; rm -rf /' >/dev/null 2>&1 || rc=$?
{ [ "$rc" = "2" ] && [ "$(cat "$STORE")" = "de" ] && ! grep -q 'rm -rf' "$SWAYLOG" && [ ! -s "$SWAYLOG" ]; }
check "K-reject" "a shell-injection layout is refused (exit 2); store + swaymsg untouched" $?

: > "$SWAYLOG"; printf 'de\n' > "$STORE"
run --reset >/dev/null
{ [ ! -e "$STORE" ] && grep -q 'swaymsg input type:keyboard xkb_layout us' "$SWAYLOG"; }
check "K-reset" "--reset drops the override AND live-applies the provisioned default (us)" $?

echo "=== LAUNCHER + sway.config wiring ==="
grep -q 'keyboard-layout' "$LAUNCHER" && grep -q 'XKB_DEFAULT_LAYOUT' "$LAUNCHER"
check "L-reads-override" "shrek-desktop reads the override file + exports XKB_DEFAULT_LAYOUT" $?

grep -q 'a-z0-9,_-' "$LAUNCHER"
check "L-charset-guard" "launcher guards the closed charset (no source/eval of the file)" $?

[ "$(grep -c 'xkb_layout' "$SWAY")" -eq 0 ]
check "L-no-sway-xkb" "sway.config sets NO xkb_layout override (env stays authoritative)" $?

echo "=== $PASS passed, $FAIL failed ==="
[ "$FAIL" -eq 0 ]
