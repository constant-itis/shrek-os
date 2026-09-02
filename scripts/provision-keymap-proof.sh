#!/usr/bin/env bash
# Host proof for ADR-005 §5b — the VT keymap adapter + clobber closure (corrected mechanism: console-setup,
# not systemd-vconsole-setup — see the §5b-correction subsection). Root-free, no VM; §11 is the runtime oracle.
#
# Asserts:
#   1. shrek-provision-kick parses XKBLAYOUT from vconsole.conf WITHOUT sourcing it and emits the fixed
#      `ckbcomp "$XKBLAYOUT" | loadkeys -` invocation; no-ops on locale/timezone and on absent/invalid layout.
#   2. the compositor adapter (zz-shrek-desktop.sh) exports XKB_DEFAULT_LAYOUT from XKBLAYOUT, parsed (grep),
#      never `source`d, before exec shrek-desktop.
#   3. clobber closure wiring: the credential-boundary re-assert is an ExecStartPre=- kick on BOTH
#      shrek-owner-provision.service and getty@tty1; shrek-keymap-seed is ordered After console-setup's
#      keymap services and Before the owner wizard, still with NO Requires=/Wants= (no cascade, §6).
#   4. we ship NO udev override and do NOT mask systemd-vconsole-setup (the corrected mechanism ships neither).
#   5. the ADR records the §5b correction.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

KICK=image/overlay/usr/lib/shrek/shrek-provision-kick
COMPOSITOR=image/overlay/etc/profile.d/zz-shrek-desktop.sh
U=image/overlay/usr/lib/systemd/system
KEYMAP_UNIT="$U/shrek-keymap-seed.service"
OWNER_UNIT="$U/shrek-owner-provision.service"
GETTY_DROPIN="$U/getty@tty1.service.d/10-vt-keymap.conf"
ADR=docs/adr-005-provisioning.md

PASS=0; FAIL=0
ok()  { echo "  PASS $*"; PASS=$((PASS+1)); }
bad() { echo "  FAIL $*"; FAIL=$((FAIL+1)); }

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
# kick with the DELIVER=0 seam against a fixture vconsole.conf; echoes the resolved layout + WOULD-run shape.
kick() { local vc="$1" dom="${2:-keymap}"; SHREK_KICK_DOMAIN="$dom" SHREK_KICK_VCONSOLE="$vc" SHREK_KICK_DELIVER=0 sh "$KICK" 2>&1; }

echo "=== 1. VT keymap adapter (shrek-provision-kick) ==="
[ -x "$KICK" ] && ok "kick helper is executable" || bad "kick helper not executable"
{ [ -f "$KICK" ] && ! git check-ignore -q "$KICK"; } && ok "kick helper is tracked source (not gitignored)" || bad "kick helper missing/gitignored"
grep -q 'ckbcomp' "$KICK" && grep -q 'loadkeys -' "$KICK" && ok "kick uses the fixed ckbcomp | loadkeys - invocation" || bad "kick missing ckbcomp|loadkeys shape"
# Scan CODE only (drop comments): no `source`/`eval` and no leading `.` builtin reading the config.
KICK_CODE="$(grep -vE '^[[:space:]]*#' "$KICK")"
if printf '%s\n' "$KICK_CODE" | grep -Eq '(^|[[:space:]])(source|eval)[[:space:]]' || printf '%s\n' "$KICK_CODE" | grep -Eq '^[[:space:]]*\.[[:space:]]'; then
  bad "kick sources/evals config (must grep/param-expand)"; else ok "kick never sources/evals vconsole.conf"; fi

printf 'KEYMAP=us\nXKBLAYOUT=de\n' > "$TMP/vc"
kick "$TMP/vc" | grep -q "layout='de'" && ok "parses XKBLAYOUT=de" || bad "did not parse de"
kick "$TMP/vc" | grep -q "ckbcomp 'de' | loadkeys -" && ok "emits ckbcomp 'de' | loadkeys -" || bad "wrong invocation for de"
printf 'XKBLAYOUT="fr"\n' > "$TMP/vc"; kick "$TMP/vc" | grep -q "layout='fr'" && ok "strips quotes (XKBLAYOUT=\"fr\" -> fr)" || bad "quote-strip failed"
printf 'KEYMAP=us\nXKBLAYOUT=us\n' > "$TMP/vc"; kick "$TMP/vc" | grep -q "layout='us'" && ok "baked default us resolves us (harmless re-assert)" || bad "us not resolved"
printf 'XKBLAYOUT=../evil\n' > "$TMP/vc"; kick "$TMP/vc" | grep -q "no usable XKBLAYOUT" && ok "rejects non-alnum layout (defense in depth)" || bad "did not reject ../evil"
: > "$TMP/vc"; kick "$TMP/vc" | grep -q "no usable XKBLAYOUT" && ok "empty vconsole -> no-op" || bad "empty vconsole not handled"
kick "$TMP/vc" locale   >/dev/null 2>&1 && ok "locale domain no-op (exit 0)"   || bad "locale domain errored"
kick "$TMP/vc" timezone >/dev/null 2>&1 && ok "timezone domain no-op (exit 0)" || bad "timezone domain errored"

echo "=== 2. compositor adapter (XKB_DEFAULT_LAYOUT) ==="
grep -q 'export XKB_DEFAULT_LAYOUT' "$COMPOSITOR" && ok "compositor exports XKB_DEFAULT_LAYOUT" || bad "compositor does not export XKB_DEFAULT_LAYOUT"
grep -Eq 'grep -E .\^XKBLAYOUT=' "$COMPOSITOR" && ok "compositor greps XKBLAYOUT from vconsole.conf" || bad "compositor does not grep XKBLAYOUT"
# It must NOT `source` / `.` /etc/vconsole.conf.
grep -Eq '(source|^\s*\. )\s*/etc/vconsole\.conf' "$COMPOSITOR" && bad "compositor sources vconsole.conf (must not)" || ok "compositor never sources vconsole.conf"
awk '/export XKB_DEFAULT_LAYOUT/{e=NR} /exec shrek-desktop/{x=NR} END{exit !(e && x && e<x)}' "$COMPOSITOR" \
  && ok "XKB_DEFAULT_LAYOUT exported before exec shrek-desktop" || bad "export not ordered before exec shrek-desktop"

echo "=== 3. clobber closure wiring (credential-boundary re-assert + race ordering) ==="
grep -Eq '^ExecStartPre=-.*SHREK_KICK_DOMAIN=keymap.*shrek-provision-kick' "$OWNER_UNIT" \
  && ok "owner-provision re-asserts keymap via ExecStartPre=- (guard i, credential VT)" || bad "owner-provision missing keymap ExecStartPre re-assert"
grep -Eq '^ExecStartPre=-.*SHREK_KICK_DOMAIN=keymap.*shrek-provision-kick' "$GETTY_DROPIN" \
  && ok "getty@tty1 drop-in re-asserts keymap via ExecStartPre=- (guard i, login VT)" || bad "getty drop-in missing keymap ExecStartPre re-assert"
{ [ -f "$GETTY_DROPIN" ] && ! git check-ignore -q "$GETTY_DROPIN"; } && ok "getty keymap drop-in is tracked (unconditional, harmless when us)" || bad "getty keymap drop-in missing/gitignored"
grep -Eq '^After=.*console-setup\.service' "$KEYMAP_UNIT" && grep -Eq '^After=.*keyboard-setup\.service' "$KEYMAP_UNIT" \
  && ok "keymap-seed After console-setup + keyboard-setup (wins the multi-user race)" || bad "keymap-seed not ordered after console-setup's keymap services"
grep -Eq '^Before=.*shrek-owner-provision\.service' "$KEYMAP_UNIT" \
  && ok "keymap-seed Before owner-provision (layout live before passphrase enrollment)" || bad "keymap-seed not ordered before the owner wizard"
grep -Eq '^(Requires|Wants)=.*(console-setup|keyboard-setup|shrek-owner-provision|shrek-provision-validate)' "$KEYMAP_UNIT" \
  && bad "keymap-seed Requires/Wants an ordered unit — would cascade failure (§6)" || ok "keymap-seed ordering is After/Before only (no cascade)"

echo "=== 4. corrected mechanism ships NO udev override / vconsole-setup mask ==="
ls image/overlay/etc/udev/rules.d/90-vconsole.rules image/overlay/usr/lib/udev/rules.d/90-vconsole.rules 2>/dev/null | grep -q . \
  && bad "ships a 90-vconsole.rules override (unnecessary — vtcon rule is already font-only)" || ok "no 90-vconsole.rules override shipped"
ls -l image/overlay/etc/systemd/system/systemd-vconsole-setup.service 2>/dev/null | grep -q '/dev/null' \
  && bad "masks systemd-vconsole-setup (absent on our image — masking is wrong)" || ok "does not mask systemd-vconsole-setup"

echo "=== 5. ADR records the §5b correction ==="
grep -q '5b-correction' "$ADR" && grep -q 'console-setup' "$ADR" && ok "ADR-005 documents the systemd-vconsole-setup -> console-setup correction" || bad "ADR-005 missing the §5b correction"

echo
echo "=== provision-keymap-proof: PASS=$PASS FAIL=$FAIL ==="
[ "$FAIL" -eq 0 ]
