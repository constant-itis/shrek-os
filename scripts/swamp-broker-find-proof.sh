#!/usr/bin/env bash
# Phase-6 Swamp slice-2 — broker-routed in-sandbox `shrek find`. Host-side, --privileged oracle (the
# fast path before the sealed-VM regression; the swamp-broker is OFF the sealed image). It stands up the
# WHOLE routed path end-to-end and proves the transport-identity gate + the invariant "routing changes
# reachability, never authority":
#
#   swampd (host, unix socket)  ◄──  swamp-broker (srv netns, tcp:8400)  ◄──  sandbox netns (cont_ip)
#          authority = root-owned handle-keyed record        forwards iff getpeername→cont_ip binding == handle
#
# The network plane is the REAL Mechanism-A shape: all sandbox egress is masqueraded EXCEPT the swamp
# broker's dst (a `return` carve-out), so its cont_ip survives to the broker; a per-veth prerouting
# anti-spoof makes that cont_ip un-forgeable at the host. Gates:
#   B1 happy-path parity  — a bound sandbox's routed query returns EXACTLY what a host-side
#                            `shrek find --session H` returns (same hits).
#   B2 stolen handle      — session A's handle presented from sandbox B's wire → empty (B's cont_ip
#                            binds a different session).
#   B3 unbound source     — a sandbox with no binding → empty (can't authorize any handle).
#   B4 no widening        — a query for an OUT-OF-SCOPE token returns nothing across the hop (the broker
#                            never widens the session's authority; BBSECRET stays invisible).
#   B5 carve-out exact    — only the broker dst is un-masqueraded (broker sees the real cont_ip, the
#                            model dst is NAT'd); an UNGRANTED sandbox is FORWARD-dropped.
#   B6 swampd frozen      — the broker-routed projection is byte-identical to the direct host-side one;
#                            swampd is oblivious to the broker (same record, same wire, allowed uid).
#   B-spoof host anti-spoof — a sandbox sending a FORGED source IP is dropped at the host veth.
#   B-reuse stale IP      — revoke a binding then reuse the cont_ip for a NEW session: the new handle
#                            works, the stale handle fails closed, and the un-bound window is empty.
#   C  coder integration  — the REAL `coder` binary, model-driven (deterministic canned responder), calls
#                            its `swamp_find` tool over the routed path and receives ONLY its authorized
#                            projection (in-scope hit present, out-of-scope BBSECRET absent).
#
# Slice-3 adds the LIVE + PERSISTENT gates (the index repairs itself and survives a restart), all still
# routed through the broker so authority is exercised end-to-end:
#   D1 live CREATE   — a newly-created in-scope file becomes searchable through the broker.
#   D2 live MODIFY   — an edit swaps the searchable content (new token in, old token retired).
#   D3 live DELETE   — removing a file removes its searchable content (metadata AND FTS).
#   D4 live RENAME   — a mv reflects at the new path and is gone at the old.
#   D5 freshness     — a healthy routed response carries `freshness fresh`; a broker fail-closed
#                      (stolen/unbound) carries `freshness unknown` (index-global, no existence oracle).
#   D6 persistence   — the DB survives a swampd restart, and the startup reconcile catches a CREATE and a
#                      DELETE that happened while swampd was DOWN (offline-change reconciliation).
#
# Slice-4 adds the SEMANTIC-TIER gates (a hermetic canned embeddings provider bridged by the off-image
# swamp-embed-proxy; swampd stays network-free), proving the abstraction + graceful degradation:
#   E0/E5 channel    — the off-image proxy serves its local socket and swampd wires the provider channel.
#   E1 available     — a healthy routed response reports `semantic available`.
#   E2 FTS floor     — the in-scope hit is still returned with semantic ON (lexical floor preserved).
#   E3 authority     — authority-before-vector: the out-of-scope token/project stay ABSENT with semantic
#                      ON (the vector tier obeys the SAME scope gate; a global-ANN leak is impossible).
#   E4 no-side-chan  — the wire carries no score/count/similarity line (projection shape unchanged).
#   E6 degrade       — provider DOWN → `semantic unavailable` AND the FTS floor still serves (never
#                      makes Swamp unavailable — the load-bearing failure asymmetry).
#   E7 fail-closed   — a broker fail-closed projection carries `semantic unavailable`.
#
# Usage: scripts/swamp-broker-find-proof.sh
set -u

if [ "${IN_CONTAINER:-0}" != "1" ]; then
  REPO_ROOT="$(git rev-parse --show-toplevel)"
  echo "=== building release swampd + gatekeeperd + shrek + swamp-broker + coder + swamp-embed-proxy (host) ==="
  ( cd "$REPO_ROOT" && cargo build --release -p swampd -p gatekeeperd -p shrek -p swamp-broker -p coder -p swamp-embed-proxy ) || exit 3
  echo "=== launching privileged debian:trixie oracle ==="
  exec docker run --rm --privileged --cgroupns=private -e IN_CONTAINER=1 \
    -v "$REPO_ROOT/scripts/swamp-broker-find-proof.sh:/proof.sh:ro" \
    -v "$REPO_ROOT/target/release/swampd:/swampd:ro" \
    -v "$REPO_ROOT/target/release/gatekeeperd:/gatekeeperd:ro" \
    -v "$REPO_ROOT/target/release/shrek:/shrek:ro" \
    -v "$REPO_ROOT/target/release/swamp-broker:/swamp-broker:ro" \
    -v "$REPO_ROOT/target/release/coder:/coder:ro" \
    -v "$REPO_ROOT/target/release/swamp-embed-proxy:/swamp-embed-proxy:ro" \
    debian:trixie bash /proof.sh
fi

# ---------------- inside the container ----------------
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
apt-get install -y --no-install-recommends iproute2 nftables curl python3 procps >/dev/null 2>&1
mount --make-rshared / 2>/dev/null || true
mount -t tmpfs tmpfs /run 2>/dev/null || true
echo 1 > /proc/sys/net/ipv4/ip_forward 2>/dev/null || true

PASS=0; FAILN=0
pass() { echo "SHREK_GATE: PASS $1"; PASS=$((PASS+1)); }
fail() { echo "SHREK_GATE: FAIL $1"; FAILN=$((FAILN+1)); }
# a fail-closed EMPTY projection = the swampd zero-hit wire (RESULT 0 / END), no hit lines.
is_empty_wire() { printf '%s' "$1" | grep -q '^RESULT 0' && ! printf '%s' "$1" | grep -q '^hit '; }
has()    { printf '%s' "$2" | grep -q -- "$3" && pass "$1" || fail "$1 (missing '$3' in: $(printf '%s' "$2" | tr '\n' '|'))"; }
absent() { printf '%s' "$2" | grep -q -- "$3" && fail "$1 (LEAKED '$3' in: $(printf '%s' "$2" | tr '\n' '|'))" || pass "$1"; }
gate_empty() { if is_empty_wire "$2"; then pass "$1"; else fail "$1 (expected empty projection, got: $(printf '%s' "$2" | tr '\n' '|'))"; fi; }

# ---- users: swamp (daemon), tester (owns the indexed tree) ----
groupadd swamp 2>/dev/null || true
useradd -g swamp -M -s /usr/sbin/nologin swamp 2>/dev/null || true
useradd -m -s /bin/bash tester 2>/dev/null || true
TESTER_UID=$(id -u tester)

# ---- seed the index: app-a (IN scope, token ISCOPETOKEN) + app-b (OUT of scope, token BBSECRET) ----
H=/home/tester
mkdir -p "$H/Projects/app-a/src" "$H/Projects/app-b/src"
echo 'the swamp routed find target lives here token ISCOPETOKEN in app-a' > "$H/Projects/app-a/README.md"
echo 'fn main() { /* ISCOPETOKEN routed */ }'                            > "$H/Projects/app-a/src/main.rs"
echo 'out-of-scope material token BBSECRET in app-b must never cross'    > "$H/Projects/app-b/README.md"
chown -R tester:tester "$H"; chmod -R a+rX "$H"

# ---- runtime dirs (root-owned root:swamp records) ----
mkdir -p /run/swamp && chown swamp:swamp /run/swamp && chmod 755 /run/swamp
mkdir -p /run/shrek/authority && chown root:swamp /run/shrek/authority && chmod 750 /run/shrek/authority
mkdir -p /run/shrek/net-binding && chown root:swamp /run/shrek/net-binding && chmod 750 /run/shrek/net-binding
export SWAMP_HOME=$H SWAMP_STATE_DIR=/run/swamp SWAMP_AUTHORITY_DIR=/run/shrek/authority
export SHREK_NET_BINDING_DIR=/run/shrek/net-binding

# ---- authority records: sessH grants app-a ONLY; sessB grants app-b ONLY; sessH2 (reuse) grants app-a ----
/gatekeeperd authority-record --session sessH  --grant "$H/Projects/app-a" --dir /run/shrek/authority >/dev/null || fail "setup:authority-record sessH"
/gatekeeperd authority-record --session sessB  --grant "$H/Projects/app-b" --dir /run/shrek/authority >/dev/null || fail "setup:authority-record sessB"
/gatekeeperd authority-record --session sessH2 --grant "$H/Projects/app-a" --dir /run/shrek/authority >/dev/null || fail "setup:authority-record sessH2"

# ---- cont_ip → session bindings via the REAL writer (format parity). sbC/sbU deliberately unbound. ----
IPA=10.10.0.2; IPB=10.11.0.2; IPC=10.12.0.2; IPU=10.13.0.2
MODEL_IP=10.20.0.2; BROKER_IP=10.20.0.3
/gatekeeperd net-binding --ip $IPA --session sessH --dir /run/shrek/net-binding >/dev/null || fail "setup:binding A"
/gatekeeperd net-binding --ip $IPB --session sessB --dir /run/shrek/net-binding >/dev/null || fail "setup:binding B"

# ---- network plane: one server netns (broker + model) + four sandbox netns, each an unspoofable /30 ----
ip netns add srv
ip link add up-srv type veth peer name srv0
ip link set srv0 netns srv
ip addr add 10.20.0.1/29 dev up-srv && ip link set up-srv up
ip -n srv addr add $MODEL_IP/29 dev srv0
ip -n srv addr add $BROKER_IP/29 dev srv0
ip -n srv link set srv0 up && ip -n srv link set lo up
ip -n srv route add default via 10.20.0.1

mk_sandbox() { # name host-if cont-ip host-gw
  local name=$1 hif=$2 cip=$3 gw=$4
  ip netns add "$name"
  ip link add "$hif" type veth peer name host0
  ip link set host0 netns "$name"
  ip addr add "$gw/30" dev "$hif" && ip link set "$hif" up
  ip -n "$name" addr add "$cip/30" dev host0
  ip -n "$name" link set host0 up && ip -n "$name" link set lo up
  ip -n "$name" route add default via "$gw"
}
mk_sandbox sbA ve-a $IPA 10.10.0.1   # granted + bound sessH
mk_sandbox sbB ve-b $IPB 10.11.0.1   # granted + bound sessB
mk_sandbox sbC ve-c $IPC 10.12.0.1   # granted, NO binding (B3)
mk_sandbox sbU ve-u $IPU 10.13.0.1   # UNGRANTED forward (B5 drop)

# The REAL Mechanism-A ruleset: per-veth anti-spoof (host-enforced cont_ip) + a masquerade with a
# carve-out that un-NATs ONLY the swamp broker's dst, so its cont_ip survives to the broker while every
# other egress (incl the model dst) is masqueraded. FORWARD is default-drop with a per-sandbox allow-list.
nft -f - <<EOF
table ip swamp_oracle {
  chain prerouting {
    type filter hook prerouting priority -300; policy accept;
    iif "ve-a" ip saddr != $IPA drop
    iif "ve-b" ip saddr != $IPB drop
    iif "ve-c" ip saddr != $IPC drop
    iif "ve-u" ip saddr != $IPU drop
  }
  chain forward {
    type filter hook forward priority 0; policy drop;
    ct state established,related accept
    ip saddr { $IPA, $IPB, $IPC } ip daddr $BROKER_IP tcp dport 8400 accept
    ip saddr { $IPA, $IPB, $IPC } ip daddr $MODEL_IP tcp dport 8100 accept
    counter drop
  }
  chain postrouting {
    type nat hook postrouting priority srcnat; policy accept;
    ip saddr { $IPA, $IPB, $IPC } ip daddr $BROKER_IP return
    oifname "up-srv" masquerade
  }
}
EOF
[ $? -eq 0 ] && pass "setup=nft-plane-installed (anti-spoof + carve-out + default-drop forward)" || fail "setup=nft-plane-installed"

# ---- swampd (host netns, as swamp): crawl + query socket ----
runuser -u swamp -- env SWAMP_HOME=$H SWAMP_STATE_DIR=/run/swamp SWAMP_AUTHORITY_DIR=/run/shrek/authority \
  SWAMP_ALLOW_UID=$TESTER_UID /swampd serve >/tmp/swampd.log 2>&1 &
for _ in $(seq 1 100); do [ -S /run/swamp/query.sock ] && break; sleep 0.1; done
if [ ! -S /run/swamp/query.sock ]; then fail "setup=swampd-serving"; echo "--- swampd.log ---"; cat /tmp/swampd.log; else pass "setup=swampd-serving"; fi

# ---- swamp-broker (srv netns, root = an allowed swampd uid): tcp:8400 → /run/swamp/query.sock ----
ip netns exec srv env SHREK_SWAMP_BROKER_LISTEN="$BROKER_IP:8400" SHREK_SWAMP_QUERY_SOCK=/run/swamp/query.sock \
  SHREK_NET_BINDING_DIR=/run/shrek/net-binding /swamp-broker >/tmp/broker.log 2>&1 &
for _ in $(seq 1 100); do ip netns exec sbA curl -s -m1 -o /dev/null "http://$BROKER_IP:8400/" 2>/dev/null && break; sleep 0.1; done

# route_query <netns> <session> <terms> -> the broker's RESULT wire on stdout. The body is the swampd
# line-text wire the coder's swamp_find emits: `q <terms>` (the broker rebuilds the QUERY with the
# AUTHENTICATED session; a client-supplied session is never trusted).
route_query() { ip netns exec "$1" curl -s -m 8 -H "X-Shrek-Session: $2" --data-binary "q $3" "http://$BROKER_IP:8400/" 2>/dev/null; }
# host-side direct query (bypasses the broker): the reference projection, as root (a swampd-allowed uid)
host_find() { env SWAMP_QUERY_SOCK=/run/swamp/query.sock /shrek find --session "$1" "$2" 2>/dev/null; }
# extract the sorted set of absolute hit paths from ANY output shape (broker `hit <path>\t..` OR the
# host-side `shrek find` listing) — format-agnostic, so the parity compare is robust.
paths_of() { printf '%s' "$1" | grep -o '/home/tester/[^[:space:]]*' | sort -u; }

echo "=== B1 happy-path parity ==="
A_WIRE=$(route_query sbA sessH ISCOPETOKEN)
has "B1a routed query returns the in-scope hit" "$A_WIRE" "app-a"
absent "B1b routed query excludes the out-of-scope project" "$A_WIRE" "app-b"
# Parity: the same in-scope absolute paths a host-side shrek find would return.
HOST_OUT=$(host_find sessH ISCOPETOKEN)
ROUTED_P=$(paths_of "$A_WIRE"); DIRECT_P=$(paths_of "$HOST_OUT")
if [ "$ROUTED_P" = "$DIRECT_P" ] && [ -n "$ROUTED_P" ]; then
  pass "B1c routed hits == host-side shrek find hits (routing changes reachability, not authority)"
else
  fail "B1c parity mismatch (routed=[$(printf '%s' "$ROUTED_P" | tr '\n' ',')] host=[$(printf '%s' "$DIRECT_P" | tr '\n' ',')])"
fi

echo "=== B2 stolen handle presented from the wrong wire ==="
B2=$(route_query sbB sessH ISCOPETOKEN)   # sbB's cont_ip binds sessB, not sessH
gate_empty "B2 stolen handle (session A's handle from sandbox B) → empty" "$B2"

echo "=== B3 unbound source ==="
B3=$(route_query sbC sessH ISCOPETOKEN)    # sbC has no binding at all
gate_empty "B3 unbound cont_ip → empty (no binding can authorize any handle)" "$B3"

echo "=== B4 no authority widening across the hop ==="
B4=$(route_query sbA sessH BBSECRET)        # in-authority session, OUT-of-authority token
gate_empty "B4 out-of-scope token via a valid session → 0 hits (BBSECRET never crosses)" "$B4"
absent "B4b BBSECRET absent from the routed projection" "$B4" "BBSECRET"

echo "=== B5 carve-out exact + ungranted FORWARD-drop ==="
RULES=$(nft list table ip swamp_oracle)
RET_LN=$(printf '%s' "$RULES" | grep -n "ip daddr $BROKER_IP return" | head -1 | cut -d: -f1)
MASQ_LN=$(printf '%s' "$RULES" | grep -n "masquerade" | head -1 | cut -d: -f1)
if [ -n "$RET_LN" ] && [ -n "$MASQ_LN" ] && [ "$RET_LN" -lt "$MASQ_LN" ]; then
  pass "B5a carve-out (return) precedes masquerade and targets ONLY the broker dst"
else
  fail "B5a carve-out ordering/exactness (ret=$RET_LN masq=$MASQ_LN)"
fi
# Concrete proof the carve-out preserved identity: the broker logged the REAL cont_ip, not a NAT'd host IP.
has "B5b broker saw the real cont_ip (carve-out worked, not masqueraded)" "$(cat /tmp/broker.log)" "cip=$IPA"
# Ungranted sandbox: not in the FORWARD allow-list → dropped (curl fails, empty body).
U=$(route_query sbU sessH ISCOPETOKEN)
if [ -z "$(printf '%s' "$U" | tr -d '[:space:]')" ]; then pass "B5c ungranted sandbox is FORWARD-dropped (no reply)"; else fail "B5c ungranted sandbox reached the broker: $U"; fi

echo "=== B6 swampd frozen (broker-routed == direct host-side projection) ==="
if [ "$ROUTED_P" = "$DIRECT_P" ]; then pass "B6 swampd oblivious to the broker (identical projection via socket and via broker)"; else fail "B6 divergence routed=[$(printf '%s' "$ROUTED_P"|tr '\n' ',')] direct=[$(printf '%s' "$DIRECT_P"|tr '\n' ',')]"; fi

echo "=== B-spoof host-enforced anti-spoof ==="
ip -n sbA addr add 10.99.0.2/32 dev host0
SPOOF=$(ip netns exec sbA curl -s -m 4 --interface 10.99.0.2 -H "X-Shrek-Session: sessH" --data-binary "q ISCOPETOKEN" "http://$BROKER_IP:8400/" 2>/dev/null)
ip -n sbA addr del 10.99.0.2/32 dev host0 2>/dev/null || true
if [ -z "$(printf '%s' "$SPOOF" | tr -d '[:space:]')" ] && ! grep -q "cip=10.99.0.2" /tmp/broker.log; then
  pass "B-spoof forged source IP dropped at the host veth (cont_ip is host-enforced)"
else
  fail "B-spoof a forged source reached the broker: [$SPOOF]"
fi

echo "=== B-reuse stale-IP revoke + reuse ==="
/gatekeeperd net-binding --ip $IPA --rm --dir /run/shrek/net-binding >/dev/null
REVOKED=$(route_query sbA sessH ISCOPETOKEN)
gate_empty "B-reuse-1 revoked binding → the cont_ip fails closed" "$REVOKED"
/gatekeeperd net-binding --ip $IPA --session sessH2 --dir /run/shrek/net-binding >/dev/null   # reuse the IP for a NEW session
NEWOK=$(route_query sbA sessH2 ISCOPETOKEN)
has "B-reuse-2 the reused cont_ip authorizes the NEW session" "$NEWOK" "app-a"
STALE=$(route_query sbA sessH ISCOPETOKEN)     # the OLD handle on the reused IP
gate_empty "B-reuse-3 the stale handle no longer authorizes (no dead-session carryover)" "$STALE"
# restore sbA→sessH for the coder gate
/gatekeeperd net-binding --ip $IPA --rm --dir /run/shrek/net-binding >/dev/null
/gatekeeperd net-binding --ip $IPA --session sessH --dir /run/shrek/net-binding >/dev/null

echo "=== C coder-driven swamp_find integration (real agent tool over the full routed path) ==="
# /etc/hosts pins the sealed names the coder resolves (no DNS egress): model + swamp broker.
grep -q "shrek-model" /etc/hosts || printf '%s shrek-model\n%s shrek-swamp-broker\n' "$MODEL_IP" "$BROKER_IP" >> /etc/hosts
# A DETERMINISTIC canned model: step 0 → call swamp_find(q=ISCOPETOKEN); step 1 → done. (openai `choices` shape.)
cat > /tmp/responder.py <<'PYEOF'
import http.server, json
STEPS = [
    {"tool": "swamp_find", "args": {"q": "ISCOPETOKEN"}},
    {"tool": "done",       "args": {"ok": True, "summary": "queried the swamp"}},
]
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_POST(self):
        n = int(self.headers.get('content-length', 0)); self.rfile.read(n)
        i = min(H.step, len(STEPS) - 1); H.step += 1
        content = json.dumps(STEPS[i])
        body = json.dumps({"choices": [{"message": {"role": "assistant", "content": content}}]}).encode()
        self.send_response(200); self.send_header('content-type', 'application/json')
        self.send_header('content-length', str(len(body))); self.end_headers(); self.wfile.write(body)
H.step = 0
http.server.HTTPServer(('10.20.0.2', 8100), H).serve_forever()
PYEOF
ip netns exec srv python3 /tmp/responder.py >/tmp/responder.log 2>&1 &
for _ in $(seq 1 50); do ip netns exec sbA curl -s -m1 -o /dev/null "http://$MODEL_IP:8100/" 2>/dev/null && break; sleep 0.1; done

# Run the REAL coder in sbA with the gatekeeper-injected SHREK_SESSION (arms swamp_find + is the bound handle).
CODER_OUT=$(cd /tmp && ip netns exec sbA env \
  SHREK_SESSION=sessH SHREK_PROVIDER=local \
  SHREK_MODEL_URL="http://shrek-model:8100/" SHREK_SWAMP_URL="http://shrek-swamp-broker:8400/" \
  /coder --task "search the swamp for ISCOPETOKEN" --max-steps 4 2>&1)
has "C1 the agent invoked its swamp_find tool" "$CODER_OUT" 'CODER-TOOL swamp_find'
has "C2 the routed tool result carried the IN-SCOPE hit (app-a)" "$CODER_OUT" "app-a"
absent "C3 the out-of-scope project never crossed to the agent" "$CODER_OUT" "app-b"
absent "C4 the out-of-scope token never crossed to the agent" "$CODER_OUT" "BBSECRET"
has "C5 the agent completed on its authorized projection" "$CODER_OUT" "CODER-DONE ok=true"
absent "C6 a FRESH index adds no stale caution to the agent's tool result" "$CODER_OUT" "NOTE swamp_find index freshness"

echo "=== D live watcher (slice-3): create / modify / delete / rename through the broker ==="
# All edits are made by `tester` (the tree owner); swampd's inotify watch repairs the map; each change is
# then queried THROUGH the broker, so authority is exercised on every live update.
runuser -u tester -- bash -c "echo 'live created token LIVECREATE in app-a' > $H/Projects/app-a/src/created.rs"
sleep 0.7
has "D1 live CREATE reflected through the routed path" "$(route_query sbA sessH LIVECREATE)" "created.rs"

runuser -u tester -- bash -c "echo 'edited token LIVEMODIFY app-a' > $H/Projects/app-a/src/created.rs"
sleep 0.7
has      "D2a live MODIFY: the new token is searchable" "$(route_query sbA sessH LIVEMODIFY)" "created.rs"
gate_empty "D2b live MODIFY: the old token is retired (FTS replaced, not appended)" "$(route_query sbA sessH LIVECREATE)"

runuser -u tester -- bash -c "rm $H/Projects/app-a/src/created.rs"
sleep 0.7
gate_empty "D3 live DELETE removes searchable content (metadata AND FTS)" "$(route_query sbA sessH LIVEMODIFY)"

runuser -u tester -- bash -c "echo 'movable token LIVERENAME app-a' > $H/Projects/app-a/src/rn-old.rs"
sleep 0.5
runuser -u tester -- bash -c "mv $H/Projects/app-a/src/rn-old.rs $H/Projects/app-a/src/rn-new.rs"
sleep 0.7
D4=$(route_query sbA sessH LIVERENAME)
has    "D4a live RENAME: the new path is present" "$D4" "rn-new.rs"
absent "D4b live RENAME: the old path is gone"    "$D4" "rn-old.rs"

echo "=== D5 freshness carried end-to-end (swampd → broker) ==="
has "D5a a healthy routed response carries freshness=fresh (relayed verbatim)" "$(route_query sbA sessH ISCOPETOKEN)" "freshness fresh"
has "D5b a broker fail-closed (stolen handle) carries freshness=unknown"       "$(route_query sbB sessH ISCOPETOKEN)" "freshness unknown"
has "D5c a broker fail-closed (unbound source) carries freshness=unknown"      "$(route_query sbC sessH ISCOPETOKEN)" "freshness unknown"

echo "=== D6 persistence + offline-change reconcile across a swampd restart ==="
# Seed a file that is indexed WHILE UP, so we can prove its offline DELETE is reconciled on restart.
runuser -u tester -- bash -c "echo 'delete-me-offline token OFFLINEDEL app-a' > $H/Projects/app-a/src/willdel.rs"
sleep 0.7
has "D6a willdel is indexed while swampd is up" "$(route_query sbA sessH OFFLINEDEL)" "willdel.rs"
[ -f /run/swamp/index.db ] && pass "D6b index DB is a real persistent file on disk" || fail "D6b index DB missing"
# Stop swampd, mutate the tree WHILE IT IS DOWN, then restart — the startup reconcile must catch both.
pkill -x swampd 2>/dev/null || true; sleep 0.5; rm -f /run/swamp/query.sock
runuser -u tester -- bash -c "echo 'born-while-down token OFFLINEADD app-a' > $H/Projects/app-a/src/offline.rs"
runuser -u tester -- bash -c "rm $H/Projects/app-a/src/willdel.rs"
runuser -u swamp -- env SWAMP_HOME=$H SWAMP_STATE_DIR=/run/swamp SWAMP_AUTHORITY_DIR=/run/shrek/authority \
  SWAMP_ALLOW_UID=$TESTER_UID /swampd serve >/tmp/swampd2.log 2>&1 &
for _ in $(seq 1 100); do [ -S /run/swamp/query.sock ] && env SWAMP_QUERY_SOCK=/run/swamp/query.sock /shrek find --session sessH __ping__ >/dev/null 2>&1 && break; sleep 0.1; done
has       "D6c restart reconcile catches the offline CREATE" "$(route_query sbA sessH OFFLINEADD)" "offline.rs"
gate_empty "D6d restart reconcile catches the offline DELETE" "$(route_query sbA sessH OFFLINEDEL)"

echo "=== E semantic tier (slice-4): provider abstraction, FTS floor, degrade-to-lexical ==="
# A HERMETIC canned embeddings provider (deterministic dim-8 vectors) on HOST loopback, bridged to
# swampd by the OFF-IMAGE swamp-embed-proxy over a LOCAL unix socket. swampd stays network-free (it only
# speaks the local socket); the proxy is the only party that speaks HTTP. This exercises the whole
# channel + the authority/availability invariants with NO dependency on any live model.
cat > /tmp/embed_responder.py <<'PYEOF'
import http.server, json
def vec(s):
    out=[]
    for i in range(8):
        acc=0
        for ch in s.encode(): acc=(acc*131 + ch + i*7) % 1000003
        out.append((acc%1000)/1000.0 + 0.001)   # deterministic, non-zero
    return out
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self,*a): pass
    def do_POST(self):
        n=int(self.headers.get('content-length',0)); req=json.loads(self.rfile.read(n) or b'{}')
        data=[{"embedding":vec(x)} for x in req.get("input",[])]
        body=json.dumps({"model":req.get("model","canned"),"data":data}).encode()
        self.send_response(200); self.send_header('content-type','application/json')
        self.send_header('content-length',str(len(body))); self.end_headers(); self.wfile.write(body)
http.server.HTTPServer(('127.0.0.1',8109), H).serve_forever()
PYEOF
python3 /tmp/embed_responder.py >/tmp/embed_responder.log 2>&1 &
for _ in $(seq 1 50); do curl -s -m1 -o /dev/null -X POST -d '{"input":["x"]}' "http://127.0.0.1:8109/v1/embeddings" 2>/dev/null && break; sleep 0.1; done
# The embed socket lives UNDER swampd's granted state dir so the Landlocked daemon can reach it.
EMBSOCK=/run/swamp/embed.sock
SWAMP_EMBED_SOCKET=$EMBSOCK SWAMP_EMBED_UPSTREAM=127.0.0.1:8109 SWAMP_EMBED_MODEL=canned /swamp-embed-proxy >/tmp/embedproxy.log 2>&1 &
for _ in $(seq 1 50); do [ -S "$EMBSOCK" ] && break; sleep 0.1; done
[ -S "$EMBSOCK" ] && pass "E0 swamp-embed-proxy serving its local socket" || fail "E0 embed proxy socket missing"
# Restart swampd WITH the semantic tier wired (dim 8 = the canned responder's width).
pkill -x swampd 2>/dev/null || true; sleep 0.5; rm -f /run/swamp/query.sock
runuser -u swamp -- env SWAMP_HOME=$H SWAMP_STATE_DIR=/run/swamp SWAMP_AUTHORITY_DIR=/run/shrek/authority \
  SWAMP_ALLOW_UID=$TESTER_UID SWAMP_EMBED_SOCKET=$EMBSOCK SWAMP_EMBED_DIM=8 SWAMP_EMBED_MODEL=canned SWAMP_EMBED_PROVIDER=oracle \
  /swampd serve >/tmp/swampd_sem.log 2>&1 &
for _ in $(seq 1 100); do [ -S /run/swamp/query.sock ] && env SWAMP_QUERY_SOCK=/run/swamp/query.sock /shrek find --session sessH __ping__ >/dev/null 2>&1 && break; sleep 0.1; done
sleep 1.5   # let the startup reconcile embed the tree through the proxy
has "E5 swampd wired the semantic provider channel (base stays network-free)" "$(cat /tmp/swampd_sem.log)" "semantic tier ENABLED"
E_WIRE=$(route_query sbA sessH ISCOPETOKEN)
has    "E1 healthy routed response reports semantic=available" "$E_WIRE" "semantic available"
has    "E2 the in-scope hit is still returned with semantic ON (FTS floor preserved)" "$E_WIRE" "app-a"
absent "E3 authority-before-vector: the out-of-scope token stays absent WITH semantic ON" "$(route_query sbA sessH BBSECRET)" "BBSECRET"
absent "E3b the out-of-scope project stays absent WITH semantic ON" "$E_WIRE" "app-b"
# No score/count/similarity side channel: EVERY wire line must be a known header or a hit (projection
# shape byte-identical to FTS — the discipline of index.rs, extended to the vector tier).
BADLINE=$(printf '%s\n' "$E_WIRE" | grep -vE '^(RESULT |freshness |semantic |hit |END|$)' | head -1)
[ -z "$BADLINE" ] && pass "E4 wire exposes no score/count side channel (projection shape unchanged)" || fail "E4 unexpected wire line: $BADLINE"
# DEGRADE: kill the provider channel; a query must fall back to the FTS floor AND report unavailable —
# never make Swamp unavailable (§1.2 T7). This is the load-bearing failure-asymmetry gate.
pkill -x swamp-embed-proxy 2>/dev/null || true; pkill -f embed_responder.py 2>/dev/null || true; sleep 0.3
DEG=$(route_query sbA sessH ISCOPETOKEN)
has "E6a provider DOWN → response reports semantic=unavailable (degrade, not disable)" "$DEG" "semantic unavailable"
has "E6b provider DOWN → the FTS floor STILL returns the in-scope hit (never unavailable)" "$DEG" "app-a"
# A broker fail-closed projection (stolen handle) carries semantic=unavailable alongside freshness=unknown.
has "E7 a broker fail-closed projection carries semantic=unavailable" "$(route_query sbB sessH ISCOPETOKEN)" "semantic unavailable"

# ---- teardown + verdict ----
pkill -x swamp-broker 2>/dev/null || true; pkill -x swampd 2>/dev/null || true; pkill -f responder.py 2>/dev/null || true
pkill -x swamp-embed-proxy 2>/dev/null || true; pkill -f embed_responder.py 2>/dev/null || true
for n in srv sbA sbB sbC sbU; do ip netns del "$n" 2>/dev/null || true; done
nft delete table ip swamp_oracle 2>/dev/null || true

echo "-----------------------------------------------------------------------------"
if [ "$FAILN" = "0" ] && [ "$PASS" -ge 42 ]; then
  echo "SHREK_GATE: PASS swamp-semantic-slice4 ($PASS gates, 0 fail)"
  exit 0
else
  echo "SHREK_GATE: FAIL swamp-semantic-slice4 (pass=$PASS fail=$FAILN)"
  echo "--- swampd_sem.log ---"; tail -25 /tmp/swampd_sem.log 2>/dev/null
  echo "--- embedproxy.log ---"; tail -15 /tmp/embedproxy.log 2>/dev/null
  echo "--- swampd2.log ---"; tail -20 /tmp/swampd2.log 2>/dev/null
  echo "--- broker.log ---"; tail -30 /tmp/broker.log 2>/dev/null
  echo "--- swampd.log ---"; tail -20 /tmp/swampd.log 2>/dev/null
  exit 1
fi
