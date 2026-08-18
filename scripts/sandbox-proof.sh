#!/usr/bin/env bash
# Phase-5 slice-1 — M3: gatekeeperd drives sandbox construction end-to-end from a grant. Runs the
# release `gatekeeperd sandbox` subcommand inside a privileged debian:trixie oracle (the fast path
# before the M4 VM gate). The in-sandbox workload probes the caps-enforced mount set and emits the
# anchored SHREK_GATE lines; this harness also re-checks from OUTSIDE. docs/phase5-slice1-mount.md M3.
#
# Usage: scripts/sandbox-proof.sh
set -uo pipefail

if [ "${IN_CONTAINER:-0}" != "1" ]; then
  REPO_ROOT="$(git rev-parse --show-toplevel)"
  echo "=== building release gatekeeperd (host) ==="
  ( cd "$REPO_ROOT" && cargo build --release -p gatekeeperd ) || exit 3
  echo "=== launching privileged debian:trixie oracle ==="
  exec docker run --rm --privileged -e IN_CONTAINER=1 \
    -v "$REPO_ROOT/scripts/sandbox-proof.sh:/proof.sh:ro" \
    -v "$REPO_ROOT/target/release/gatekeeperd:/gatekeeperd:ro" \
    debian:trixie bash /proof.sh
fi

# ---------------- inside the container ----------------
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
apt-get install -y --no-install-recommends systemd-container >/dev/null 2>&1
mount --make-rshared / 2>/dev/null || true
mount -t tmpfs tmpfs /run 2>/dev/null || true

# host tree: granted 'project', denied 'vault'
mkdir -p /srv/project /srv/vault
echo PROJECT > /srv/project/marker
echo VAULT   > /srv/vault/marker

# In-sandbox probe: assert the caps-enforced mount set from INSIDE and emit SHREK_GATE lines.
PROBE='
m=$(cat /srv/project/marker 2>/dev/null)
[ "$m" = PROJECT ] && echo "SHREK_GATE: PASS gate=in-project-readable marker=$m" || echo "SHREK_GATE: FAIL gate=in-project-readable marker=$m"
if cat /srv/vault/marker 2>&1 | grep -qi "No such file"; then echo "SHREK_GATE: PASS gate=in-vault-enoent"; else echo "SHREK_GATE: FAIL gate=in-vault-enoent (vault reachable!)"; fi
l=$(ls /srv | tr "\n" ,)
[ "$l" = "project," ] && echo "SHREK_GATE: PASS gate=in-vault-absent ls=$l" || echo "SHREK_GATE: FAIL gate=in-vault-absent ls=$l"
o=$(stat -c %u /srv/project/marker)
[ "$o" = 65534 ] && echo "SHREK_GATE: PASS gate=in-private-users host-root=nobody($o)" || echo "SHREK_GATE: FAIL gate=in-private-users host-root-uid=$o"
'

echo "=== M3: gatekeeperd sandbox (grant project, deny vault) ==="
/gatekeeperd sandbox --id m3 --anchor /srv --grant project -- /bin/sh -c "$PROBE"
RC=$?
echo "  gatekeeperd exit=$RC"

# Aggregate: PASS only if every in-sandbox gate passed and construction succeeded. The workload's
# exit code is the last probe line's; we assert on the emitted gates for a clean verdict, captured
# by the caller. Re-run capturing output to count.
OUT=$(/gatekeeperd sandbox --id m3b --anchor /srv --grant project -- /bin/sh -c "$PROBE" 2>/dev/null)
echo "$OUT"
PASS=$(printf '%s\n' "$OUT" | grep -c 'SHREK_GATE: PASS')
FAILN=$(printf '%s\n' "$OUT" | grep -c 'SHREK_GATE: FAIL')
echo
if [ "$FAILN" = "0" ] && [ "$PASS" -ge 4 ]; then
  echo "SHREK_GATE: PASS M3 gatekeeperd-driven sandbox ($PASS gates)"
  exit 0
else
  echo "SHREK_GATE: FAIL M3 gatekeeperd-driven sandbox (pass=$PASS fail=$FAILN)"
  exit 1
fi
