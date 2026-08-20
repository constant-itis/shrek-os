#!/usr/bin/env bash
# Regression for finding F1 (docs/phase5-consolidation.md §2): the `pin-verity` fixture surface
# (privileged FS_IOC_ENABLE_VERITY + its CLI dispatch) MUST be compiled OUT of a default/production
# build and present ONLY under `--features spike`. This proves the command is unavailable in the
# shipped artifact — not merely undocumented, but absent from the binary.
#
# Method: build gatekeeperd both ways, then assert the pin-verity CLI usage-string literal (which lives
# inside `pin_verity_cli`, the gated function) is ABSENT from the default binary and PRESENT in the
# spike binary. The string is a faithful proxy for "the dispatch + enable_verity code is compiled in":
# if the function is cfg'd out, its literal cannot survive.
set -u
cd "$(git rev-parse --show-toplevel)" || exit 3
PASS=0; FAIL=0
ok(){ echo "SPIKE-STRIP: PASS $1"; PASS=$((PASS+1)); }
no(){ echo "SPIKE-STRIP: FAIL $1"; FAIL=$((FAIL+1)); }
NEEDLE='usage: gatekeeperd pin-verity'

echo "=== build DEFAULT (production) ==="
cargo build --release -p gatekeeperd >/dev/null 2>&1 || { echo "default build failed"; exit 3; }
DEF=target/release/gatekeeperd
if strings -a "$DEF" | grep -qF "$NEEDLE"; then
  no "default build STILL contains the pin-verity surface (F1 not stripped)"
else
  ok "default build has NO pin-verity surface (string absent from artifact)"
fi

echo "=== build SPIKE (oracle/VM gate) ==="
cargo build --release -p gatekeeperd --features spike >/dev/null 2>&1 || { echo "spike build failed"; exit 3; }
SPK=target/release/gatekeeperd
if strings -a "$SPK" | grep -qF "$NEEDLE"; then
  ok "spike build DOES contain the pin-verity surface (feature opt-in works)"
else
  no "spike build is MISSING the pin-verity surface (feature broken)"
fi

# Leave the tree on the SPIKE build (the oracle/VM consumers expect the fixture verb present).
echo "=== SPIKE-STRIP summary: $PASS passed, $FAIL failed ==="
[ "$FAIL" -eq 0 ] || exit 1
