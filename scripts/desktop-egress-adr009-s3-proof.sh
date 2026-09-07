#!/usr/bin/env bash
# Shrek OS — ADR-009 v2 S3: host oracle for the UPDATE-TIME collision quarantine (§4.4 layer 3).
#
# S3's other half — the gatekeeperd `manifest-install/remove` ceremony (card render + storage-host warning
# + the "toggle ≠ live intent" line) — is a SAK/VT console flow: its pure precheck/render/commit-mapping is
# unit-tested (gatekeeperd desktop_egress + consent lib tests) and the live seat ceremony is the S6 sealed-
# VM gate, exactly like ADR-007 S4. What a host oracle CAN prove without a seat is the runtime quarantine:
# the daemon's boot reconcile, over the REAL two-dir catalog loader + store, disabling an owner capability
# whose host became reserved by a system update.
#
#   Q-clean       an owner capability reaching a benign host boots ACTIVE (no quarantine, fault=-)
#   Q-update      after an OS update ships a SEALED capability that now reserves that same host, the next
#                 boot QUARANTINES the owner capability (state fault=quarantined; the store fault names the
#                 now-reserved host) — never a silent allow (§4.4 layer 3)
#   Q-legible     the owner capability stays present + legible (source=owner) — disabled, not vanished
#   Q-sealed-safe the SEALED capability that caused the collision is itself never quarantined (only owner
#                 pins are §4.4 subjects)
#
# Uses the `oracle-env` build's SHREK_EGRESS_* overrides + `unshare -rn` so `egressd reconcile` runs as
# euid 0 in an isolated netns (its @cap_pinned apply harmlessly no-ops against the absent baked table —
# the deny floor stands — while the quarantine + state projection, which touch only the store, run fully).
set -uo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

PASS=0; FAIL=0
check() { if [ "$3" -eq 0 ]; then echo "  PASS $1 — $2"; PASS=$((PASS+1)); else echo "  FAIL $1 — $2"; FAIL=$((FAIL+1)); fi; }

echo "=== building egressd (release, oracle-env) ==="
CARGO_NET_OFFLINE=true cargo build --release -p egressd --features oracle-env >/dev/null 2>&1 || \
  cargo build --release -p egressd --features oracle-env
B="$REPO_ROOT/target/release/egressd"

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
S="$WORK/store"
ERUN="$WORK/run/egress"
HRUN="$WORK/run"
HHOME="$WORK/home"
SEALED="$WORK/cap-sealed"; OWNER="$WORK/cap-owner"; STAGING="$WORK/cap-staging"
mkdir -p "$ERUN" "$HHOME" "$SEALED" "$OWNER" "$STAGING"

export SHREK_EGRESS_STORE="$S" SHREK_EGRESS_RUN="$ERUN"
export SHREK_HOSTS_HOME="$HHOME" SHREK_HOSTS_RUN="$HRUN"
export SHREK_EGRESS_CAP_SEALED="$SEALED" SHREK_EGRESS_CAP_OWNER="$OWNER" SHREK_EGRESS_CAP_STAGING="$STAGING"
"$B" store init >/dev/null

STATE="$ERUN/state"
# `egressd reconcile` is euid-0-only + touches nft; run it in a user+net namespace. nft errors (no baked
# table) are expected + caught inside reconcile — the quarantine/state work still completes.
reconcile() { unshare -rn "$B" reconcile >/dev/null 2>&1; }

# An owner capability the owner installed earlier, reaching a benign host (deliver none, tier one-click).
cat > "$OWNER/radar.capability" <<'EOF'
schema shrek-egress-capability/1
name radar
title Rain Radar
purpose Local precipitation radar
feature dms:radar
tier one-click
deliver none
host radar.example.test tcp 443
EOF

echo "=== 1. clean boot — owner cap active, not quarantined ==="
reconcile
grep -q '^profile radar .*fault=- source=owner feature=dms:radar' "$STATE"
check "Q-clean" "owner cap boots active (fault=-)" $?

echo "=== 2. an OS update ships a SEALED cap that now reserves radar's host ==="
# The §4.4 layer-3 scenario: safe-when-installed becomes root-adjacent six updates later. The new sealed
# data manifest (NOT in the compiled table — proves host_reserved_by_system consults the sealed CATALOG)
# reaches the very host the owner cap already names.
cat > "$SEALED/rainmap.capability" <<'EOF'
schema shrek-egress-capability/1
name rainmap
title Rain Map
purpose Sealed precipitation tiles
feature dms:rainmap
tier one-click
deliver hosts
host radar.example.test tcp 443
EOF
reconcile

grep -q '^profile radar .*fault=quarantined' "$STATE"
check "Q-update" "owner cap quarantined after the colliding update" $?

grep -q 'radar.example.test' "$S/fault/radar" 2>/dev/null
check "Q-legible-reason" "the quarantine fault names the now-reserved host" $?

grep -q '^profile radar .*source=owner feature=dms:radar' "$STATE"
check "Q-legible" "quarantined owner cap stays present + legible (source=owner)" $?

grep -q '^profile rainmap .*fault=- source=sealed' "$STATE"
check "Q-sealed-safe" "the sealed cap that caused the collision is never quarantined" $?

echo
echo "=== ADR-009 S3 quarantine oracle: $PASS passed, $FAIL failed ==="
[ "$FAIL" -eq 0 ]
