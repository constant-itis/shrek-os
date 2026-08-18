#!/usr/bin/env bash
# Phase-5 slice-2 — construction proof through the DECISION PLANE, in the privileged debian:trixie
# oracle (the fast path before the VM gate). SPIKE-ONLY (strip before ship).
#
# Two things the host repro cannot show because they need real nspawn:
#   G2  a CLEARED request (T-pinned/C-proj-rw ⇒ T1) actually constructs at T1 and the slice-1
#       caps property still holds through the new code path (granted-out path ENOENT).
#   G3* a REFUSED request (⇒T2) emits ZERO SHREK_GATE lines — the workload NEVER runs. This is the
#       load-bearing negative: a ≥T2 requirement is failed closed, NOT silently run in a T1 box.
set -uo pipefail

if [ "${IN_CONTAINER:-0}" != "1" ]; then
  REPO_ROOT="$(git rev-parse --show-toplevel)"
  echo "=== building release gatekeeperd (host) ==="
  ( cd "$REPO_ROOT" && cargo build --release -p gatekeeperd ) || exit 3
  echo "=== launching privileged debian:trixie oracle ==="
  exec docker run --rm --privileged -e IN_CONTAINER=1 \
    -v "$REPO_ROOT/scripts/tier-plane-proof.sh:/proof.sh:ro" \
    -v "$REPO_ROOT/target/release/gatekeeperd:/gatekeeperd:ro" \
    debian:trixie bash /proof.sh
fi

# ---------------- inside the container ----------------
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
apt-get install -y --no-install-recommends systemd-container >/dev/null 2>&1
mount --make-rshared / 2>/dev/null || true
mount -t tmpfs tmpfs /run 2>/dev/null || true

mkdir -p /srv/project /srv/vault
echo PROJECT > /srv/project/marker
echo VAULT   > /srv/vault/marker

PROBE='
m=$(cat /srv/project/marker 2>/dev/null)
[ "$m" = PROJECT ] && echo "SHREK_GATE: PASS gate=in-project-readable marker=$m" || echo "SHREK_GATE: FAIL gate=in-project-readable marker=$m"
if cat /srv/vault/marker 2>&1 | grep -qi "No such file"; then echo "SHREK_GATE: PASS gate=in-vault-enoent"; else echo "SHREK_GATE: FAIL gate=in-vault-enoent (vault reachable!)"; fi
l=$(ls /srv | tr "\n" ,)
[ "$l" = "project," ] && echo "SHREK_GATE: PASS gate=in-vault-absent ls=$l" || echo "SHREK_GATE: FAIL gate=in-vault-absent ls=$l"
o=$(stat -c %u /srv/project/marker)
[ "$o" = 65534 ] && echo "SHREK_GATE: PASS gate=in-private-users host-root=nobody($o)" || echo "SHREK_GATE: FAIL gate=in-private-users host-root-uid=$o"
'
fails=0

echo "=== G2: CLEARED T-pinned/C-proj-rw ⇒ construct at T1 (decision plane) ==="
OUT=$(/gatekeeperd sandbox --tier T1 --trust T-pinned --caps C-proj-rw --profile C-proj-rw \
        --id s2 --anchor /srv --grant project -- /bin/sh -c "$PROBE" 2>&1)
echo "$OUT" | sed 's/^/    /'
echo "$OUT" | grep -q "SANDBOX-DECISION cleared construct-at=T1 effective=T1" \
  && echo "PASS [decision=cleared emitted]" || { echo "FAIL [no cleared decision]"; fails=$((fails+1)); }
# Anchor to line-start: gatekeeperd's `exec {cmd:?}` debug line echoes the probe SOURCE (which
# contains literal "SHREK_GATE: PASS/FAIL" strings mid-line) — only real gate RESULTS start the line.
PASS=$(printf '%s\n' "$OUT" | grep -c '^SHREK_GATE: PASS')
FAILN=$(printf '%s\n' "$OUT" | grep -c '^SHREK_GATE: FAIL')
{ [ "$FAILN" = 0 ] && [ "$PASS" -ge 4 ]; } \
  && echo "PASS [G2 constructed at T1, $PASS caps gates through decision plane]" \
  || { echo "FAIL [G2 construction/caps gates: pass=$PASS fail=$FAILN]"; fails=$((fails+1)); }

echo
echo "=== G3*: REFUSED ⇒T2 emits ZERO SHREK_GATE lines (workload never runs) ==="
OUT=$(/gatekeeperd sandbox --tier T2 --trust T-untrust --caps C-net --profile C-net \
        --id s2r --anchor /srv --grant project -- /bin/sh -c "$PROBE" 2>&1); RC=$?
echo "$OUT" | sed 's/^/    /'
echo "  exit=$RC"
[ "$RC" = 12 ] && echo "PASS [refused exit=12 no-constructor]" || { echo "FAIL [want exit 12, got $RC]"; fails=$((fails+1)); }
GATES=$(printf '%s\n' "$OUT" | grep -c '^SHREK_GATE')
[ "$GATES" = 0 ] && echo "PASS [G3* workload NEVER ran — 0 SHREK_GATE lines, fail-closed]" \
  || { echo "FAIL [G3* workload ran despite refusal: $GATES gate lines]"; fails=$((fails+1)); }

echo "----"
if [ "$fails" = 0 ]; then echo "SHREK_GATE: PASS slice-2 decision-plane construction proof"; exit 0
else echo "SHREK_GATE: FAIL slice-2 ($fails)"; exit 1; fi
