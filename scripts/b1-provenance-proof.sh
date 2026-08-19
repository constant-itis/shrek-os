#!/usr/bin/env bash
# b1-provenance-proof.sh — SPIKE-ONLY host oracle for Phase-5 slice-7 (B1 trust-band provenance).
#
# Runs UNPRIVILEGED on any host: every assertion is a gatekeeperd DECISION that returns before the
# mount namespace is touched, so no root / no dm-verity is needed. Because this host has no sealed
# verity root, `sealed_root_dev()` is None and EVERY derivation fails high to T-hostile — which is
# exactly what we assert here. The POSITIVE T-first arm (real dm-verity ⇒ derived T-first ⇒ T0/T1) is
# owned by the sealed VM gate (image/overlay/usr/lib/shrek/mount-plane-gate S2/S3/S4), never faked
# here. This oracle owns: (1) fail-high, (2) anti-spoof (a caller proposing T-first is corrected DOWN
# to T-hostile), (3) the audited proposal/derivation MISMATCH line.
set -u
cd "$(dirname "$0")/.." || exit 2
cargo build --workspace -q || { echo "BUILD FAIL"; exit 2; }
G=target/debug/gatekeeperd
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
fails=0
ck() { # ck <label> <expected-exit> <grep-pattern> <actual-exit> <output-file>
  local label=$1 exp=$2 pat=$3 got=$4 out=$5
  if [ "$got" != "$exp" ]; then echo "FAIL [$label] exit=$got want=$exp"; sed 's/^/    /' "$out"; fails=$((fails+1)); return; fi
  if [ -n "$pat" ] && ! grep -q "$pat" "$out"; then echo "FAIL [$label] missing /$pat/"; sed 's/^/    /' "$out"; fails=$((fails+1)); return; fi
  echo "PASS [$label] exit=$got  ($pat)"
}

echo "== B1: the band is DERIVED, --trust is only an audited proposal =="

# (1) ANTI-SPOOF: a caller PROPOSES T-first over an unsealed entrypoint and requests the weak T0 wall.
# Derivation ignores the proposal, measures the entrypoint (no sealed root here) ⇒ T-hostile, whose
# floor is T2, so the T0 request is a forbidden downgrade (code 10). The proposal bought nothing.
$G sandbox --tier T0 --trust T-first --caps C-ro-nosec --profile C-ro-nosec \
   --anchor /tmp --grant x -- /usr/bin/true >"$T/spoof" 2>&1
ck "anti-spoof: proposed T-first over unsealed ⇒ derived T-hostile, T0 refused" \
   10 "downgrade-below-floor bound=T2" $? "$T/spoof"
grep -q 'SANDBOX-PROVENANCE derived=T-hostile proposed=T-first match=false' "$T/spoof" \
  && echo "PASS [anti-spoof: audited mismatch line derived=T-hostile proposed=T-first match=false]" \
  || { echo "FAIL [anti-spoof: no mismatch audit line]"; sed 's/^/    /' "$T/spoof"; fails=$((fails+1)); }

# (2) FAIL-HIGH on an unresolvable entrypoint: measurement error ⇒ T-hostile, identical to "measured
# hostile" (the broker never distinguishes could-not-measure from measured-hostile).
$G sandbox --tier T0 --trust T-first --caps C-ro-nosec --profile C-ro-nosec \
   --anchor /tmp --grant x -- /nonexistent/tool >"$T/miss" 2>&1
ck "fail-high: unresolvable entrypoint ⇒ T-hostile ⇒ T0 refused" 10 "downgrade-below-floor bound=T2" $? "$T/miss"
grep -q 'entrypoint_sealed=false' "$T/miss" \
  && echo "PASS [fail-high: entrypoint_sealed=false recorded]" \
  || { echo "FAIL [fail-high: entrypoint_sealed flag]"; fails=$((fails+1)); }

# (3) NO FAKED T-FIRST IN THE ORACLE: even the ENROLLED closed-world path derives T-hostile here,
# because there is no sealed verity root to measure against (sealed_root=None). Proves the oracle
# cannot manufacture T-first (that is the VM's job). Requesting its T-first floor (T0) is then a
# forbidden downgrade below the derived T-hostile floor (T2).
$G sandbox --tier T0 --trust T-first --caps C-ro-nosec --profile C-ro-nosec \
   --anchor /tmp --grant x -- /usr/libexec/shrek/gate-probe >"$T/nofake" 2>&1
ck "no-faked-T-first: enrolled path off-verity ⇒ derived T-hostile, T0 refused" \
   10 "downgrade-below-floor bound=T2" $? "$T/nofake"
grep -q 'SANDBOX-PROVENANCE derived=T-hostile.*sealed_root=None' "$T/nofake" \
  && echo "PASS [no-faked-T-first: derived=T-hostile (sealed_root=None) even for the closed-world path]" \
  || { echo "FAIL [no-faked-T-first: derivation]"; sed 's/^/    /' "$T/nofake"; fails=$((fails+1)); }

# (4) A T-hostile workload with C-ro-nosec is the ONE constructible low cell for hostile code (its
# floor); the decision clears to T2 (construction itself is the VM/oracle gate). Proposal omitted ⇒
# proposed parses to T-hostile too ⇒ match=true (no spurious mismatch noise for honest requests).
$G sandbox --tier T2 --caps C-ro-nosec --profile C-ro-nosec \
   --anchor /tmp --grant x -- /usr/libexec/shrek/gate-probe >"$T/clear" 2>&1
if grep -q 'SANDBOX-PROVENANCE derived=T-hostile proposed=T-hostile match=true' "$T/clear"; then
  echo "PASS [honest hostile request: match=true, no spurious mismatch]"
else
  echo "FAIL [honest hostile request mismatch line]"; sed 's/^/    /' "$T/clear"; fails=$((fails+1))
fi

echo "----"
[ "$fails" = 0 ] && { echo "ALL B1 PROVENANCE CHECKS PASS"; exit 0; } || { echo "$fails CHECK(S) FAILED"; exit 1; }
