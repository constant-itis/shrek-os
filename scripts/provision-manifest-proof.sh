#!/usr/bin/env bash
# Host proof for the LIVE-side provisioning manifest staging (ADR-005 §6.1-3, §5, §11 host oracle).
#
# Runs the real helper (layers/shrek-installer/overlay/usr/libexec/shrek/shrek-provision-stage) via its
# SHREK_PROVISION_STAGE_DIR seam against fixture enum sources, as an ordinary non-root user with no VM, and
# asserts the schema/validation contract:
#   - valid intent stages a sorted key=value manifest with schema_version + accepted keys, no fault;
#   - a keymap outside the shipped XKB layout set is DROPPED with a legible fault (fail-open-to-default);
#   - a locale outside the installed set is DROPPED with a fault;
#   - the display name is sanitized (ESC/CSI + control + ':' stripped, length capped) — never rejected;
#   - a schema_version mismatch rejects the WHOLE manifest -> only schema_version staged (all defaults);
#   - the staged manifest is 0600 (not group/world readable) and key=value/LF/sorted;
#   - a collect FILE is authoritative over env.
#
# The root:root chowns and the real /run tmpfs path need root; they are covered by the sealed-VM dogfood.
# This is the fast, root-free, build-free unit gate (mirrors scripts/owner-provision-proof.sh).
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

HELPER="layers/shrek-installer/overlay/usr/libexec/shrek/shrek-provision-stage"
[ -x "$HELPER" ] || { echo "FAIL: $HELPER not executable"; exit 1; }

PASS=0; FAIL=0
ok()  { echo "  PASS $*"; PASS=$((PASS+1)); }
bad() { echo "  FAIL $*"; FAIL=$((FAIL+1)); }

SCRATCH="$(mktemp -d)"; trap 'rm -rf "$SCRATCH"' EXIT

# ---- fixtures: a hermetic XKB symbols dir + a supported-locale allowlist (no dependency on host data) ----
XKB="$SCRATCH/xkb"; mkdir -p "$XKB"; : > "$XKB/us"; : > "$XKB/de"; : > "$XKB/fr"
LOCS="$SCRATCH/locales"; printf '%s\n' "C.UTF-8" "en_US.utf8" "de_DE.utf8" > "$LOCS"
export SHREK_XKB_SYMBOLS_DIR="$XKB" SHREK_LOCALE_ALLOWLIST="$LOCS"

# run the helper into a fresh stage dir; echoes the stage dir
run() {  # run TAG  (intent via SHREK_PROV_* / SHREK_PROVISION_COLLECT_FILE already exported by caller)
    local dir="$SCRATCH/stage-$1"; rm -rf "$dir"
    SHREK_PROVISION_STAGE_DIR="$dir" "$REPO_ROOT/$HELPER" >/dev/null 2>&1 || { echo "HELPER-EXIT-NONZERO"; return; }
    printf '%s' "$dir"
}
mval() { getk() { awk -F= -v k="$1" '$1==k{print substr($0,length($1)+2)}' "$2"; }; getk "$2" "$1/manifest"; }
has_key() { grep -q "^$2=" "$1/manifest"; }

# ======================================================================================================
echo "=== 1. valid intent: all keys accepted, sorted, no fault ==="
( export SHREK_PROV_LOCALE="en_US.UTF-8" SHREK_PROV_KEYMAP="de" SHREK_PROV_OWNER_NAME="Ogre King"
  unset SHREK_PROVISION_COLLECT_FILE SHREK_PROV_SCHEMA_VERSION 2>/dev/null || true
  D="$(run valid)"
  [ "$(mval "$D" schema_version)" = "1" ] && ok "schema_version=1" || bad "schema_version"
  [ "$(mval "$D" locale)" = "en_US.UTF-8" ] && ok "locale accepted" || bad "locale ($(mval "$D" locale))"
  [ "$(mval "$D" keymap)" = "de" ] && ok "keymap accepted (XKB layout)" || bad "keymap ($(mval "$D" keymap))"
  [ "$(mval "$D" owner_display_name)" = "Ogre King" ] && ok "name accepted" || bad "name ($(mval "$D" owner_display_name))"
  [ ! -f "$D/fault" ] && ok "no fault file on clean input" || bad "fault present: $(cat "$D/fault")"
  # sorted + key=value/LF
  if LC_ALL=C sort -c "$D/manifest" 2>/dev/null; then ok "manifest is sorted"; else bad "manifest not sorted"; fi
  if grep -Evq '^[a-z_]+=' "$D/manifest"; then bad "non key=value line present"; else ok "every line is key=value"; fi
  PASS_1=$PASS; FAIL_1=$FAIL; echo "$PASS $FAIL" > "$SCRATCH/c1" )
read PASS FAIL < "$SCRATCH/c1"

echo "=== 2. keymap outside the XKB set is dropped with a fault (fail-open-to-default) ==="
( export SHREK_PROV_LOCALE="en_US.UTF-8" SHREK_PROV_KEYMAP="zz9" SHREK_PROV_OWNER_NAME="Donkey"
  D="$(run badkm)"
  has_key "$D" keymap && bad "invalid keymap should be dropped" || ok "invalid keymap dropped"
  has_key "$D" locale && ok "valid locale still present" || bad "locale wrongly dropped"
  [ -f "$D/fault" ] && grep -q "keymap:" "$D/fault" && ok "fault names keymap" || bad "no keymap fault"
  echo "$PASS $FAIL" > "$SCRATCH/c2" )
read PASS FAIL < "$SCRATCH/c2"

echo "=== 3. locale outside the installed set is dropped with a fault ==="
( export SHREK_PROV_LOCALE="xx_XX.UTF-8" SHREK_PROV_KEYMAP="us" SHREK_PROV_OWNER_NAME="Fiona"
  D="$(run badloc)"
  has_key "$D" locale && bad "invalid locale should be dropped" || ok "invalid locale dropped"
  has_key "$D" keymap && ok "valid keymap still present" || bad "keymap wrongly dropped"
  [ -f "$D/fault" ] && grep -q "locale:" "$D/fault" && ok "fault names locale" || bad "no locale fault"
  echo "$PASS $FAIL" > "$SCRATCH/c3" )
read PASS FAIL < "$SCRATCH/c3"

echo "=== 4. display name sanitized (ESC/CSI + control + colon stripped, length capped) ==="
( # name with an ESC-CSI sequence, a control char, a colon, and > NAME_MAX chars
  printf -v NM '\033[31mBig%sOgre:Lord\t%s' "$(printf '\033')" "$(head -c 80 </dev/zero | tr '\0' 'x')"
  export SHREK_PROV_LOCALE="en_US.UTF-8" SHREK_PROV_KEYMAP="us" SHREK_PROV_OWNER_NAME="$NM"
  export SHREK_PROVISION_NAME_MAX="64"
  D="$(run sani)"
  V="$(mval "$D" owner_display_name)"
  if printf '%s' "$V" | LC_ALL=C grep -q $'\033'; then bad "ESC survived in name"; else ok "ESC stripped"; fi
  case "$V" in *:*) bad "colon survived in name";; *) ok "colon stripped";; esac
  if printf '%s' "$V" | LC_ALL=C grep -Pq '[\x00-\x1f\x7f]'; then bad "control byte survived"; else ok "control bytes stripped"; fi
  [ "${#V}" -le 64 ] && ok "length capped (${#V} <= 64)" || bad "length not capped (${#V})"
  [ -f "$D/fault" ] && grep -q "owner_display_name: sanitized" "$D/fault" && ok "sanitize recorded in fault" || bad "no sanitize fault"
  echo "$PASS $FAIL" > "$SCRATCH/c4" )
read PASS FAIL < "$SCRATCH/c4"

echo "=== 5. schema_version mismatch rejects the whole manifest (all defaults) ==="
( export SHREK_PROV_LOCALE="en_US.UTF-8" SHREK_PROV_KEYMAP="de" SHREK_PROV_OWNER_NAME="Ogre" SHREK_PROV_SCHEMA_VERSION="99"
  D="$(run badschema)"
  [ "$(mval "$D" schema_version)" = "1" ] && ok "manifest carries expected schema_version=1" || bad "schema_version"
  if has_key "$D" locale || has_key "$D" keymap || has_key "$D" owner_display_name; then
      bad "data keys present despite schema mismatch"; else ok "no data keys (all default)"; fi
  [ -f "$D/fault" ] && grep -q "whole manifest rejected" "$D/fault" && ok "fault explains whole-manifest reject" || bad "no whole-reject fault"
  echo "$PASS $FAIL" > "$SCRATCH/c5" )
read PASS FAIL < "$SCRATCH/c5"

echo "=== 6. staged manifest is 0600 (not group/world readable) ==="
( export SHREK_PROV_LOCALE="en_US.UTF-8" SHREK_PROV_KEYMAP="us" SHREK_PROV_OWNER_NAME="Ogre"
  D="$(run perms)"
  M="$(stat -c '%a' "$D/manifest")"
  [ "$M" = "600" ] && ok "manifest mode 0600" || bad "manifest mode $M (want 600)"
  echo "$PASS $FAIL" > "$SCRATCH/c6" )
read PASS FAIL < "$SCRATCH/c6"

echo "=== 7. collect FILE is authoritative over env ==="
( CF="$SCRATCH/collect.env"
  printf '%s\n' "schema_version=1" "locale=de_DE.UTF-8" "keymap=fr" "owner_display_name=Shrek" > "$CF"
  export SHREK_PROVISION_COLLECT_FILE="$CF"
  export SHREK_PROV_LOCALE="en_US.UTF-8" SHREK_PROV_KEYMAP="us" SHREK_PROV_OWNER_NAME="EnvName"  # should be ignored
  D="$(run collectfile)"
  [ "$(mval "$D" locale)" = "de_DE.UTF-8" ] && ok "locale from file wins" || bad "locale ($(mval "$D" locale))"
  [ "$(mval "$D" keymap)" = "fr" ] && ok "keymap from file wins" || bad "keymap ($(mval "$D" keymap))"
  [ "$(mval "$D" owner_display_name)" = "Shrek" ] && ok "name from file wins" || bad "name ($(mval "$D" owner_display_name))"
  echo "$PASS $FAIL" > "$SCRATCH/c7" )
read PASS FAIL < "$SCRATCH/c7"

echo "=== 8. empty intent: only schema_version, no fault (target defaults everything) ==="
( unset SHREK_PROV_LOCALE SHREK_PROV_KEYMAP SHREK_PROV_OWNER_NAME SHREK_PROVISION_COLLECT_FILE SHREK_PROV_SCHEMA_VERSION 2>/dev/null || true
  D="$(run empty)"
  [ "$(mval "$D" schema_version)" = "1" ] && ok "schema_version=1" || bad "schema_version"
  if has_key "$D" locale || has_key "$D" keymap || has_key "$D" owner_display_name; then bad "unexpected data key"; else ok "no data keys"; fi
  [ ! -f "$D/fault" ] && ok "no fault on empty (not an error to omit optional intent)" || bad "fault on empty: $(cat "$D/fault")"
  echo "$PASS $FAIL" > "$SCRATCH/c8" )
read PASS FAIL < "$SCRATCH/c8"

echo
echo "==================================================================="
echo "PROVISION-MANIFEST-PROOF: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ] && echo "RESULT: GREEN" || { echo "RESULT: RED"; exit 1; }
