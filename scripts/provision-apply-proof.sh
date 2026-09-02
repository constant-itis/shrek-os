#!/usr/bin/env bash
# Host proof for the TARGET-side provisioning appliers (ADR-005 §6.6-6.8, §11 host oracle).
#
# Runs the real gate (shrek-provision-validate) and the real applier (shrek-provision-seed) via their
# SHREK_PROVISION_STORE/RUN seams against fixture enum sources, as a NON-root user with no VM, and asserts:
#   GATE (§6.6): valid intent -> per-key /run files + sentinel, no fault; read-hardening (symlink / dup-key
#     / unknown-key / oversize / schema-mismatch) -> WHOLE-manifest reject with sentinel STILL fired + audit
#     copy; a bad single value -> that key omitted + fault, others survive; missing manifest -> clean default.
#   APPLIER three-state (§6.7): gate-complete+value -> seed state file + stamp (seed-once, no re-clobber);
#     gate-complete+no-value -> terminal-default stamp, NO state file; sentinel ABSENT -> neither stamp nor
#     seed (retry), a prior state file preserved.
#   RENDER: locale->LANG=, keymap->XKBLAYOUT= (+ KEYMAP baked default), timezone-> regular-file zoneinfo copy.
#   PIPELINE: live stage -> gate -> seed end-to-end.
#
# The bind-mount delivery over sealed /etc + the consumer kick need root+a real fs; they are proven by the
# sealed-VM dogfood (§11). This is the fast, root-free, build-free unit gate (mirrors owner-provision-proof).
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

GATE="$REPO_ROOT/image/overlay/usr/lib/shrek/shrek-provision-validate"
SEED="$REPO_ROOT/image/overlay/usr/lib/shrek/shrek-provision-seed"
STAGE="$REPO_ROOT/layers/shrek-installer/overlay/usr/libexec/shrek/shrek-provision-stage"
for f in "$GATE" "$SEED" "$STAGE"; do [ -x "$f" ] || { echo "FAIL: $f not executable"; exit 1; }; done

PASS=0; FAIL=0
ok()  { echo "  PASS $*"; PASS=$((PASS+1)); }
bad() { echo "  FAIL $*"; FAIL=$((FAIL+1)); }

SCRATCH="$(mktemp -d)"; trap 'rm -rf "$SCRATCH"' EXIT

# fixtures
XKB="$SCRATCH/xkb"; mkdir -p "$XKB"; : > "$XKB/us"; : > "$XKB/de"; : > "$XKB/fr"
LOCS="$SCRATCH/locales"; printf '%s\n' "C.UTF-8" "en_US.utf8" "de_DE.utf8" > "$LOCS"
ZI="$SCRATCH/zoneinfo"; mkdir -p "$ZI/America"; : > "$ZI/UTC"; : > "$ZI/America/New_York"
export SHREK_XKB_SYMBOLS_DIR="$XKB" SHREK_LOCALE_ALLOWLIST="$LOCS" SHREK_ZONEINFO_DIR="$ZI"

newstore() { local d="$SCRATCH/store-$1"; rm -rf "$d"; mkdir -p "$d"; printf '%s' "$d"; }
manifest() { printf '%s\n' "$2" > "$1/manifest"; chmod 0600 "$1/manifest"; }
gate()  { SHREK_PROVISION_STORE="$1" SHREK_PROVISION_RUN="$1/run" SHREK_PROVISION_SKIP_OWNER=1 "$GATE" >/dev/null 2>&1 || echo "GATE-NONZERO"; }
seed()  { SHREK_PROVISION_DOMAIN="$1" SHREK_PROVISION_STORE="$2" SHREK_PROVISION_RUN="$2/run" SHREK_PROVISION_DELIVER=0 "$SEED" >/dev/null 2>&1; }
runf()  { [ -e "$1/run/$2" ]; }   # per-key file present?
sentinel() { [ -e "$1/run/.gate-complete" ]; }

# =====================================================================================================
echo "=== 1. GATE: valid manifest -> per-key files + sentinel, no fault ==="
D="$(newstore valid)"
manifest "$D" "$(printf 'keymap=de\nlocale=en_US.UTF-8\nowner_display_name=Ogre King\nschema_version=1')"
gate "$D"
sentinel "$D" && ok "sentinel present" || bad "no sentinel"
runf "$D" locale && [ "$(cat "$D/run/locale")" = "en_US.UTF-8" ] && ok "locale emitted" || bad "locale"
runf "$D" keymap && [ "$(cat "$D/run/keymap")" = "de" ] && ok "keymap emitted" || bad "keymap"
runf "$D" owner_display_name && ok "name emitted" || bad "name"
[ ! -f "$D/fault" ] && ok "no fault" || bad "fault: $(cat "$D/fault")"

echo "=== 2. GATE read-hardening -> WHOLE reject, sentinel STILL fires, audit copy, no per-key ==="
# symlink manifest
D="$(newstore symlink)"; printf 'schema_version=1\nlocale=en_US.UTF-8\n' > "$D/real"; ln -s "$D/real" "$D/manifest"
gate "$D"
sentinel "$D" && ok "symlink: sentinel still fired" || bad "symlink: no sentinel"
runf "$D" locale && bad "symlink: leaked a key" || ok "symlink: no per-key emitted"
[ -f "$D/manifest.rejected" ] || [ -f "$D/fault" ] && ok "symlink: fault/audit recorded" || bad "symlink: no fault"
# duplicate key
D="$(newstore dup)"; manifest "$D" "$(printf 'schema_version=1\nlocale=en_US.UTF-8\nlocale=de_DE.UTF-8')"
gate "$D"; sentinel "$D" && ! runf "$D" locale && grep -q "duplicate" "$D/fault" && ok "duplicate key rejected whole" || bad "dup-key"
# unknown key
D="$(newstore unk)"; manifest "$D" "$(printf 'schema_version=1\nevil=1')"
gate "$D"; sentinel "$D" && grep -q "unknown key" "$D/fault" && ok "unknown key rejected whole" || bad "unknown-key"
# oversize
D="$(newstore big)"; { printf 'schema_version=1\n'; head -c 70000 </dev/zero | tr '\0' 'x' | sed 's/^/pad=/'; } > "$D/manifest"; chmod 0600 "$D/manifest"
SHREK_PROVISION_STORE="$D" SHREK_PROVISION_RUN="$D/run" SHREK_PROVISION_SKIP_OWNER=1 SHREK_PROVISION_SIZE_CAP=65536 "$GATE" >/dev/null 2>&1 || true
sentinel "$D" && grep -q "size cap" "$D/fault" && ok "oversize rejected whole" || bad "oversize"
# schema mismatch
D="$(newstore sch)"; manifest "$D" "$(printf 'schema_version=9\nlocale=en_US.UTF-8')"
gate "$D"; sentinel "$D" && ! runf "$D" locale && grep -q "schema_version" "$D/fault" && ok "schema mismatch rejected whole" || bad "schema"

echo "=== 3. GATE: bad single value dropped + fault, others survive ==="
D="$(newstore badval)"
manifest "$D" "$(printf 'schema_version=1\nlocale=en_US.UTF-8\nkeymap=zz9\ntimezone=../etc/passwd')"
gate "$D"
runf "$D" locale && ok "good locale survives" || bad "locale dropped"
! runf "$D" keymap && grep -q "keymap:" "$D/fault" && ok "bad keymap dropped+fault" || bad "keymap not dropped"
! runf "$D" timezone && grep -q "timezone:" "$D/fault" && ok "path-escape timezone dropped+fault" || bad "timezone not dropped"

echo "=== 4. GATE: missing manifest -> clean default (sentinel, no keys, no fault) ==="
D="$(newstore none)"; gate "$D"
sentinel "$D" && [ ! -f "$D/fault" ] && ! runf "$D" locale && ok "missing manifest = clean default" || bad "missing-manifest"

echo "=== 5. APPLIER: gate-complete + value -> seed state file + stamp; render correct ==="
D="$(newstore seedok)"
manifest "$D" "$(printf 'schema_version=1\nlocale=en_US.UTF-8\nkeymap=de\ntimezone=America/New_York')"
gate "$D"
seed locale "$D"; seed keymap "$D"; seed timezone "$D"
[ -f "$D/state/locale.conf" ] && [ "$(cat "$D/state/locale.conf")" = "LANG=en_US.UTF-8" ] && ok "locale seeded (LANG=)" || bad "locale state"
[ -f "$D/state/vconsole.conf" ] && grep -q "XKBLAYOUT=de" "$D/state/vconsole.conf" && grep -q "KEYMAP=us" "$D/state/vconsole.conf" && ok "keymap seeded (XKBLAYOUT=de + KEYMAP=us fallback)" || bad "keymap state"
[ -f "$D/state/localtime" ] && [ ! -L "$D/state/localtime" ] && ok "timezone seeded (regular file)" || bad "timezone state"
[ -f "$D/.applied/locale" ] && [ -f "$D/.applied/keymap" ] && [ -f "$D/.applied/timezone" ] && ok "all domains stamped" || bad "stamps"

echo "=== 6. APPLIER: seed-once — a prior stamp is never re-clobbered ==="
D="$(newstore once)"
manifest "$D" "$(printf 'schema_version=1\nlocale=en_US.UTF-8')"
gate "$D"; seed locale "$D"
printf 'LANG=de_DE.UTF-8\n' > "$D/state/locale.conf"   # simulate a later manual user change
seed locale "$D"                                        # must NOT re-seed over it
[ "$(cat "$D/state/locale.conf")" = "LANG=de_DE.UTF-8" ] && ok "manual change preserved (seed-once)" || bad "re-clobbered"

echo "=== 7. APPLIER: gate-complete + NO value -> terminal-default stamp, NO state file ==="
D="$(newstore def)"
manifest "$D" "$(printf 'schema_version=1\nlocale=en_US.UTF-8')"   # keymap absent
gate "$D"; seed keymap "$D"
[ -f "$D/.applied/keymap" ] && grep -q "default" "$D/.applied/keymap" && ok "terminal-default stamp written" || bad "no default stamp"
[ ! -f "$D/state/vconsole.conf" ] && ok "no state file (baked default stands)" || bad "unexpected state file"

echo "=== 8. APPLIER: sentinel ABSENT (gate crashed) -> neither seed nor stamp; retry ==="
D="$(newstore crash)"
manifest "$D" "$(printf 'schema_version=1\nlocale=en_US.UTF-8')"
mkdir -p "$D/run"; printf 'en_US.UTF-8' > "$D/run/locale"   # a key file exists but NO .gate-complete sentinel
seed locale "$D"
[ ! -f "$D/.applied/locale" ] && ok "no stamp on crashed gate" || bad "stamped despite no sentinel"
[ ! -f "$D/state/locale.conf" ] && ok "no seed on crashed gate (retry next boot)" || bad "seeded despite no sentinel"

echo "=== 9. APPLIER: crashed gate but prior state persists -> not stamped, state preserved ==="
D="$(newstore persist)"
manifest "$D" "$(printf 'schema_version=1\nlocale=en_US.UTF-8')"
gate "$D"; seed locale "$D"                 # boot 1: seed + stamp
rm -f "$D/run/.gate-complete"               # boot 2: gate crashed (sentinel gone), key file may be stale
seed locale "$D"
[ -f "$D/state/locale.conf" ] && ok "prior seeded state preserved across a crashed gate" || bad "lost persistent state"

echo "=== 10. PIPELINE: live stage -> gate -> seed end-to-end ==="
D="$(newstore e2e)"
SHREK_PROVISION_STAGE_DIR="$D/staged" SHREK_PROV_LOCALE="en_US.UTF-8" SHREK_PROV_KEYMAP="fr" SHREK_PROV_OWNER_NAME="Fiona" \
  "$STAGE" >/dev/null 2>&1
cp "$D/staged/manifest" "$D/manifest"; chmod 0600 "$D/manifest"   # (transplant, simulated)
gate "$D"; seed locale "$D"; seed keymap "$D"
[ "$(cat "$D/state/locale.conf" 2>/dev/null)" = "LANG=en_US.UTF-8" ] && grep -q "XKBLAYOUT=fr" "$D/state/vconsole.conf" 2>/dev/null \
  && ok "stage->gate->seed carries locale+keymap end-to-end" || bad "pipeline mismatch"

echo
echo "==================================================================="
echo "PROVISION-APPLY-PROOF: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ] && echo "RESULT: GREEN" || { echo "RESULT: RED"; exit 1; }
