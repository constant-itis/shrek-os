#!/usr/bin/env bash
# Fast host proof for ADR-005 §6.5 — the provisioning-manifest transplant in shrek-install-target.
#
# Exercises the transplant via the SHREK_INSTALL_TRANSPLANT_SEAM (no target disk / mkfs / mount / root):
#   1. a staged manifest lands at <home>/.shrek-system/provisioning/manifest, mode 0600, store dir 0700,
#      byte-identical to the source, with NO leftover manifest.tmp (atomic rename);
#   2. an absent/empty staged manifest is a clean no-op (exit 0, nothing written) — target first-boot-defaults;
#   3. main.py passes --provisioning-manifest to the writer, and the writer accepts the flag.
# The real mkfs'd-fs transplant (fsync/rename durability on a live ext4) is covered by install0-writer-proof
# under the payload build; the sealed-VM dogfood (§11) is the end-to-end runtime oracle.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

TARGET=layers/shrek-installer/overlay/usr/libexec/shrek/shrek-install-target
MAINPY=layers/shrek-installer/overlay/usr/lib/calamares/modules/shrekdeploy/main.py

PASS=0; FAIL=0
ok()  { echo "  PASS $*"; PASS=$((PASS+1)); }
bad() { echo "  FAIL $*"; FAIL=$((FAIL+1)); }

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
transplant() { SHREK_INSTALL_TRANSPLANT_SEAM=1 SHREK_PROV_MANIFEST_SRC="$1" SHREK_INSTALL_HOME_MNT="$2" sh "$TARGET"; }

echo "=== 1. staged manifest transplants with the ADR-004 write discipline ==="
printf 'schema_version=1\nkeymap=de\nlocale=C.UTF-8\nowner_display_name=Swamp Lord\n' > "$TMP/staged"
mkdir -p "$TMP/home"
transplant "$TMP/staged" "$TMP/home" >/dev/null 2>&1 && ok "transplant exits 0" || bad "transplant errored"
DST="$TMP/home/.shrek-system/provisioning/manifest"
[ -f "$DST" ] && ok "manifest landed at .shrek-system/provisioning/manifest" || bad "manifest not written"
[ "$(stat -c %a "$DST" 2>/dev/null)" = 600 ] && ok "manifest mode 0600" || bad "manifest mode=$(stat -c %a "$DST" 2>/dev/null) (want 600)"
[ "$(stat -c %a "$TMP/home/.shrek-system/provisioning" 2>/dev/null)" = 700 ] && ok "store dir mode 0700" || bad "store dir not 0700"
[ "$(stat -c %a "$TMP/home/.shrek-system" 2>/dev/null)" = 700 ] && ok ".shrek-system mode 0700" || bad ".shrek-system not 0700"
cmp -s "$TMP/staged" "$DST" && ok "manifest is byte-identical to the staged source" || bad "manifest content differs from source"
[ ! -e "$TMP/home/.shrek-system/provisioning/manifest.tmp" ] && ok "no leftover manifest.tmp (atomic rename)" || bad "manifest.tmp left behind"

echo "=== 2. absent staged manifest is a clean no-op ==="
mkdir -p "$TMP/home2"
transplant "$TMP/does-not-exist" "$TMP/home2" >/dev/null 2>&1 && ok "absent manifest exits 0" || bad "absent manifest errored"
[ -z "$(ls -A "$TMP/home2" 2>/dev/null)" ] && ok "nothing written (target will first-boot-default)" || bad "absent manifest wrote something"

echo "=== 3. writer + calamares wiring ==="
grep -q -- '--provisioning-manifest) PROV_MANIFEST=' "$TARGET" && ok "writer parses --provisioning-manifest" || bad "writer missing --provisioning-manifest arg"
grep -q 'transplant_provisioning_manifest "\$PROV_MANIFEST"' "$TARGET" && ok "writer calls transplant after mkfs with the parsed path" || bad "writer does not invoke the transplant with PROV_MANIFEST"
grep -q -- '"--provisioning-manifest"' "$MAINPY" && grep -q 'prov_manifest' "$MAINPY" && ok "main.py passes --provisioning-manifest to the writer" || bad "main.py does not pass the staged manifest path"
python3 -m py_compile "$MAINPY" 2>/dev/null && ok "main.py compiles" || bad "main.py syntax error"

echo
echo "=== install0-transplant-proof: PASS=$PASS FAIL=$FAIL ==="
[ "$FAIL" -eq 0 ]
