#!/usr/bin/env bash
# Phase-5 slice-3 — egress construction proof. Drives the REAL release `gatekeeperd sandbox`
# subcommand (net_plane + sandbox.rs) inside a privileged debian:trixie oracle, the fast gate before
# the VM cycle. Proves the wired construct() behaviour end-to-end:
#   * no-net cell            -> --private-network, loopback-only, reaches nothing (host-net hole shut)
#   * C-net + sealed profile -> gatekeeperd resolves the profile, injects veth+nft, workload REACHES
#                               the allowed dst; a NON-allowed dst is DROPPED (default-deny)
#   * unknown profile        -> REFUSED (exit 13), no construction
#   * C-broad                -> REFUSED (exit 13), no egress plane
#   * teardown               -> no residual veth / nft table after a run
#
# Hermetic: the sealed `github-https` profile's hosts are mapped (in the oracle's /etc/hosts, which
# gatekeeperd's resolver reads) to a local "internet" server netns; nft pins that IP. No real net.
#
# Usage: scripts/egress-construct-proof.sh
set -uo pipefail

if [ "${IN_CONTAINER:-0}" != "1" ]; then
  REPO_ROOT="$(git rev-parse --show-toplevel)"
  echo "=== building release gatekeeperd (host) ==="
  ( cd "$REPO_ROOT" && cargo build --release -p gatekeeperd ) || exit 3
  echo "=== launching privileged debian:trixie oracle ==="
  exec docker run --rm --privileged -e IN_CONTAINER=1 \
    -v "$REPO_ROOT/scripts/egress-construct-proof.sh:/proof.sh:ro" \
    -v "$REPO_ROOT/target/release/gatekeeperd:/gatekeeperd:ro" \
    debian:trixie bash /proof.sh
fi

# ---------------- inside the container ----------------
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
apt-get install -y --no-install-recommends \
  systemd-container busybox-static iproute2 nftables ncat >/dev/null 2>&1

mount --make-rshared / 2>/dev/null || true
mount -t tmpfs tmpfs /run 2>/dev/null || true
echo 1 > /proc/sys/net/ipv4/ip_forward 2>/dev/null || true

FAIL=0
gate() { if [ "$2" = "0" ]; then echo "SHREK_GATE: PASS gate=$1 $3"; else echo "SHREK_GATE: FAIL gate=$1 reason=$3"; FAIL=1; fi; }

# --- the local "internet": a server netns holding the pinned dst, reached via a root-side uplink ---
ip netns add srv
ip link add up-egr type veth peer name srv0
ip link set srv0 netns srv
ip addr add 10.20.0.1/24 dev up-egr; ip link set up-egr up
ip -n srv addr add 10.20.0.2/24 dev srv0; ip -n srv link set srv0 up; ip -n srv link set lo up
ip -n srv route add default via 10.20.0.1
ip netns exec srv ncat -lk 10.20.0.2 443 --sh-exec \
  'printf "HTTP/1.0 200 OK\r\nContent-Length: 15\r\n\r\nSHREK_EGRESS_OK"' >/dev/null 2>&1 &
SRV_PID=$!
# Map the sealed github-https profile's hosts to the local server so gatekeeperd's resolver pins them.
printf '10.20.0.2 github.com codeload.github.com objects.githubusercontent.com\n' >> /etc/hosts
sleep 0.4

# host tree: a single granted dir (mount plane is orthogonal here; we just need a valid grant).
mkdir -p /srv/project; echo PROJECT > /srv/project/marker

BB=/bin/busybox
COMMON=(--anchor /srv --grant project)

echo
echo "=== T1: C-net + sealed profile github-https — allowed dst REACHED ==="
OUT=$(/gatekeeperd sandbox --tier T1 --trust T-first --caps C-net --egress-profile github-https \
  --id cnet-ok "${COMMON[@]}" -- "$BB" wget -T6 -qO- http://github.com:443/ 2>/tmp/e1)
RC=$?
echo "  gatekeeperd rc=$RC stdout=[$(echo "$OUT" | tr -d '\r' | tr '\n' ' ')]"
grep -q 'cleared construct-at=T1' /tmp/e1 && echo "  decision: $(grep -o 'SANDBOX-DECISION cleared.*' /tmp/e1 | head -1)"
echo "$OUT" | grep -q SHREK_EGRESS_OK && gate cnet-allowed 0 "workload reached pinned dst via injected egress" || gate cnet-allowed 1 "no marker (stderr: $(tail -1 /tmp/e1 | tr -d '\r'))"

echo
echo "=== T1: C-net + same profile — NON-allowed dst DROPPED (default-deny) ==="
# 10.20.0.3 is routable toward the server subnet but is NOT in the profile -> forward drop -> timeout.
OUT=$(/gatekeeperd sandbox --tier T1 --trust T-first --caps C-net --egress-profile github-https \
  --id cnet-deny "${COMMON[@]}" -- "$BB" sh -c 'wget -T3 -qO- http://10.20.0.3:443/ 2>&1; echo RC=$?' 2>/dev/null)
echo "  workload=[$(echo "$OUT" | tr -d '\r' | tr '\n' ' ')]"
if echo "$OUT" | grep -q SHREK_EGRESS_OK; then gate cnet-denied 1 "reached a non-allowed dst (leak!)"
elif echo "$OUT" | grep -qE 'RC=[^0]'; then gate cnet-denied 0 "non-allowed dst dropped (wget rc!=0, no RST)"
else gate cnet-denied 1 "unexpected: $OUT"; fi

echo
echo "=== T1: no-net cell (C-ro-nosec) — loopback-only, reaches nothing ==="
OUT=$(/gatekeeperd sandbox --tier T1 --trust T-first --caps C-ro-nosec \
  --id nonet "${COMMON[@]}" -- "$BB" sh -c 'echo IFACES=$(ls /sys/class/net | tr "\n" ,); wget -T3 -qO- http://github.com:443/ 2>&1; echo RC=$?' 2>/dev/null)
echo "  workload=[$(echo "$OUT" | tr -d '\r' | tr '\n' ' ')]"
if echo "$OUT" | grep -q SHREK_EGRESS_OK; then gate no-net-isolation 1 "no-net cell REACHED dst (leak!)"
elif echo "$OUT" | grep -qE 'IFACES=(lo,?)?$'; then gate no-net-isolation 0 "loopback-only, dst unreachable"
else gate no-net-isolation 1 "unexpected ifaces: $(echo "$OUT" | grep -o 'IFACES=[^ ]*')"; fi

echo
echo "=== T1: unknown egress profile — REFUSED, no construction ==="
/gatekeeperd sandbox --tier T1 --trust T-first --caps C-net --egress-profile bogus-exfil \
  --id unk "${COMMON[@]}" -- "$BB" true 2>/tmp/e4; RC=$?
grep -q 'reason=unknown-egress-profile=bogus-exfil' /tmp/e4 && [ "$RC" = 13 ] \
  && gate unknown-profile-refused 0 "exit=$RC reason=unknown-egress-profile" \
  || gate unknown-profile-refused 1 "exit=$RC (expected 13 + unknown-egress-profile); $(grep -o 'SANDBOX-DECISION.*' /tmp/e4 | head -1)"

echo
echo "=== T1: C-broad — REFUSED (no egress plane) ==="
/gatekeeperd sandbox --tier T1 --trust T-first --caps C-broad \
  --id broad "${COMMON[@]}" -- "$BB" true 2>/tmp/e5; RC=$?
[ "$RC" = 13 ] && grep -q 'no-plane-for-C-broad' /tmp/e5 \
  && gate cbroad-refused 0 "exit=$RC reason=no-plane-for-C-broad" \
  || gate cbroad-refused 1 "exit=$RC; $(grep -o 'SANDBOX-DECISION.*' /tmp/e5 | head -1)"

echo
echo "=== teardown: no residual veth / nft table after the runs ==="
resid_if=$(ip -o link show 2>/dev/null | grep -oE 'skh[0-9a-f]{4}' | tr '\n' ',')
resid_tb=$(nft list tables 2>/dev/null | grep -o 'shrek_egress_[a-z0-9_]*' | tr '\n' ',')
if [ -z "$resid_if" ] && [ -z "$resid_tb" ]; then gate teardown 0 "no residual egress plumbing"; else gate teardown 1 "residual if=[$resid_if] tables=[$resid_tb]"; fi

kill "$SRV_PID" 2>/dev/null
echo
if [ "$FAIL" = "0" ]; then echo "SHREK_GATE: PASS egress-construct proof (construct() egress plane validated end-to-end)"; else echo "SHREK_GATE: FAIL egress-construct proof"; fi
exit $FAIL
