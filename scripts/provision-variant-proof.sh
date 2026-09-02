#!/usr/bin/env bash
# Host proof for ADR-005 provisioning-plane ENABLEMENT (the gate + locale/keymap appliers, §8 variant gating).
#
# Asserts, WITHOUT the mkosi/docker image build (via the SHREK_STAGE_ONLY seam in build-in-container.sh):
#   1. the validate-gate + locale/keymap applier enable symlinks are staged into multi-user.target.wants/
#      for INSTALLABLE + DOGFOOD, and are ABSENT for LIVE_INSTALLER + plain-CI;
#   2. the DOGFOOD-only baked test-manifest + gate-seed env are present ONLY for DOGFOOD, and carry values
#      that DIFFER from the §5a baked defaults (so the sealed-VM dogfood can prove delivery, not a match);
#   3. the unit FILES are tracked source, carry NO [Install] section (presets can never auto-enable them),
#      and the appliers depend on the gate with After= ORDERING ONLY — never Requires=/Wants= (the §6
#      "no failure cascade in the non-secret plane" property);
#   4. the provisioning store dirs are declared in tmpfiles, and all generated artifacts are gitignored.
# The sealed-VM dogfood (§11) is the runtime proof; this is the fast, root-free, build-free enablement gate.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

U=image/overlay/usr/lib/systemd/system
GATE="$U/shrek-provision-validate.service"
LOCALE="$U/shrek-locale-seed.service"
KEYMAP="$U/shrek-keymap-seed.service"
UNITS="shrek-provision-validate.service shrek-locale-seed.service shrek-keymap-seed.service"
ENV=image/overlay/etc/shrek/provisioning.env
MANIFEST=image/overlay/etc/shrek/test-manifest
TMPFILES=image/overlay/usr/lib/tmpfiles.d/shrek-home.conf

PASS=0; FAIL=0
ok()  { echo "  PASS $*"; PASS=$((PASS+1)); }
bad() { echo "  FAIL $*"; FAIL=$((FAIL+1)); }

# All five generated artifacts are gitignored build inputs; clear them so each stage starts clean.
clean() { for u in $UNITS; do rm -f "$U/multi-user.target.wants/$u"; done; rm -f "$ENV" "$MANIFEST"; return 0; }
trap clean EXIT
stage() { clean; env "$@" SHREK_STAGE_ONLY=1 bash scripts/build-in-container.sh 1 >/dev/null 2>&1 || return 1; }
present() { [ -e "$1" ] || [ -L "$1" ]; }
# count of the three applier/gate enable symlinks currently staged
wants_n() { local n=0 u; for u in $UNITS; do present "$U/multi-user.target.wants/$u" && n=$((n+1)); done; echo "$n"; }

echo "=== §8 variant matrix (gate+appliers enabled iff INSTALLABLE|DOGFOOD; test-manifest iff DOGFOOD) ==="

echo "--- DOGFOOD=1 ---"
stage DOGFOOD=1 || bad "DOGFOOD staging errored"
[ "$(wants_n)" = 3 ] && ok "DOGFOOD: gate + 2 appliers enabled (3/3 wants)" || bad "DOGFOOD: enabled $(wants_n)/3 (want 3)"
present "$ENV"      && ok "DOGFOOD: gate-seed provisioning.env staged"       || bad "DOGFOOD: provisioning.env MISSING"
present "$MANIFEST" && ok "DOGFOOD: baked test-manifest staged"              || bad "DOGFOOD: test-manifest MISSING"
if present "$ENV"; then grep -q '^SHREK_PROVISION_SEED_MANIFEST=/etc/shrek/test-manifest$' "$ENV" \
  && ok "DOGFOOD: env points the gate at /etc/shrek/test-manifest" || bad "DOGFOOD: env does not name the test-manifest"; fi
if present "$MANIFEST"; then
  grep -q '^schema_version=1$' "$MANIFEST" && ok "DOGFOOD: test-manifest schema_version=1" || bad "DOGFOOD: test-manifest missing schema_version=1"
  # Values MUST differ from the §5a baked defaults (locale.conf LANG=en_US.UTF-8, vconsole XKBLAYOUT=us),
  # else 'session LANG/active-VT layout correct' could pass on the baked default without any delivery.
  grep -q '^locale=C.UTF-8$' "$MANIFEST" && ok "DOGFOOD: test locale C.UTF-8 differs from baked en_US.UTF-8" || bad "DOGFOOD: test locale not the expected non-default"
  grep -q '^keymap=de$'      "$MANIFEST" && ok "DOGFOOD: test keymap de differs from baked us"               || bad "DOGFOOD: test keymap not the expected non-default"
fi

echo "--- INSTALLABLE=1 (the product) ---"
stage INSTALLABLE=1 || bad "INSTALLABLE staging errored"
[ "$(wants_n)" = 3 ] && ok "INSTALLABLE: gate + 2 appliers enabled (3/3 wants)" || bad "INSTALLABLE: enabled $(wants_n)/3 (want 3)"
present "$ENV"      && bad "INSTALLABLE: must NOT bake an env (validates the real transplanted manifest)" || ok "INSTALLABLE: no test env"
present "$MANIFEST" && bad "INSTALLABLE: must NOT bake a test-manifest"                                   || ok "INSTALLABLE: no test-manifest"

echo "--- LIVE_INSTALLER=1 ---"
stage LIVE_INSTALLER=1 || bad "LIVE_INSTALLER staging errored"
[ "$(wants_n)" = 0 ] && ok "LIVE_INSTALLER: provisioning plane OFF (0/3 wants)" || bad "LIVE_INSTALLER: $(wants_n)/3 enabled (want 0 — no persistent /home)"
present "$ENV" || present "$MANIFEST" && bad "LIVE_INSTALLER: leaked env/test-manifest" || ok "LIVE_INSTALLER: no env/test-manifest"

echo "--- plain (no flags / desktop-sealed-proof default) ---"
stage || bad "plain staging errored"
[ "$(wants_n)" = 0 ] && ok "plain: provisioning plane OFF (0/3 wants)" || bad "plain: $(wants_n)/3 enabled (want 0)"
present "$ENV" || present "$MANIFEST" && bad "plain: leaked env/test-manifest" || ok "plain: no env/test-manifest"

echo "=== unit contracts (tracked source · no [Install] · non-secret-plane dependency rule) ==="
for u in $GATE $LOCALE $KEYMAP; do
  # Source, not a gitignored build input: the unit FILE must exist and NOT be caught by a gitignore glob
  # (contrast the enable symlinks below, which MUST be ignored). Works before the files are first committed.
  { [ -f "$u" ] && ! git check-ignore -q "$u"; } && ok "$(basename "$u"): tracked source (present, not ignored)" || bad "$(basename "$u"): missing or gitignored"
  grep -q '^\[Install\]' "$u" && bad "$(basename "$u"): has [Install] (presets could auto-enable it)" || ok "$(basename "$u"): no [Install] (enablement is variant-gated)"
done
# Gate ordering.
grep -Eq '^After=.*home\.mount' "$GATE" && ok "gate: After=home.mount (store lives on /home)" || bad "gate: missing After=home.mount"
grep -Eq '^Before=.*getty@tty1\.service' "$GATE" && ok "gate: Before=getty@tty1.service" || bad "gate: missing Before=getty@tty1.service"
# Appliers: After= the gate for ordering, but NEVER Requires=/Wants= it (no failure cascade, §6).
for a in $LOCALE $KEYMAP; do
  b=$(basename "$a")
  grep -Eq '^After=.*shrek-provision-validate\.service' "$a" && ok "$b: After= gate (ordering)" || bad "$b: not ordered After the gate"
  grep -Eq '^(Requires|Wants)=.*shrek-provision-validate' "$a" && bad "$b: Requires/Wants the gate — would cascade failure (§6 violation)" || ok "$b: no Requires/Wants on the gate (no cascade)"
  grep -Eq '^Before=.*getty@tty1\.service' "$a" && ok "$b: Before=getty@tty1.service (deliver before first session)" || bad "$b: missing Before=getty@tty1.service"
  grep -Eq '^Environment=SHREK_PROVISION_DOMAIN=' "$a" && ok "$b: selects its domain via Environment=" || bad "$b: no SHREK_PROVISION_DOMAIN"
  grep -Eq '^(PrivateMounts|MountFlags)=' "$a" && bad "$b: mount-namespace isolation would hide the /etc bind from PID1/PAM" || ok "$b: no mount-namespace isolation"
done

echo "=== store dirs (tmpfiles) + gitignore coverage ==="
for d in provisioning provisioning/state provisioning/.applied; do
  grep -Eq "^d /home/\.shrek-system/$d " "$TMPFILES" && ok "tmpfiles declares /home/.shrek-system/$d" || bad "tmpfiles missing /home/.shrek-system/$d"
done
for g in "$U/multi-user.target.wants/shrek-provision-validate.service" "$U/multi-user.target.wants/shrek-locale-seed.service" "$U/multi-user.target.wants/shrek-keymap-seed.service" "$ENV" "$MANIFEST"; do
  git check-ignore -q "$g" && ok "gitignored: $g" || bad "NOT gitignored: $g"
done

echo
echo "=== provision-variant-proof: PASS=$PASS FAIL=$FAIL ==="
[ "$FAIL" -eq 0 ]
