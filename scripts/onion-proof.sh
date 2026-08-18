#!/usr/bin/env bash
# Shrek OS Phase-2 — prove the Onion: signed sysext/confext layers merge onto the sealed base, and a
# bad layer is refused (docs/phase2-onion.md). Reuses the Phase-1 sealed root (built once with the
# Phase-2 machinery: shrek-onion.service + masks + /usr/lib/verity.d/shrek.crt) and swaps only the
# cheap layer-store disk between gates.
#
#   L1/L2/L4  good store      → sysext MERGED (into sealed /usr) + confext MERGED (into sealed /etc)
#   L3a       unsigned store  → sysext REFUSED (verity present but no PKCS#7 signature)
#   L3b       tamper store    → sysext REFUSED (byte-flipped → verity roothash mismatch)
#
# The verdict is read from shrek-onion.service's ExecStartPost lines in out/vm-console.log.
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"
mkdir -p out
ROOT_RAW="out/shrek_1_x86-64.raw"
REBUILD_ROOT="${REBUILD_ROOT:-0}"     # 1 = rebuild the sealed root (needed after changing the overlay)
BUDGET="${BUDGET:-150}"

if [ "$REBUILD_ROOT" = 1 ] || [ ! -f "$ROOT_RAW" ]; then
  echo "############ building the Phase-2 sealed root ############"
  scripts/build-in-container.sh 1 2>&1 | tee out/onion-root.log
fi

# Run one gate: $1 = layer-store mode (good|unsigned|tamper); $2 = expected sysext verdict (MERGED|REFUSED)
run_gate() {
  local mode="$1" expect="$2" log="out/vm-console.log"
  echo "############ gate: store=${mode}, expect sysext ${expect} ############"
  scripts/build-layers.sh "$mode" 2>&1 | tee "out/onion-layers-${mode}.log" >/dev/null
  STORE=out/layer-store.raw RAW="$ROOT_RAW" scripts/boot-vm.sh "$BUDGET"
  echo "--- shrek-onion verdict lines (${mode}) ---"
  grep -aE 'shrek-onion:' "$log" | tail -6 || true
  local got="REFUSED"
  grep -aq 'shrek-onion: sysext MERGED' "$log" && got="MERGED"
  if [ "$got" = "$expect" ]; then
    echo "PASS ✅  store=${mode}: sysext ${got} (expected ${expect})"
    [ "$mode" = good ] && { grep -aq 'shrek-onion: confext MERGED' "$log" \
        && echo "PASS ✅  store=good: confext MERGED (L4)" \
        || echo "CHECK ⚠  store=good: confext not merged (L4) — inspect ${log}"; }
    return 0
  fi
  echo "CHECK ⚠  store=${mode}: sysext ${got} (expected ${expect}) — inspect ${log}"
  return 1
}

GATE="${1:-all}"
case "$GATE" in
  good|L1|L2|L4) run_gate good MERGED ;;
  unsigned|L3a) run_gate unsigned REFUSED ;;
  tamper|L3b)   run_gate tamper REFUSED ;;
  all)
    run_gate good     MERGED  || true
    run_gate unsigned REFUSED || true
    run_gate tamper   REFUSED || true
    ;;
  *) echo "usage: $0 [all|good|unsigned|tamper]" >&2; exit 1 ;;
esac
