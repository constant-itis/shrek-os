#!/usr/bin/env bash
# Shrek OS Phase-1 — S8 (final Phase-1 milestone): prove automatic rollback of a broken update.
#
# THESIS: a broken A/B update that never reaches boot-complete.target exhausts its boot-count tries
# and systemd-boot falls back to the last-good UKI — with NO operator action. This script drives the
# whole proof offline, reusing the S7 machinery:
#
#   1. build a GOOD v1                         → out/shrek_1_x86-64.raw    (UKI shrek_1_x86-64.efi, NO counter = permanent good)
#   2. build a DELIBERATELY BROKEN v2 (BREAK=1)→ out/shrek_2_x86-64.*      (sealed root carries /usr/lib/shrek/boot-poison)
#   3. offline A/B update v1's disk to v2      → out/shrek-updated.raw     (v2 in slot B + boot-counted UKI shrek_2_x86-64+3-0.efi)
#   4. boot out/shrek-updated.raw in the VM    → systemd-boot picks v2, the health gate fails, FailureAction=reboot,
#                                                the loader decrements +3-0 → +2-1 → +1-2 → +0-3, marks v2 bad,
#                                                and falls back to v1. Final steady state = v1 (IMAGE_VERSION=1) = ROLLBACK.
#
# Everything runs in ephemeral debian:trixie containers (build/update) + a throwaway KVM VM (boot);
# the build host is never mutated. Builds are ~10 min each — set REBUILD=0 to reuse out/ artifacts.
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"
mkdir -p out    # tee targets below need it before build-in-container.sh creates it
REBUILD="${REBUILD:-1}"
BUDGET="${BUDGET:-480}"      # wall-clock for the multi-reboot VM cycle (enrollment + 3 failed v2 boots + v1)

if [ "$REBUILD" = "1" ]; then
  echo "############ S8/1 — build GOOD v1 ############"
  scripts/build-in-container.sh 1 2>&1 | tee out/s8-build-v1.log
  echo "############ S8/2 — build BROKEN v2 (BREAK=1) ############"
  BREAK=1 scripts/build-in-container.sh 2 2>&1 | tee out/s8-build-v2.log
else
  echo "REBUILD=0 — reusing existing out/ artifacts"
fi

echo "############ S8/3 — offline A/B update v1 → v2 ############"
scripts/update-in-container.sh 2>&1 | tee out/s8-update.log

echo "############ S8/4 — boot the updated disk; watch the rollback (${BUDGET}s budget) ############"
RAW=out/shrek-updated.raw scripts/boot-vm.sh "$BUDGET"

LOG=out/vm-console.log
echo
echo "############ S8 — verdict ############"
# The UKI banner (/etc/issue) prints IMAGE_VERSION on each login prompt. A correct rollback shows v2
# attempts (with the health-gate failure) followed by a FINAL settle on v1.
echo "--- health-gate failures seen (v2 unhealthy) ---"
grep -n "shrek-boot-health" "$LOG" || echo "  (none — unexpected)"
echo "--- IMAGE_VERSION banners in boot order ---"
grep -n "IMAGE_VERSION=" "$LOG" || echo "  (none)"
LAST_VER="$(grep -o 'IMAGE_VERSION=[0-9]*' "$LOG" | tail -1 || true)"
echo
if [ "$LAST_VER" = "IMAGE_VERSION=1" ]; then
  echo "PASS ✅  final boot settled on v1 ($LAST_VER) — the broken v2 update rolled back automatically."
else
  echo "CHECK ⚠  final banner was '${LAST_VER:-none}' (expected IMAGE_VERSION=1). Inspect $LOG."
fi
