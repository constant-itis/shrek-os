#!/usr/bin/env bash
# Shrek OS Phase-4 (slice 2) — prove gatekeeperd privilege-separates the merge (docs/phase4-gatekeeperd.md).
# Rebuilds the sealed root ONCE with the slice-2 machinery (gatekeeperd broker + units + the shrek user
# + non-root oniond), then swaps the cheap layer store per gate. Verdict from oniond/gatekeeperd/shrekctl
# lines in out/vm-console.log.
#
#   G1  good store    → oniond runs non-root + a DIRECT merge probe is DENIED ("Need to be privileged")
#   G2  good/select/  → merge outcomes hold THROUGH the broker (hello+conf merged; extra omitted;
#       unsigned/tamper  unsigned+tampered refused)
#   G3  inject store  → a compromised oniond requests shrek-extra → wall REFUSES (not-sealed-policy)
#   G4  no store      → nothing merges, oniond fails-closed, system still reaches login
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"
mkdir -p out
ROOT_RAW="out/shrek_1_x86-64.raw"
REBUILD_ROOT="${REBUILD_ROOT:-0}"    # 1 = rebuild the sealed root (needed: overlay/units/user changed)
BUDGET="${BUDGET:-180}"
LOG="out/vm-console.log"

if [ "$REBUILD_ROOT" = 1 ] || [ ! -f "$ROOT_RAW" ]; then
  echo "############ building the Phase-4 slice-2 sealed root (gatekeeperd + non-root oniond) ############"
  scripts/build-in-container.sh 1 2>&1 | tee out/gk-root.log
fi

has()    { grep -aqF "$1" "$LOG"; }
hasre()  { grep -aqE "$1" "$LOG"; }
absent() { ! grep -aqF "$1" "$LOG"; }
check()  { if [ "$2" = 0 ]; then echo "PASS ✅  $1"; else echo "FAIL ❌  $1"; GATE_OK=1; fi; }

# run_gate <mode|nostore> <assert-fn>
run_gate() {
  local mode="$1" assert_fn="$2"
  echo "############ gate: ${mode} ############"
  local store_arg=""
  if [ "$mode" != "nostore" ]; then
    scripts/build-layers.sh "$mode" 2>&1 | tee "out/gk-layers-${mode}.log" >/dev/null
    store_arg="out/layer-store.raw"
  fi
  STORE="$store_arg" RAW="$ROOT_RAW" scripts/boot-vm.sh "$BUDGET"
  echo "--- broker / oniond / shrekctl lines (${mode}) ---"
  grep -aE 'oniond:|gatekeeperd:|shrek onion' "$LOG" | tail -24 || true
  GATE_OK=0
  "$assert_fn"
  # every successful boot reaches a login prompt (OS availability)
  hasre 'login:' && check "system reached login prompt" 0 || check "system reached login prompt" 1
  [ "$GATE_OK" = 0 ] && echo ">>> ${mode}: PASS" || echo ">>> ${mode}: FAIL (inspect ${LOG})"
  return "$GATE_OK"
}

assert_good() {
  # G1 — privilege dropped
  hasre 'oniond: running as uid=[1-9]' && check "G1 oniond runs non-root" 0 || check "G1 oniond runs non-root" 1
  has "oniond: privilege probe DENIED" && check "G1 direct merge DENIED (privilege gone)" 0 || check "G1 direct merge DENIED (privilege gone)" 1
  # G2 — parity through the broker
  has "oniond: shrek-hello (sysext) -> merged" && check "G2 shrek-hello MERGED via broker" 0 || check "G2 shrek-hello MERGED via broker" 1
  has "oniond: shrek-conf (confext) -> merged" && check "G2 shrek-conf MERGED via broker" 0 || check "G2 shrek-conf MERGED via broker" 1
}
assert_select() {
  has "oniond: shrek-hello (sysext) -> merged"                && check "G2 shrek-hello MERGED" 0 || check "G2 shrek-hello MERGED" 1
  has "oniond: shrek-extra (sysext) -> omitted (not-enabled)" && check "G2 shrek-extra OMITTED (not sealed)" 0 || check "G2 shrek-extra OMITTED (not sealed)" 1
}
assert_inject() {
  has "oniond: INJECT" && check "G3 compromised oniond injected a request" 0 || check "G3 compromised oniond injected a request" 1
  has "oniond: shrek-extra (sysext) -> refused (not-sealed-policy)" && check "G3 wall REFUSED the injected layer" 0 || check "G3 wall REFUSED the injected layer" 1
  absent "oniond: shrek-extra (sysext) -> merged" && check "G3 injected layer did NOT merge" 0 || check "G3 injected layer did NOT merge" 1
}
assert_refused() {
  has "oniond: shrek-hello (sysext) -> refused" && check "G2 unsigned/tampered REFUSED" 0 || check "G2 unsigned/tampered REFUSED" 1
  absent "oniond: shrek-hello (sysext) -> merged" && check "G2 bad layer did NOT merge" 0 || check "G2 bad layer did NOT merge" 1
}
assert_failclosed() {
  absent "oniond: shrek-hello (sysext) -> merged" && check "G4 nothing merged with no store" 0 || check "G4 nothing merged with no store" 1
  hasre 'no layer store mounted|broker unavailable|-> absent' && check "G4 fail-closed reported" 0 || check "G4 fail-closed reported" 1
}

GATE="${1:-all}"
rc=0
case "$GATE" in
  good|G1|G2) run_gate good     assert_good      || rc=1 ;;
  select)     run_gate select   assert_select    || rc=1 ;;
  inject|G3)  run_gate inject    assert_inject    || rc=1 ;;
  unsigned)   run_gate unsigned  assert_refused   || rc=1 ;;
  tamper)     run_gate tamper    assert_refused   || rc=1 ;;
  nostore|G4) run_gate nostore   assert_failclosed|| rc=1 ;;
  all)
    run_gate good     assert_good       || rc=1
    run_gate select   assert_select     || rc=1
    run_gate inject   assert_inject     || rc=1
    run_gate unsigned assert_refused    || rc=1
    run_gate tamper   assert_refused    || rc=1
    run_gate nostore  assert_failclosed || rc=1
    ;;
  *) echo "usage: $0 [all|good|select|inject|unsigned|tamper|nostore]" >&2; exit 1 ;;
esac
echo "################ gatekeeperd-proof: $([ "$rc" = 0 ] && echo ALL PASS ✅ || echo SOME FAILED ❌) ################"
exit "$rc"
