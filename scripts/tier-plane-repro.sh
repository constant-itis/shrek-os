#!/usr/bin/env bash
# tier-plane-repro.sh — SPIKE-ONLY proof of the Phase-5 slice-2 decision plane (strip before ship).
#
# Exercises the resolver→broker handoff and gatekeeperd's independent re-check WITHOUT privilege:
# every refusal returns before the mount namespace is touched, so this runs on any host. Actual T1
# construction (the "cleared" path) is the VM/oracle gate, not this script.
#
# Proves: G3 (a ≥T2 workload is REFUSED, never run at T1), G4 (forged downgrade refused by the
# independent recompute), G5 (garbage trust ⇒ T-hostile ⇒ refused), plus caps⊆profile and the
# no-egress-plane fail-closed gate. Maps decision codes: 10 downgrade, 11 caps-exceed, 12
# no-constructor, 13 no-egress, 14 bad-request.
set -u
cd "$(dirname "$0")/.." || exit 2
cargo build --workspace -q || { echo "BUILD FAIL"; exit 2; }
A=target/debug/agentd
G=target/debug/gatekeeperd
DUMMY="--anchor /tmp --grant x -- /bin/true"
fails=0
ck() { # ck <label> <expected-exit> <expected-grep> <actual-exit> <output-file>
  local label=$1 exp=$2 pat=$3 got=$4 out=$5
  if [ "$got" != "$exp" ]; then echo "FAIL [$label] exit=$got want=$exp"; sed 's/^/    /' "$out"; fails=$((fails+1)); return; fi
  if [ -n "$pat" ] && ! grep -q "$pat" "$out"; then echo "FAIL [$label] missing /$pat/"; sed 's/^/    /' "$out"; fails=$((fails+1)); return; fi
  echo "PASS [$label] exit=$got  ($pat)"
}
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT

# --- agentd resolver (unprivileged) ---
$A resolve --trust T-pinned --caps C-proj-rw --anchor /srv --grant foo -- /bin/echo hi >"$T/a1o" 2>"$T/a1e"
ck "agentd:constructible resolves T1" 0 "tier=T1" $? "$T/a1e"
grep -q -- "--tier T1" "$T/a1o" || { echo "FAIL [agentd:stdout carries --tier T1]"; fails=$((fails+1)); }
$A resolve --trust T-first --caps C-broad --profile C-proj-rw --anchor /srv --grant foo -- /bin/true >"$T/a2o" 2>"$T/a2e"
ck "agentd:caps-exceed refused (no request emitted)" 11 "caps-exceed-profile" $? "$T/a2e"
[ -s "$T/a2o" ] && { echo "FAIL [agentd:refusal must emit no stdout]"; fails=$((fails+1)); } || echo "PASS [agentd:refusal emits empty stdout]"
$A resolve --trust NONSENSE --caps C-ro-nosec --anchor /srv --grant foo -- /bin/true >"$T/a3o" 2>"$T/a3e"
ck "agentd:garbage trust ⇒ T-hostile ⇒ T2" 0 "tier=T2 trust=T-hostile" $? "$T/a3e"

# --- gatekeeperd independent re-check (unprivileged refusals) ---
$G sandbox --tier T0 --trust T-hostile --caps C-net $DUMMY >"$T/g4" 2>&1
ck "G4:forged downgrade T0 for hostile/net" 10 "downgrade-below-floor bound=T3" $? "$T/g4"
$G sandbox --tier T2 --trust T-untrust --caps C-net --profile C-net $DUMMY >"$T/g3" 2>&1
ck "G3:->T2 refused, NOT run at T1" 12 "no-constructor-T2" $? "$T/g3"
$G sandbox --tier T1 --trust T-first --caps C-net --profile C-proj-rw $DUMMY >"$T/g5" 2>&1
ck "caps-exceed-profile at broker" 11 "caps-exceed-profile" $? "$T/g5"
$G sandbox --tier T1 --trust T-first --caps C-net --profile C-net $DUMMY >"$T/g6" 2>&1
ck "no-egress-plane fail-closed" 13 "no-egress-plane" $? "$T/g6"
$G sandbox --tier T9 --trust T-first --caps C-ro-nosec $DUMMY >"$T/g7" 2>&1
ck "bad-request tier" 14 "bad-request-tier" $? "$T/g7"

# --- end-to-end: garbage trust survives the whole pipeline as a refusal ---
REQ=$($A resolve --trust NONSENSE --caps C-net --anchor /srv --grant foo -- /bin/true 2>/dev/null)
$G sandbox $REQ >"$T/e2e" 2>&1
ck "e2e:garbage trust ⇒ hostile ⇒ refused at broker" 12 "no-constructor" $? "$T/e2e"

# --- cleared path prints the decision (construction itself is the VM/oracle gate) ---
$G sandbox --tier T1 --trust T-pinned --caps C-proj-rw --profile C-proj-rw $DUMMY >"$T/cl" 2>&1
if grep -q "SANDBOX-DECISION cleared construct-at=T1 effective=T1" "$T/cl" && ! grep -q "refused" "$T/cl"; then
  echo "PASS [cleared:T-pinned/C-proj-rw ⇒ cleared for T1 construction]"
else
  echo "FAIL [cleared path]"; sed 's/^/    /' "$T/cl"; fails=$((fails+1))
fi

echo "----"
[ "$fails" = 0 ] && { echo "ALL DECISION-PLANE CHECKS PASS"; exit 0; } || { echo "$fails CHECK(S) FAILED"; exit 1; }
