#!/usr/bin/env bash
# Shrek OS Desktop Bootstrap-0 — the SEALED-VM close-out (Pn-desktop). Proves the signed shrek-desktop
# sysext merges onto the sealed /usr in the KVM gate and DMS surfaces instantiate headless on the
# REAL image — the step scripts/desktop-smoke.sh (container smoke) explicitly could not cover.
# docs/desktop-bootstrap-0.md §Delivery / §Smoke.
#
# Pipeline (each stage is skippable/cached so re-runs are cheap):
#   1. build the signed shrek-desktop DDI            scripts/build-desktop-layer.sh   (heavy; packaged DMS)
#   2. bake the sealed root w/ policy + gate baked   scripts/build-in-container.sh 1   (REBUILD_ROOT=1)
#   3. assemble the layer store w/ shrek-desktop      scripts/build-layers.sh desktop
#   4. boot the KVM gate w/ the store attached        scripts/boot-vm.sh
#   5. assert Pn-desktop off out/vm-console.log
#
# The verdict is read from oniond's merge line + the shrek-desktop-gate.service SHREK_GATE lines.
#
#   Pn-desktop-merge      oniond/broker MERGED shrek-desktop (sysext) onto sealed /usr
#   Pn-desktop-sway       Sway came up headless in the sealed boot
#   Pn-desktop-qs-load    DMS-spawned Quickshell loaded /usr/share/quickshell/dms with no QML error
#   Pn-desktop-dms-core   DMS session-bus/writable-state managers did not fail at startup
#   Pn-desktop-surfaces   DMS QML connected to the DMS API
#   Pn-desktop-logout     clean session teardown
#   Pn-desktop-regress    shrek-hello + shrek-conf still merge alongside (no regression)
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"
mkdir -p out
ROOT_RAW="out/shrek_1_x86-64.raw"
LOG="out/vm-console.log"
BUDGET="${BUDGET:-200}"                 # desktop bring-up needs more than the marker-layer gates
REBUILD_DDI="${REBUILD_DDI:-0}"       # 1 = force-rebuild the desktop DDI even if one exists
REBUILD_ROOT="${REBUILD_ROOT:-1}"       # 1 = rebuild the sealed root (needed once: policy + gate baked in)

# --- 1. desktop DDI (heavy; reuse an existing one unless forced) ---
if [ "${REBUILD_DDI}" = 1 ] || ! ls out/layers/shrek-desktop*.raw >/dev/null 2>&1; then
  echo "############ 1/5 building the signed shrek-desktop DDI (packaged DMS) ############"
  scripts/build-desktop-layer.sh 2>&1 | tee out/desktop-ddi-build.log
fi
ls -l out/layers/shrek-desktop*.raw

# --- 2. sealed root with the enable-line + gate unit baked under dm-verity ---
if [ "${REBUILD_ROOT}" = 1 ] || [ ! -f "$ROOT_RAW" ]; then
  echo "############ 2/5 baking the sealed root (onion-policy enable shrek-desktop + shrek-desktop-gate) ############"
  scripts/build-in-container.sh 1 2>&1 | tee out/desktop-root.log
fi

# --- 3. layer store carrying shrek-desktop ---
echo "############ 3/5 assembling the layer store (desktop) ############"
scripts/build-layers.sh desktop 2>&1 | tee out/desktop-layers.log >/dev/null

# --- 4. boot the KVM gate ---
echo "############ 4/5 booting the sealed KVM gate w/ the desktop store ############"
STORE=out/layer-store.raw RAW="$ROOT_RAW" scripts/boot-vm.sh "$BUDGET"

# --- 5. assert Pn-desktop off the serial log ---
echo "############ 5/5 Pn-desktop assertions ############"
has()    { grep -aqF "$1" "$LOG"; }
GATE_OK=0
check()  { if [ "$2" = 0 ]; then echo "PASS ✅  $1"; else echo "FAIL ❌  $1"; GATE_OK=1; fi; }

echo "--- oniond merge verdict + shrek-desktop-gate lines ---"
grep -aE 'oniond:|SHREK_GATE:|shrek-desktop-gate' "$LOG" | tail -40 || true
echo "----------------------------------------------------"

has "oniond: shrek-desktop (sysext) -> merged"  && check "Pn-desktop-merge   shrek-desktop MERGED onto sealed /usr" 0 || check "Pn-desktop-merge   shrek-desktop MERGED onto sealed /usr" 1
has "SHREK_GATE: PASS Pn-desktop-sway"          && check "Pn-desktop-sway    Sway up headless in the sealed boot"   0 || check "Pn-desktop-sway    Sway up headless in the sealed boot"   1
has "SHREK_GATE: PASS Pn-desktop-qs-load"       && check "Pn-desktop-qs-load DMS Quickshell loaded cleanly"          0 || check "Pn-desktop-qs-load DMS Quickshell loaded cleanly"          1
has "SHREK_GATE: PASS Pn-desktop-dms-core"      && check "Pn-desktop-dms-core session bus + writable state ok"        0 || check "Pn-desktop-dms-core session bus + writable state ok"        1
has "SHREK_GATE: PASS Pn-desktop-surfaces"      && check "Pn-desktop-surfaces DMS QML connected to API"              0 || check "Pn-desktop-surfaces DMS QML connected to API"              1
has "SHREK_GATE: PASS Pn-desktop-logout"        && check "Pn-desktop-logout  clean session teardown"               0 || check "Pn-desktop-logout  clean session teardown"               1
# regression: the previously-proven marker layers must still merge alongside the desktop layer.
{ has "oniond: shrek-hello (sysext) -> merged" && has "oniond: shrek-conf (confext) -> merged"; } \
  && check "Pn-desktop-regress shrek-hello + shrek-conf still merge" 0 || check "Pn-desktop-regress shrek-hello + shrek-conf still merge" 1

echo "################ desktop-sealed-proof: $([ "$GATE_OK" = 0 ] && echo 'Pn-desktop ALL PASS ✅' || echo 'SOME FAILED ❌ (inspect out/vm-console.log)') ################"
exit "$GATE_OK"
