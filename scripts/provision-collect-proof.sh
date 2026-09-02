#!/usr/bin/env bash
# Fast host proof for ADR-005 §6 collect step — shrek-provision-collect (the Quickshell installer's
# file-legible bridge to shrek-provision-stage). Root-free, no VM.
#
# Asserts: the helper writes a well-formed key=value collect file (0600) from ARGV; a newline in the
# untrusted name cannot forge a second key; an empty name omits the line; it chains shrek-provision-stage
# (producing the staged manifest) and honors RUN_STAGE=0; and the EraseConfirm QML invokes it by ARGV.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

H=layers/shrek-installer/overlay/usr/libexec/shrek/shrek-provision-collect
STAGE=layers/shrek-installer/overlay/usr/libexec/shrek/shrek-provision-stage
ERASE=ui-installer/ui/EraseConfirm.qml

PASS=0; FAIL=0
ok()  { echo "  PASS $*"; PASS=$((PASS+1)); }
bad() { echo "  FAIL $*"; FAIL=$((FAIL+1)); }
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT

echo "=== helper shape ==="
[ -x "$H" ] && ok "collect helper is executable" || bad "collect helper not executable"
{ [ -f "$H" ] && ! git check-ignore -q "$H"; } && ok "collect helper is tracked source" || bad "collect helper missing/gitignored"
sh -n "$H" && ok "collect helper parses (sh -n)" || bad "collect helper sh -n failed"

echo "=== 1. collect file: keys, mode, ordering ==="
C="$TMP/collect"
SHREK_PROVISION_COLLECT_FILE="$C" SHREK_PROVISION_COLLECT_RUN_STAGE=0 sh "$H" --locale en_US.UTF-8 --keymap us --name "Sebastian" >/dev/null 2>&1 && ok "collect (stage-skipped) exits 0" || bad "collect errored"
[ "$(stat -c %a "$C" 2>/dev/null)" = 600 ] && ok "collect file mode 0600" || bad "collect file mode=$(stat -c %a "$C" 2>/dev/null)"
grep -qx 'schema_version=1' "$C"        && ok "schema_version=1 present"          || bad "schema_version missing"
grep -qx 'locale=en_US.UTF-8' "$C"      && ok "locale line written"               || bad "locale line missing"
grep -qx 'keymap=us' "$C"               && ok "keymap line written"               || bad "keymap line missing"
grep -qx 'owner_display_name=Sebastian' "$C" && ok "owner_display_name line written" || bad "owner_display_name missing"

echo "=== 2. untrusted name cannot forge a key (newline stripped) ==="
Cinj="$TMP/inj"
SHREK_PROVISION_COLLECT_FILE="$Cinj" SHREK_PROVISION_COLLECT_RUN_STAGE=0 sh "$H" --locale en_US.UTF-8 --keymap de --name "$(printf 'Evil\nkeymap=us')" >/dev/null 2>&1
[ "$(grep -c '^keymap=' "$Cinj")" = 1 ] && ok "exactly one keymap line (no forged key from the name)" || bad "name newline forged an extra key"
grep -q '^owner_display_name=Evilkeymap=us$' "$Cinj" && ok "name newline collapsed onto one line" || bad "name control-strip did not collapse the newline"

echo "=== 3. empty name omits the line ==="
Cempty="$TMP/empty"
SHREK_PROVISION_COLLECT_FILE="$Cempty" SHREK_PROVISION_COLLECT_RUN_STAGE=0 sh "$H" --locale en_US.UTF-8 --keymap us >/dev/null 2>&1
grep -q '^owner_display_name=' "$Cempty" && bad "empty name still wrote owner_display_name" || ok "empty name omits owner_display_name"

echo "=== 4. chains stage -> staged manifest (and RUN_STAGE=0 does not) ==="
Cs="$TMP/cs"; SDIR="$TMP/stage"
SHREK_PROVISION_COLLECT_FILE="$Cs" SHREK_PROVISION_STAGE_BIN="$STAGE" SHREK_PROVISION_STAGE_DIR="$SDIR" \
  SHREK_XKB_SYMBOLS_DIR=/usr/share/X11/xkb/symbols sh "$H" --locale en_US.UTF-8 --keymap us --name "Sebastian" >/dev/null 2>&1
[ -f "$SDIR/manifest" ] && grep -qx 'owner_display_name=Sebastian' "$SDIR/manifest" && ok "collect chains stage -> staged manifest carries the intent" || bad "stage was not produced from the collect file"
Cn="$TMP/cn"; SDIR2="$TMP/stage2"
SHREK_PROVISION_COLLECT_FILE="$Cn" SHREK_PROVISION_COLLECT_RUN_STAGE=0 SHREK_PROVISION_STAGE_DIR="$SDIR2" sh "$H" --locale en_US.UTF-8 --keymap us --name x >/dev/null 2>&1
[ ! -e "$SDIR2/manifest" ] && ok "RUN_STAGE=0 writes the collect file only (no stage)" || bad "RUN_STAGE=0 still ran stage"

echo "=== 5. EraseConfirm QML invokes the helper by ARGV ==="
grep -q '/usr/libexec/shrek/shrek-provision-collect' "$ERASE" && ok "EraseConfirm calls shrek-provision-collect" || bad "EraseConfirm does not call the collect helper"
grep -Eq '"--name",[[:space:]]*Intent.ownerName' "$ERASE" && ok "EraseConfirm passes the name as an argv element (not a shell string)" || bad "EraseConfirm does not pass the name as argv"

echo
echo "=== provision-collect-proof: PASS=$PASS FAIL=$FAIL ==="
[ "$FAIL" -eq 0 ]
