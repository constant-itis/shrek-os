#!/usr/bin/env bash
# Shrek OS Phase-4 (slice 1) — prove oniond owns the layer-merge policy (docs/phase4-oniond.md).
# Reuses the sealed Phase-1 root, rebuilt ONCE with the Phase-4 machinery (oniond + shrekctl in
# /usr/libexec/shrek, /usr/lib/shrek/onion-policy, the reworked shrek-onion.service). Then each gate
# is a cheap layer-store swap. The verdict is read from oniond's + shrekctl's lines in vm-console.log.
#
#   O1  good      → shrek-hello (sysext) MERGED + shrek-conf (confext) MERGED; verdict via oniond/shrekctl
#   O2  select    → shrek-hello MERGED, shrek-extra (signed but not enabled) OMITTED (not merged)
#   O3a unsigned  → shrek-hello REFUSED (verity present, no signature)
#   O3b tamper    → shrek-hello REFUSED (verity signature byte-flipped)
#   O4            → shrekctl onion status renders the audit record on every gate (checked inline)
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"
mkdir -p out
ROOT_RAW="out/shrek_1_x86-64.raw"
REBUILD_ROOT="${REBUILD_ROOT:-0}"    # 1 = rebuild the sealed root (needed: the overlay/units changed)
BUDGET="${BUDGET:-160}"
LOG="out/vm-console.log"

if [ "$REBUILD_ROOT" = 1 ] || [ ! -f "$ROOT_RAW" ]; then
  echo "############ building the Phase-4 sealed root (oniond + shrekctl + policy) ############"
  scripts/build-in-container.sh 1 2>&1 | tee out/oniond-root.log
fi

# grep the console log for a fixed string; print PASS/FAIL.  has <label> <pattern>
has()    { grep -aqF "$1" "$LOG"; }
absent() { ! grep -aqF "$1" "$LOG"; }
check()  { # <desc> <0=pass|1=fail from caller>
  if [ "$2" = 0 ]; then echo "PASS ✅  $1"; else echo "FAIL ❌  $1"; GATE_OK=1; fi
}

# Boot one layer-store mode, then run the caller-provided assertions.  run_gate <mode> <assert-fn>
run_gate() {
  local mode="$1" assert_fn="$2"
  echo "############ gate: store=${mode} ############"
  scripts/build-layers.sh "$mode" 2>&1 | tee "out/oniond-layers-${mode}.log" >/dev/null
  STORE=out/layer-store.raw RAW="$ROOT_RAW" scripts/boot-vm.sh "$BUDGET"
  echo "--- oniond / shrekctl verdict lines (${mode}) ---"
  grep -aE 'oniond:|shrek onion' "$LOG" | tail -20 || true
  GATE_OK=0
  "$assert_fn"
  # O4 holds on every gate: shrekctl must render the record back on the console.
  has "shrek onion —" && check "O4 shrekctl onion status rendered the audit record" 0 || check "O4 shrekctl onion status rendered the audit record" 1
  [ "$GATE_OK" = 0 ] && echo ">>> gate ${mode}: PASS" || echo ">>> gate ${mode}: FAIL (inspect ${LOG})"
  return "$GATE_OK"
}

assert_good() {
  has "oniond: shrek-hello (sysext) -> merged"  && check "O1 shrek-hello sysext MERGED"  0 || check "O1 shrek-hello sysext MERGED"  1
  has "oniond: shrek-conf (confext) -> merged"  && check "O1 shrek-conf confext MERGED"  0 || check "O1 shrek-conf confext MERGED"  1
}
assert_select() {
  has "oniond: shrek-hello (sysext) -> merged"                 && check "O2 enabled shrek-hello MERGED"          0 || check "O2 enabled shrek-hello MERGED"          1
  has "oniond: shrek-extra (sysext) -> omitted (not-enabled)"  && check "O2 unselected shrek-extra OMITTED"      0 || check "O2 unselected shrek-extra OMITTED"      1
  absent "oniond: shrek-extra (sysext) -> merged"              && check "O2 shrek-extra did NOT merge (leak check)" 0 || check "O2 shrek-extra did NOT merge (leak check)" 1
}
assert_refused() { # unsigned + tamper share this
  has "oniond: shrek-hello (sysext) -> refused" && check "O3 shrek-hello REFUSED"        0 || check "O3 shrek-hello REFUSED"        1
  absent "oniond: shrek-hello (sysext) -> merged" && check "O3 shrek-hello did NOT merge" 0 || check "O3 shrek-hello did NOT merge" 1
}

GATE="${1:-all}"
rc=0
case "$GATE" in
  good|O1)      run_gate good     assert_good    || rc=1 ;;
  select|O2)    run_gate select   assert_select  || rc=1 ;;
  unsigned|O3a) run_gate unsigned assert_refused || rc=1 ;;
  tamper|O3b)   run_gate tamper   assert_refused || rc=1 ;;
  all)
    run_gate good     assert_good    || rc=1
    run_gate select   assert_select  || rc=1
    run_gate unsigned assert_refused || rc=1
    run_gate tamper   assert_refused || rc=1
    ;;
  *) echo "usage: $0 [all|good|select|unsigned|tamper]" >&2; exit 1 ;;
esac
echo "################ oniond-proof: $([ "$rc" = 0 ] && echo ALL PASS ✅ || echo SOME FAILED ❌) ################"
exit "$rc"
