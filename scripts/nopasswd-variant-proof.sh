#!/usr/bin/env bash
# Host proof for bench authz slice step 4 (retire the dev NOPASSWD placeholder).
#
# Asserts, WITHOUT the mkosi/docker image build (via the SHREK_STAGE_ONLY seam in build-in-container.sh):
#   1. the dev NOPASSWD sudoers is baked into the sealed base /etc ONLY for LIVE_INSTALLER builds;
#   2. the DOGFOOD-only root debug-shell enable is present ONLY for DOGFOOD builds;
#   3. the live-installer template parses as sudoers;
#   4. the companion no-sudo paths hold (hosts-seed owns the binding store to the box owner; shrek-connect
#      has no sudo fallback; build-installer-payload.sh gates the product artifact).
# The sealed-artifact gate (build-installer-payload.sh) and the sealed-VM dogfood are the runtime proofs;
# this is the fast, root-free, build-free unit gate.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

SUDOERS_GEN="image/overlay/etc/sudoers.d/dev-nopasswd"
DEBUG_SHELL="image/overlay/usr/lib/systemd/system/multi-user.target.wants/debug-shell.service"
TEMPLATE="image/live-installer/sudoers.d/dev-nopasswd"

PASS=0; FAIL=0
ok()  { echo "  PASS $*"; PASS=$((PASS+1)); }
bad() { echo "  FAIL $*"; FAIL=$((FAIL+1)); }

# Both generated artifacts are gitignored build inputs; clear them so each stage starts clean.
# The plain/INSTALLABLE branches of build-in-container.sh also `rm` the untracked var-lib-swamp.mount mask
# (a pre-existing, NOPASSWD-orthogonal leak — the owner's standing note is to LEAVE that file), so snapshot
# and restore it, keeping this proof non-destructive.
SWAMP=image/overlay/etc/systemd/system/var-lib-swamp.mount
SWAMP_WAS_LINK=""; [ -L "$SWAMP" ] && SWAMP_WAS_LINK="$(readlink "$SWAMP")"
clean() {
  rm -f "$SUDOERS_GEN" "$DEBUG_SHELL"
  [ -n "$SWAMP_WAS_LINK" ] && [ ! -e "$SWAMP" ] && [ ! -L "$SWAMP" ] && ln -sf "$SWAMP_WAS_LINK" "$SWAMP"
  return 0
}
trap clean EXIT

# Run build-in-container.sh's host staging up to the seam for one variant, then report the two artifacts.
stage() { clean; env "$@" SHREK_STAGE_ONLY=1 bash scripts/build-in-container.sh 1 >/dev/null 2>&1 || return 1; }
# "staged" = a real file OR a symlink. The debug-shell enable points at ../debug-shell.service, a STOCK
# systemd unit that exists only in the baked image (not our overlay source), so at stage time it is a
# dangling symlink — which still means the enable is staged and will resolve at runtime. Hence -L too.
present() { [ -e "$1" ] || [ -L "$1" ]; }

echo "=== step 4 variant matrix (dev NOPASSWD present iff LIVE_INSTALLER; debug-shell iff DOGFOOD) ==="

echo "--- LIVE_INSTALLER=1 ---"
stage LIVE_INSTALLER=1 || bad "LIVE_INSTALLER staging errored"
present "$SUDOERS_GEN"  && ok "LIVE_INSTALLER: dev NOPASSWD baked"        || bad "LIVE_INSTALLER: dev NOPASSWD MISSING"
present "$DEBUG_SHELL"  && bad "LIVE_INSTALLER: debug-shell must NOT be enabled" || ok "LIVE_INSTALLER: no debug-shell"
# The baked copy must be mode 0440 (install -m0440), regardless of the tracked template's git mode.
if present "$SUDOERS_GEN"; then
  m=$(stat -c '%a' "$SUDOERS_GEN")
  [ "$m" = "440" ] && ok "LIVE_INSTALLER: baked sudoers mode 0440" || bad "LIVE_INSTALLER: baked sudoers mode=$m (want 440)"
fi

echo "--- DOGFOOD=1 ---"
stage DOGFOOD=1 || bad "DOGFOOD staging errored"
present "$SUDOERS_GEN" && bad "DOGFOOD: dev NOPASSWD must be ABSENT (else sudo bypasses the consent ceremony)" || ok "DOGFOOD: no dev NOPASSWD"
present "$DEBUG_SHELL" && ok "DOGFOOD: root debug-shell enabled" || bad "DOGFOOD: debug-shell MISSING (adminless VM)"

echo "--- INSTALLABLE=1 (the product) ---"
stage INSTALLABLE=1 || bad "INSTALLABLE staging errored"
present "$SUDOERS_GEN" && bad "INSTALLABLE: dev NOPASSWD must be ABSENT on the product" || ok "INSTALLABLE: no dev NOPASSWD"
present "$DEBUG_SHELL" && bad "INSTALLABLE: debug-shell must be ABSENT on the product" || ok "INSTALLABLE: no debug-shell"

echo "--- plain (no flags / desktop-sealed-proof default) ---"
stage || bad "plain staging errored"
present "$SUDOERS_GEN" && bad "plain: dev NOPASSWD must be ABSENT" || ok "plain: no dev NOPASSWD"
present "$DEBUG_SHELL" && bad "plain: debug-shell must be ABSENT" || ok "plain: no debug-shell"

echo "=== template + companion no-sudo paths ==="
visudo -cf "$TEMPLATE" >/dev/null 2>&1 && ok "live-installer template parses as sudoers" || bad "template failed visudo"
grep -q 'LIVE INSTALLER ONLY' "$TEMPLATE" && ok "template header marks it live-installer-only" || bad "template header missing warning"

HS=image/overlay/usr/lib/shrek/hosts-seed
grep -Eq 'chown +"?\$?owner' "$HS" && ok "hosts-seed re-owns the binding store to the box owner (no root:root)" || bad "hosts-seed does not chown the store to the owner"

SC=layers/shrek-desktop/overlay/usr/bin/shrek-connect
grep -q 'sudo shrek-connect' "$SC" && bad "shrek-connect still suggests 'sudo shrek-connect'" || ok "shrek-connect has no sudo fallback"

PG=scripts/build-installer-payload.sh
grep -q 'sudoers.d/dev-nopasswd' "$PG" && ok "build-installer-payload gates the product artifact against dev NOPASSWD" || bad "payload script has no product-base gate"

echo
echo "=== step 4: PASS=$PASS FAIL=$FAIL ==="
[ "$FAIL" -eq 0 ]
