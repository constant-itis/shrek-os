#!/usr/bin/env bash
# Shrek OS — ADR-007 S4: host oracle for the CONSOLE-CEREMONY egress tier (web-browsing + raw).
# Proves the S4 ENGINE fast, without a VM, via the `oracle-env` build's SHREK_EGRESS_* overrides. What it
# does NOT prove — and cannot, headless — is the SAK/VT ceremony itself (no seat, no kernel VT): that is
# the sealed-VM gate (Q3, S6). This oracle exercises the root-only `egressd confirmed-*` engine (what the
# ceremony execs on a confirmed OK) + the geteuid boundary + reboot survival, plus the gatekeeperd
# precheck/tier gate (unit-tested in-crate). The ceremony DECLINE path (nothing persists) is the pure
# decision logic in consent.rs, unit-tested there.
#
#   G-nonroot     uid-1000 cannot invoke confirmed-* (geteuid gate) — refused, NOTHING written
#   G-badraw      a malformed raw triple is refused by the grammar — no store write, no element
#   G-raw-add     confirmed-add-raw (literal host) lands (ip . proto . port) in @raw_pinned, element-only
#   G-raw-multi   a second confirmed-add-raw adds its tuple; both live; intent is the flat TSV file
#   G-raw-remove  confirmed-remove-raw drops ONLY its tuple; the other survives; @raw_pinned reconciled
#   G-raw-survive a fresh `egressd reconcile` (reboot) re-adds the stored raw tuples — never flushes
#   G-wb-bless    confirmed-bless web-browsing persists tier=ceremony; state shows blessed=1; pending
#                 (no browser rule — the slice is absent in the oracle; browser-up installs it at launch)
#   G-wb-unbless  confirmed-unbless web-browsing removes the record + tears down any browser rules
#   G-wb-tier     confirmed-bless of a NON-ceremony profile (weather) is refused (tier-matrix integrity)
set -uo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

PASS=0; FAIL=0
check() { if [ "$3" -eq 0 ]; then echo "  PASS $1 — $2"; PASS=$((PASS+1)); else echo "  FAIL $1 — $2"; FAIL=$((FAIL+1)); fi; }

echo "=== building egressd (release, oracle-env) ==="
CARGO_NET_OFFLINE=true cargo build --release -p egressd --features oracle-env >/dev/null 2>&1 || \
  cargo build --release -p egressd --features oracle-env
B="$REPO_ROOT/target/release/egressd"
NFT_FILE="$REPO_ROOT/image/overlay/usr/lib/shrek/desktop-egress.nft"

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
S="$WORK/store"; R="$WORK/run"; mkdir -p "$R"
export SHREK_EGRESS_STORE="$S" SHREK_EGRESS_RUN="$R"
"$B" store init >/dev/null

# ── boundary arena: the geteuid gate + grammar, run as the REAL (non-root) oracle user ───────────────
echo "=== boundary (non-root host) ==="
"$B" confirmed-add-raw 203.0.113.7:tcp:8443 >/dev/null 2>&1
rc=$?; [ "$rc" -eq 2 ] && [ ! -s "$S/raw" ]; check "G-nonroot" "uid-1000 confirmed-* refused (rc=$rc), no raw written" $?

# ── enforcement arena: a netns (userns maps us to root ⇒ geteuid==0) with the baked table loaded ─────
echo "=== A. confirmed-* engine + @raw_pinned enforcement (netns as root) ==="
A_OUT="$WORK/a.out"
unshare -rn sh -c '
  # NO `set -e`: several steps below intentionally FAIL (bad triple, tier refusal) and we capture their rc.
  B="'"$B"'"; NFT_FILE="'"$NFT_FILE"'"; S="'"$S"'"; R="'"$R"'"
  export SHREK_EGRESS_STORE="$S" SHREK_EGRESS_RUN="$R"
  rawview() { nft list set inet shrek_desktop_egress raw_pinned | tr -d "\n\t "; }
  chainview() { nft -a list chain inet shrek_desktop_egress output | tr -d "\t"; }

  nft -f "$NFT_FILE" || { echo "NFT-LOAD-FAIL"; exit 1; }

  # a malformed triple: refused by the grammar, no element, no store line
  "$B" confirmed-add-raw "-evil.com:tcp:443" >/dev/null 2>&1; echo "BADRAW_RC=$?"
  echo "BADRAW_STORE=$( [ -s "$S/raw" ] && echo yes || echo no )"

  # a good literal raw add ⇒ element present
  "$B" confirmed-add-raw 203.0.113.7:tcp:8443 >/dev/null 2>&1; echo "ADD1_RC=$?"
  echo "RAW_AFTER_ADD1=$(rawview)"

  # a second add ⇒ both tuples; intent is the flat TSV
  "$B" confirmed-add-raw 198.51.100.9:udp:53 >/dev/null 2>&1; echo "ADD2_RC=$?"
  echo "RAW_AFTER_ADD2=$(rawview)"
  echo "TSV_LINES=$(wc -l < "$S/raw" | tr -d " ")"

  # remove the first ⇒ only its tuple leaves; the second survives
  "$B" confirmed-remove-raw 203.0.113.7:tcp:8443 >/dev/null 2>&1; echo "RM_RC=$?"
  echo "RAW_AFTER_RM=$(rawview)"

  # REBOOT: reload the baked table (empties every set), then reconcile ⇒ the surviving tuple re-appears
  nft -f "$NFT_FILE"
  echo "RAW_AFTER_RELOAD=$(rawview)"
  "$B" reconcile >/dev/null 2>&1; echo "RECON_RC=$?"
  echo "RAW_AFTER_RECON=$(rawview)"

  # web-browsing ceremony bless ⇒ record persists (tier=ceremony), state blessed=1; no browser rule
  # (no shrek-browser.slice in the oracle ⇒ pending); a weather bless via the ceremony verb is refused.
  "$B" confirmed-bless web-browsing >/dev/null 2>&1; echo "WB_RC=$?"
  echo "WB_STATE=$(grep "^profile web-browsing" "$R/state" | tr -d "\n")"
  echo "WB_BROWSER_RULES=$(chainview | grep -c "shrek-browser.slice" || true)"
  "$B" confirmed-bless weather >/dev/null 2>&1; echo "WBTIER_RC=$?"

  # unbless web-browsing ⇒ record gone, teardown idempotent (0 browser rules)
  "$B" confirmed-unbless web-browsing >/dev/null 2>&1; echo "WBU_RC=$?"
  echo "WB_BLESSED_AFTER=$( [ -f "$S/blessed/web-browsing" ] && echo yes || echo no )"
' >"$A_OUT" 2>/dev/null
cat "$A_OUT" | sed 's/^/    /'

g() { grep -m1 "^$1=" "$A_OUT" | cut -d= -f2-; }

[ "$(g BADRAW_RC)" = "2" ] && [ "$(g BADRAW_STORE)" = "no" ]; check "G-badraw" "malformed triple refused, nothing written" $?
[ "$(g ADD1_RC)" = "0" ] && echo "$(g RAW_AFTER_ADD1)" | grep -q "203.0.113.7.tcp.8443"; check "G-raw-add" "literal raw pinned as (ip . proto . port)" $?
echo "$(g RAW_AFTER_ADD2)" | grep -q "198.51.100.9.udp.53" && echo "$(g RAW_AFTER_ADD2)" | grep -q "203.0.113.7.tcp.8443" && [ "$(g TSV_LINES)" = "2" ]; check "G-raw-multi" "both tuples live; intent is the flat TSV ($(g TSV_LINES) lines)" $?
echo "$(g RAW_AFTER_RM)" | grep -q "198.51.100.9.udp.53" && ! echo "$(g RAW_AFTER_RM)" | grep -q "203.0.113.7.tcp.8443"; check "G-raw-remove" "removed tuple gone, the other survives" $?
! echo "$(g RAW_AFTER_RELOAD)" | grep -q "198.51.100.9.udp.53" && echo "$(g RAW_AFTER_RECON)" | grep -q "198.51.100.9.udp.53"; check "G-raw-survive" "reload empties the set; reconcile re-adds the stored tuple" $?
[ "$(g WB_RC)" = "0" ] && echo "$(g WB_STATE)" | grep -q "tier=ceremony blessed=1" && [ "$(g WB_BROWSER_RULES)" = "0" ]; check "G-wb-bless" "web-browsing persisted tier=ceremony, pending (no slice ⇒ 0 browser rules)" $?
[ "$(g WBTIER_RC)" = "2" ]; check "G-wb-tier" "ceremony verb refuses a non-ceremony profile (weather)" $?
[ "$(g WBU_RC)" = "0" ] && [ "$(g WB_BLESSED_AFTER)" = "no" ]; check "G-wb-unbless" "unbless removed the record; teardown idempotent" $?

echo ""
echo "=== S4 oracle: PASS=$PASS FAIL=$FAIL ==="
[ "$FAIL" -eq 0 ]
