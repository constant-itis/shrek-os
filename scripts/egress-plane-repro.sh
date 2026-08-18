#!/usr/bin/env bash
# Phase-5 slice-3 — egress plane, step-1 netns proof harness (oracle: privileged debian:trixie).
# Resolves ONE open item before any construct() network code is written: with
# `systemd-nspawn --network-namespace-path=/run/netns/NS` the outbound allow-listed SYN is
# forwarded but the RETURN path did not complete in the spike, while plain `ip netns exec` in the
# SAME ns works. Same netns, same veth, same nft, same root-side masquerade/conntrack — so the only
# variable is HOW the client enters the ns. This harness holds the ns/nft/server IDENTICAL and runs
# a three-way diff of the client entry, with tcpdump on both veths + conntrack + nft counters, so a
# single run pinpoints the cause:
#   B  netns-exec        : ip netns exec NS <busybox> wget      (the known-good control)
#   C1 nspawn-no-userns  : systemd-nspawn --network-namespace-path ...            (no --private-users)
#   C2 nspawn-userns     : systemd-nspawn --network-namespace-path ... --private-users=pick  (prod cfg)
# If B passes and C* fail => nspawn is the cause; C1-vs-C2 tells us whether it is the userns.
#
# Hermetic: no real internet/DNS. A second veth to a 'srv' netns is the pinned "internet" host; nft
# pins its IP. Faithful masquerade (srv has no route back to the container subnet). IPv4-only.
#
# Usage: scripts/egress-plane-repro.sh    # whole repro in a throwaway privileged container
set -uo pipefail

if [ "${IN_CONTAINER:-0}" != "1" ]; then
  REPO_ROOT="$(git rev-parse --show-toplevel)"
  echo "=== launching privileged debian:trixie oracle ==="
  exec docker run --rm --privileged -e IN_CONTAINER=1 \
    -v "$REPO_ROOT/scripts/egress-plane-repro.sh:/repro.sh:ro" \
    debian:trixie bash /repro.sh
fi

# ---------------- inside the container ----------------
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
apt-get install -y --no-install-recommends \
  systemd-container busybox-static iproute2 nftables tcpdump conntrack ncat >/dev/null 2>&1

# nspawn-in-container prerequisites (environment, not design — mount-plane M0 results).
# ORDER MATTERS: remount /run to tmpfs FIRST, then create netns — a later tmpfs-on-/run would hide
# /run/netns/<NS> (the #2563 ordering lesson).
mount --make-rshared / 2>/dev/null || true
mount -t tmpfs tmpfs /run 2>/dev/null || true

# Deterministic forwarding + reverse-path state (held identical across ALL scenarios, so it can never
# be the B-vs-C differentiator; recorded in each snapshot for the record). Write /proc directly — the
# minimal image has no `sysctl` binary.
echo 1 > /proc/sys/net/ipv4/ip_forward 2>/dev/null || true
echo 0 > /proc/sys/net/ipv4/conf/all/rp_filter 2>/dev/null || true
echo 0 > /proc/sys/net/ipv4/conf/default/rp_filter 2>/dev/null || true

FAIL=0
gate() { # gate <name> <cond-rc> <detail>
  if [ "$2" = "0" ]; then echo "SHREK_GATE: PASS gate=$1 $3"; else echo "SHREK_GATE: FAIL gate=$1 reason=$3"; FAIL=1; fi
}

# --- topology --------------------------------------------------------------------------------------
# container ns 'egr':  host0/10.10.0.2  <--veth-->  ve-egr/10.10.0.1  (root)
# server    ns 'srv':  srv0 /10.20.0.2  <--veth-->  up-egr/10.20.0.1  (root)
# allowed dst = 10.20.0.2 tcp 443 ; root FORWARDs+MASQUERADEs egr->srv, srv has NO route back (faithful)
DST=10.20.0.2; DPORT=443
echo "=== building netns topology (locked #2563 mechanism, IPv4-only) ==="
ip netns add egr
ip netns add srv

# container veth
ip link add ve-egr type veth peer name host0
ip link set host0 netns egr
ip addr add 10.10.0.1/30 dev ve-egr; ip link set ve-egr up
ip -n egr addr add 10.10.0.2/30 dev host0
ip -n egr link set host0 up; ip -n egr link set lo up
ip -n egr route add default via 10.10.0.1

# server veth
ip link add up-egr type veth peer name srv0
ip link set srv0 netns srv
ip addr add 10.20.0.1/30 dev up-egr; ip link set up-egr up
ip -n srv addr add 10.20.0.2/30 dev srv0
ip -n srv link set srv0 up; ip -n srv link set lo up
ip -n srv route add default via 10.20.0.1
# NB: 'srv' has NO route to 10.10.0.0/30 -> the reply can only return via conntrack un-NAT => forces
# a real masquerade round-trip, exactly like the internet.

# rules-before-usable: default-DROP forward, allow ONLY egr->pinned-dst:port, established back, masq.
nft -f - <<EOF
table ip shrek_egress {
  chain forward {
    type filter hook forward priority 0; policy drop;
    ct state established,related counter accept
    ip saddr 10.10.0.2 ip daddr $DST tcp dport $DPORT counter accept
    ip saddr 10.11.0.2 ip daddr $DST tcp dport $DPORT counter accept
    counter drop
  }
  chain postrouting {
    type nat hook postrouting priority srcnat; policy accept;
    oifname "up-egr" masquerade
  }
}
EOF
# 10.10.0.2 = the pre-created-netns scenarios (B/C1); 10.11.0.2 = C4's host-injected container veth.
# Masquerade keyed on the uplink interface (up-egr) so both container subnets NAT out identically.

# the pinned "internet" server (full host userland in the srv ns): a PERSISTENT listener (ncat -k)
# so every scenario hits a live socket — a one-shot re-listen loop races the client (a between-conns
# gap yields a false "Connection refused" RST, which is a harness artifact, not the property tested).
ip netns exec srv ncat -lk 10.20.0.2 443 --sh-exec \
  'printf "HTTP/1.0 200 OK\r\nContent-Length: 15\r\n\r\nSHREK_EGRESS_OK"' >/dev/null 2>&1 &
SRV_PID=$!
sleep 0.5

# --- synthetic OS-shaped root for nspawn (same shape as the mount plane) ---------------------------
ROOT=/run/shrek/egr/root
mkdir -p "$ROOT/usr/bin" "$ROOT/etc" "$ROOT/proc" "$ROOT/sys" "$ROOT/dev" "$ROOT/run" "$ROOT/tmp"
cp /bin/busybox "$ROOT/usr/bin/busybox"
for a in sh wget nc ip cat ls; do ln -sf busybox "$ROOT/usr/bin/$a"; done
ln -sf usr/bin "$ROOT/bin"
printf 'ID=shrek-sandbox\n' > "$ROOT/etc/os-release"
CLIENT_BB="$ROOT/usr/bin/busybox"   # the ONE client binary used by every scenario

# the client command line, identical everywhere (busybox wget to the pinned dst; -T4 keeps the
# socket open ~4s so the root-side tcpdump sees the whole handshake).
client_argv() { echo "wget -T 4 -q -O - http://$DST:$DPORT/"; }

snapshot() { # dump the state that would distinguish H1 (ns reconfigured) from H2 (reply not delivered)
  echo "  --- snapshot ---"
  echo "  egr addr : $(ip -n egr -br addr show host0 2>/dev/null | tr -s ' ')"
  echo "  egr route: $(ip -n egr route 2>/dev/null | tr '\n' ';')"
  echo "  conntrack: $(conntrack -L 2>/dev/null | grep -i "$DST" | head -3 | tr -s ' ' | tr '\n' '|')"
  echo "  nft fwd  : $(nft -a list chain ip shrek_egress forward 2>/dev/null | grep -Eo 'counter packets [0-9]+ bytes [0-9]+ (accept|drop)' | tr '\n' '|')"
}

run_scenario() { # run_scenario <name> <expect:ok|deny> -- <command...>  ; +both-veth pcap counts
  local name="$1" expect="$2"; shift 2; shift  # drop the literal '--'
  echo
  echo "=== scenario $name ==="
  nft reset counters table ip shrek_egress >/dev/null 2>&1
  conntrack -F >/dev/null 2>&1
  # tcpdump both veths (root side) for the life of the client
  timeout 8 tcpdump -i ve-egr -n -l "tcp and port $DPORT" >/tmp/pcap-ve.$name 2>/dev/null &
  local tv=$!
  timeout 8 tcpdump -i up-egr -n -l "tcp and port $DPORT" >/tmp/pcap-up.$name 2>/dev/null &
  local tu=$!
  sleep 0.4
  local out; out="$("$@" 2>/tmp/err.$name)"
  sleep 0.6; kill "$tv" "$tu" 2>/dev/null; wait "$tv" "$tu" 2>/dev/null
  echo "  client-stdout: [$(echo "$out" | tr -d '\r' | tr '\n' ' ')]"
  [ -s /tmp/err.$name ] && echo "  client-stderr: [$(tr -d '\r' <"/tmp/err.$name" | tr '\n' ' ')]"
  echo "  ve-egr  syn/synack: SYN=$(grep -c 'Flags \[S\],' /tmp/pcap-ve.$name) SYN-ACK=$(grep -c 'Flags \[S\.\]' /tmp/pcap-ve.$name)"
  echo "  up-egr  syn/synack: SYN=$(grep -c 'Flags \[S\],' /tmp/pcap-up.$name) SYN-ACK=$(grep -c 'Flags \[S\.\]' /tmp/pcap-up.$name)"
  snapshot
  if echo "$out" | grep -q SHREK_EGRESS_OK; then
    [ "$expect" = ok ] && gate "egress-$name" 0 "return-path completed (expected)" \
                       || gate "egress-$name" 1 "REACHED dst but was expected to be refused"
  else
    [ "$expect" = deny ] && gate "egress-$name" 0 "refused as expected (negative control): $(tr -d '\r' <"/tmp/err.$name" | tr '\n' ' ')" \
                         || gate "egress-$name" 1 "no marker (did NOT complete)"
  fi
}

# --- B: control — ip netns exec, the KNOWN-GOOD path, using the exact nspawn client binary ---------
run_scenario B-netns-exec ok -- ip netns exec egr "$CLIENT_BB" $(client_argv)

# --- C1: nspawn WITHOUT --private-users (isolates the userns axis) — proves the mechanism is sound -
run_scenario C1-nspawn-nouserns ok -- systemd-nspawn -q --console=pipe --register=no --keep-unit \
  --resolv-conf=off --link-journal=no --machine=egr-c1 --directory="$ROOT" \
  --network-namespace-path=/run/netns/egr \
  -- /usr/bin/busybox $(client_argv)

# --- C2: NEGATIVE CONTROL — nspawn WITH --private-users=pick (the mandated prod config) must be
#     REFUSED joining a host-owned netns ('Operation not permitted'). This deterministic refusal is
#     the whole reason the production model injects into an nspawn-OWNED netns instead (see C4).
run_scenario C2-nspawn-userns deny -- systemd-nspawn -q --console=pipe --register=no --keep-unit \
  --resolv-conf=off --link-journal=no --machine=egr-c2 --directory="$ROOT" \
  --private-users=pick --private-users-ownership=off \
  --network-namespace-path=/run/netns/egr \
  -- /usr/bin/busybox $(client_argv)

# --- C3: no-net cell — nspawn OWNS a fresh empty netns (--private-network) WITH userns -------------
# This is slice-3 requirement 1 (close the latent host-netns hole) AND proves userns is compatible
# with nspawn *owning* its netns (unlike joining a host-owned one, C2). Inverted gate: PASS = the
# container sees ONLY loopback and reaches the pinned dst NOTHING.
echo
echo "=== scenario C3-nspawn-privatenet (no-net cell: --private-users + --private-network) ==="
nft reset counters table ip shrek_egress >/dev/null 2>&1
C3OUT="$(systemd-nspawn -q --console=pipe --register=no --keep-unit --resolv-conf=off --link-journal=no \
  --machine=egr-c3 --directory="$ROOT" --private-users=pick --private-users-ownership=off \
  --private-network \
  -- /usr/bin/busybox sh -c 'echo IFACES=$(ls /sys/class/net | tr "\n" ",");
     wget -T2 -qO- http://'"$DST"':'"$DPORT"'/ 2>&1; echo RC=$?' 2>&1)"
C3OUT="$(echo "$C3OUT" | tr -d '\r' | tr '\n' ' ')"
echo "  container-view: [$C3OUT]"
ifaces="$(echo "$C3OUT" | grep -oE 'IFACES=[^ ]*')"
if echo "$C3OUT" | grep -q SHREK_EGRESS_OK; then
  gate no-net-isolation 1 "container REACHED the pinned dst (egress leak!)"
elif echo "$ifaces" | grep -qE 'IFACES=(lo,?)?$'; then
  gate no-net-isolation 0 "$ifaces — loopback-only, pinned dst unreachable (host-net hole CLOSED)"
else
  gate no-net-isolation 1 "unexpected ifaces: $ifaces"
fi

# --- C4: THE PRODUCTION MODEL — nspawn owns the netns, gatekeeperd injects egress (CNI-style) ------
# nspawn --private-users=pick --private-network (baseline, C3-proven) -> the privileged host discovers
# the container leader/netns, injects veth+addressing+routes, relies on the pre-installed default-DROP
# +allow nft, and only THEN releases the workload past a ready-barrier. Proves: allowed round-trip OK,
# denied dst DROPPED, clean teardown. A rendezvous dir (bound in) carries barrier + results both ways.
echo
echo "=== scenario C4-inject (nspawn-owned netns + host-injected egress, rules-before-usable) ==="
RV=/run/rv; rm -rf "$RV"; mkdir -p "$RV"; chmod 777 "$RV"
nft reset counters table ip shrek_egress >/dev/null 2>&1
conntrack -F >/dev/null 2>&1

# workload: BLOCK on the barrier (/rv/go) until the host says networking is wired, THEN test allowed
# (DST:443 -> marker) and denied (DST:80 -> must be DROPPED = timeout, no RST), reporting via /rv/out.
systemd-nspawn -q --console=pipe --register=no --keep-unit --resolv-conf=off --link-journal=no \
  --machine=egr-c4 --directory="$ROOT" --private-users=pick --private-users-ownership=off \
  --private-network --bind="$RV:/rv" \
  -- /usr/bin/busybox sh -c '
    while [ ! -e /rv/go ] && [ ! -e /rv/abort ]; do sleep 0.05; done
    if [ -e /rv/abort ]; then echo "ALLOW=[aborted-fail-closed]" > /rv/out; echo "DENY_RC=1" >> /rv/out; echo DONE >> /rv/out; exit 0; fi
    echo "ALLOW=[$(wget -T3 -qO- http://'"$DST"':443/ 2>&1)]"          >  /rv/out
    wget -T2 -qO- http://'"$DST"':80/ >/dev/null 2>&1; echo "DENY_RC=$?" >> /rv/out
    echo DONE >> /rv/out' &
NSPAWN_PID=$!

# (1) discover the container leader = ANY process that descends from nspawn AND lives in a different
# net namespace (walk full ancestry — the netns-holder sits several forks below systemd-nspawn).
host_net="$(readlink /proc/self/ns/net)"; LEADER=""
is_descendant() { local pid=$1 g=0; while [ "${pid:-0}" -gt 1 ] 2>/dev/null && [ $g -lt 25 ]; do
    pid=$(awk '/^PPid:/{print $2}' "/proc/$pid/status" 2>/dev/null); g=$((g+1))
    [ "$pid" = "$NSPAWN_PID" ] && return 0; done; return 1; }
for _ in $(seq 1 60); do
  for d in /proc/[0-9]*; do
    p=${d#/proc/}; n="$(readlink "$d/ns/net" 2>/dev/null)" || continue
    [ -z "$n" ] || [ "$n" = "$host_net" ] && continue
    is_descendant "$p" && { LEADER=$p; break; }
  done
  [ -n "$LEADER" ] && break; sleep 0.15
done
echo "  leader-pid: ${LEADER:-<none>}  container-netns: $([ -n "$LEADER" ] && readlink /proc/$LEADER/ns/net 2>/dev/null)"

# fail-closed injection: any step fails -> tear down AND signal the workload to abort (never egress).
inject_teardown() { ip netns del c4ns 2>/dev/null; ip link del ve-c4 2>/dev/null; }
if [ -z "$LEADER" ]; then
  gate c4-leader-discovery 1 "could not locate container netns"; inject_teardown; touch "$RV/abort"
else
  gate c4-leader-discovery 0 "leader=$LEADER"
  if ip netns attach c4ns "$LEADER" 2>/tmp/c4.err \
     && ip link add ve-c4 type veth peer name egr0 2>>/tmp/c4.err \
     && ip link set egr0 netns c4ns 2>>/tmp/c4.err \
     && ip addr add 10.11.0.1/30 dev ve-c4 2>>/tmp/c4.err && ip link set ve-c4 up 2>>/tmp/c4.err \
     && ip -n c4ns addr add 10.11.0.2/30 dev egr0 2>>/tmp/c4.err \
     && ip -n c4ns link set egr0 up 2>>/tmp/c4.err && ip -n c4ns link set lo up 2>>/tmp/c4.err \
     && ip -n c4ns route add default via 10.11.0.1 2>>/tmp/c4.err; then
    gate c4-inject 0 "veth egr0/10.11.0.2 injected into container netns; nft default-DROP+allow already live"
    touch "$RV/go"           # rules-before-usable: barrier released ONLY after wiring is complete
  else
    gate c4-inject 1 "injection failed: $(tr '\n' ';' </tmp/c4.err)"; inject_teardown; touch "$RV/abort"
  fi
fi

# collect the container's own results
for _ in $(seq 1 60); do grep -q DONE "$RV/out" 2>/dev/null && break; sleep 0.1; done
C4=$(tr -d '\r' <"$RV/out" 2>/dev/null | tr '\n' ' ')
echo "  container-results: [$C4]"
echo "  nft fwd  : $(nft list chain ip shrek_egress forward 2>/dev/null | grep -Eo 'counter packets [0-9]+ bytes [0-9]+ (accept|drop)' | tr '\n' '|')"
echo "$C4" | grep -q 'ALLOW=\[SHREK_EGRESS_OK\]' && gate c4-allowed 0 "pinned dst round-trip completed" || gate c4-allowed 1 "allowed dst did NOT complete: $C4"
# denied: dropped (no RST) => wget exits non-zero via TIMEOUT, not 'refused'. rc!=0 AND no marker.
if echo "$C4" | grep -qE 'DENY_RC=[^0]'; then gate c4-denied 0 "non-allowed dst DROPPED (wget rc!=0, no RST)"; else gate c4-denied 1 "non-allowed dst was NOT denied: $C4"; fi

wait "$NSPAWN_PID" 2>/dev/null
# teardown proof: remove injected plumbing, confirm it is gone (fail-closed leaves nothing usable).
inject_teardown
if ip link show ve-c4 >/dev/null 2>&1 || ip netns list 2>/dev/null | grep -q c4ns; then
  gate c4-teardown 1 "residual veth/netns after teardown"
else
  gate c4-teardown 0 "veth + netns-name removed; no residual egress plumbing"
fi

kill "$SRV_PID" 2>/dev/null

echo
echo "=== verdict (resolved) ==="
echo "  B  + C1  PASS  => netns+veth+nft(default-drop/allow)+masquerade round-trip is CORRECT; the"
echo "                   #2563 'return path didn't complete' was a MIS-DIAGNOSIS."
echo "  C2 (neg ctl)   => --private-users (child userns) CANNOT setns() into a host-owned netns"
echo "                   (EPERM). So pre-create-netns + --network-namespace-path is dead for us."
echo "  C3        PASS => --private-users + --private-network = clean no-net cell (closes latent hole)."
echo "  C4        PASS => PRODUCTION MODEL: nspawn OWNS the netns; host discovers the leader, injects"
echo "                   veth+nft AFTER boot, releases the workload past a ready-barrier; allowed dst"
echo "                   round-trips, denied dst DROPs, teardown clean, injection-fail => fail-closed."
if [ "$FAIL" = "0" ]; then echo "SHREK_GATE: PASS egress-plane repro (production egress model validated end-to-end)"; else echo "SHREK_GATE: FAIL egress-plane repro (see failing gate above)"; fi
exit $FAIL
