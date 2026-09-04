#!/usr/bin/env bash
# Shrek OS — ADR-007 S2e: the INTEGRATED host oracle for the desktop-egress supervisor (S2a-d).
# Proves the whole S2 machine — store, element-only applier, sealed DoT re-pin, and the uid-1000 socket
# boundary — WITHOUT a VM, using the `oracle-env` build's SHREK_EGRESS_* overrides (mirrors the
# authority-record / net-binding host oracles). The REAL sealed-VM end-to-end (a booted image, a real
# uid-1000 session, the browser cgroup) is the S6 dogfood; this oracle proves the logic fast + regresses
# it.
#
# It runs in three coordinated arenas because nft tables are per-netns and a fresh netns has no
# connectivity, so nft-enforcement and live-DoT cannot share one namespace:
#   A. nft enforcement  — a NETNS (unshare -rn) with the S1-baked table loaded + PRE-SEEDED pins (no DoT).
#   B. sealed DoT        — the HOST net (real 1.1.1.1/9.9.9.9); the table is NEVER loaded here (it would
#                          drop this uid-1000 user's own traffic). Poison arenas use a mount ns.
#   C. socket boundary   — the real daemon + a real UnixStream client on the host (no table/DoT needed;
#                          the accept-bless path's nft + DoT halves are arenas A + B).
#
#   G-split        store is 0700 (unreadable to uid 1000); the /run pinned map is 0644 (the [R2-MF-A] split)
#   G-bless-elem   applying a blessed weather pin lands the IP in @weather_pinned (element-only)
#   G-nonbrowser   the baked rule-0 stub-DROP for a non-browser uid-1000 lookup is present above lo-accept
#   G-unknown      applying a non-pinnable profile parks an unknown-profile fault and writes NO element
#   G-applyfail    an apply with the baked table ABSENT fails closed — a fault, the deny floor stands
#   G-unbless      unbless reconciles @weather_pinned back to empty
#   G-reconcile    a fresh apply (daemon restart) re-adds the stored pins as elements — never flushes
#   G-dot          a live sealed-DoT resolve of weather returns a real IPv4
#   G-dot-hosts    a poisoned /etc/hosts (mount ns) steers getent but NOT the DoT pin (#3121 workaround)
#   G-dot-resolv   a poisoned resolv.conf (uid-1000/NM proxy) likewise does NOT steer the DoT pin
#   G-dot-trust    a wrong trust base ⇒ every resolver rejected ⇒ fail-closed, no pin
#   G-sock-uid     SO_PEERCRED gate: served when the peer == sealed desktop uid, denied when it doesn't
#   G-sock-tier    identity != authority: a uid-1000 bless of a non-Tier-B profile is denied
#   G-sock-parse   verb+profile only: a 3rd field / unknown verb / giant payload are rejected
#   G-sock-rate    a bless flood is rate-limited (denied attempts counted) — no oracle/DoS
#   (browser-scope stub ACCEPT is validated in S6: nft resolves the cgroupv2 path at load, so it needs
#    a real shrek-browser.slice — see apply.rs.)
set -uo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

PASS=0; FAIL=0
check() { # check "NAME" "condition-description" <exit-status-of-preceding-test>
  if [ "$3" -eq 0 ]; then echo "  PASS $1 — $2"; PASS=$((PASS+1));
  else echo "  FAIL $1 — $2"; FAIL=$((FAIL+1)); fi
}

echo "=== building egressd (release, oracle-env) ==="
CARGO_NET_OFFLINE=true cargo build --release -p egressd --features oracle-env >/dev/null 2>&1 || \
  cargo build --release -p egressd --features oracle-env
B="$REPO_ROOT/target/release/egressd"
NFT_FILE="$REPO_ROOT/image/overlay/usr/lib/shrek/desktop-egress.nft"
CA="$REPO_ROOT/image/overlay/usr/lib/shrek/dot-ca-roots.pem"

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
S="$WORK/store"; R="$WORK/run"; mkdir -p "$R"
export SHREK_EGRESS_STORE="$S" SHREK_EGRESS_RUN="$R" SHREK_EGRESS_DOT_CA="$CA"
"$B" store init >/dev/null

# ── arena A: nft enforcement in a netns (baked table + pre-seeded pins, no DoT) ──────────────────────
echo "=== A. nft enforcement (netns) ==="
# seed a bless + pin OUTSIDE the netns (pure store ops, no nft/DoT)
"$B" store bless --profile weather --tier one-click --at 100 >/dev/null
"$B" store pin --profile weather --at 100 --pin api.open-meteo.com=104.16.1.1 --pin api.open-meteo.com=104.16.2.2 >/dev/null
"$B" store project >/dev/null
sm=$(stat -c '%a' "$S"); mm=$(stat -c '%a' "$R/pinned")
[ "$sm" = "700" ] && [ "$mm" = "644" ]; check "G-split" "store=$sm run-map=$mm" $?

A_OUT="$WORK/a.out"
# Emit ONE clean labeled line per result (the applier's own stdout is silenced so it can't sit between
# a marker and the nft output). `set` view is space/tab/newline-stripped for a stable grep.
unshare -rn sh -c '
  nft -f "'"$NFT_FILE"'" || { echo "NFT-LOAD-FAIL"; exit 1; }
  setview() { nft list set inet shrek_desktop_egress weather_pinned | tr -d "\n\t "; }
  echo "RULE0: $(nft -a list chain inet shrek_desktop_egress output | grep "th dport 53 drop" | tr -d "\t")"
  "'"$B"'" apply --profile weather --at 100 >/dev/null 2>&1;          echo "AFTER-BLESS: $(setview)"
  "'"$B"'" apply --profile web-browsing --at 100 >/dev/null 2>&1;     echo "AFTER-UNKNOWN: $(setview)"
  "'"$B"'" apply --profile weather --unbless --at 100 >/dev/null 2>&1; echo "AFTER-UNBLESS: $(setview)"
  "'"$B"'" apply --profile weather --at 100 >/dev/null 2>&1;          echo "AFTER-RECONCILE: $(setview)"
' >"$A_OUT" 2>&1
grep -q 'RULE0:.*skuid 1000 ip daddr.*127.0.0.53.*th dport 53 drop' "$A_OUT"; check "G-nonbrowser" "baked rule-0 stub-DROP present" $?
grep -q 'AFTER-BLESS:.*elements={104.16.1.1,104.16.2.2}' "$A_OUT"; check "G-bless-elem" "both IPs in @weather_pinned" $?
[ -f "$S/fault/web-browsing" ]; check "G-unknown" "unknown-profile fault parked, no element" $?
grep -q 'AFTER-UNBLESS:.*weather_pinned{typeipv4_addr}}' "$A_OUT"; check "G-unbless" "@weather_pinned empty after unbless" $?
grep -q 'AFTER-RECONCILE:.*elements={104.16.1.1,104.16.2.2}' "$A_OUT"; check "G-reconcile" "restart re-adds elements (no flush)" $?

# apply-fail: no table loaded in this netns ⇒ nft element add errors ⇒ fault, fail-closed
"$B" store project >/dev/null
unshare -rn sh -c '"'"$B"'" apply --profile weather --at 101' >/dev/null 2>&1
af=$?
[ "$af" -ne 0 ] && [ -f "$S/fault/weather" ]; check "G-applyfail" "apply w/o baked table ⇒ fault, floor stands" $?
"$B" store fault --profile weather --kind resolve-fail --reason clear --at 0 >/dev/null 2>&1; rm -f "$S/fault/weather"

# ── arena B: sealed DoT on the host net (real resolvers; NEVER load the table here) ──────────────────
echo "=== B. sealed DoT (host net) ==="
rm -f "$S/pinned/weather"
"$B" resolve --profile weather --at 200 >/dev/null 2>&1
ip=$(grep '^pin ' "$S/pinned/weather" 2>/dev/null | awk '{print $3}' | head -1)
[ -n "$ip" ] && [ "$ip" != "6.6.6.6" ]; check "G-dot" "live DoT resolve ⇒ real IPv4 ($ip)" $?

# poisoned /etc/hosts in a mount ns: getent honors it, DoT ignores it
printf '6.6.6.6 api.open-meteo.com\n6.6.6.6 cloudflare-dns.com\n6.6.6.6 dns.quad9.net\n' > "$WORK/ph"
unshare -rm sh -c '
  mount --bind "'"$WORK/ph"'" /etc/hosts
  getent hosts api.open-meteo.com | grep -q 6.6.6.6 || { echo "getent-not-poisoned"; exit 9; }
  "'"$B"'" resolve --profile weather --at 201 >/dev/null 2>&1
  grep "^pin " "'"$S"'/pinned/weather" | awk "{print \$3}" | grep -qv 6.6.6.6
' ; check "G-dot-hosts" "poisoned /etc/hosts steers getent, not the pin" $?

# poisoned resolv.conf (the uid-1000/NM DNS-server steer): DoT ignores resolv.conf entirely
printf 'nameserver 6.6.6.6\n' > "$WORK/prc"
unshare -rm sh -c '
  mount --bind "'"$WORK/prc"'" /etc/resolv.conf
  "'"$B"'" resolve --profile weather --at 202 >/dev/null 2>&1
  ip=$(grep "^pin " "'"$S"'/pinned/weather" | awk "{print \$3}" | head -1)
  [ -n "$ip" ] && [ "$ip" != "6.6.6.6" ]
' ; check "G-dot-resolv" "poisoned resolv.conf/NM DNS does not steer the pin" $?

# wrong trust base ⇒ fail-closed
cp "$S/pinned/weather" "$WORK/pin.bak"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -keyout /dev/null -nodes \
  -subj "/CN=bogus" -days 1 2>/dev/null > "$WORK/bogus-ca.pem"
SHREK_EGRESS_DOT_CA="$WORK/bogus-ca.pem" "$B" resolve --profile weather --at 203 >/dev/null 2>&1
tf=$?
[ "$tf" -ne 0 ] && [ -f "$S/fault/weather" ] && diff -q "$WORK/pin.bak" "$S/pinned/weather" >/dev/null
check "G-dot-trust" "wrong trust base ⇒ all resolvers rejected, prior pin intact" $?
rm -f "$S/fault/weather"

# ── arena C: the socket boundary (real daemon + client on the host) ──────────────────────────────────
echo "=== C. socket boundary ==="
cat > "$WORK/client.py" <<'PY'
import socket, os, time, sys
SOCK=os.environ["SOCK"]
def req(line):
    for _ in range(60):
        try:
            s=socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.settimeout(6); s.connect(SOCK); break
        except (FileNotFoundError, ConnectionRefusedError): time.sleep(0.1)
    else: return "NO-SOCKET"
    try: s.sendall(line if isinstance(line,bytes) else (line+"\n").encode())
    except (BrokenPipeError, ConnectionResetError): pass
    d=b""
    try:
        while True:
            c=s.recv(4096)
            if not c: break
            d+=c
    except (socket.timeout, ConnectionResetError): pass
    s.close(); return d.decode(errors="replace").strip() or "REJECTED-CLOSED"
print(req(sys.argv[1] if len(sys.argv)>1 else "status"))
PY
runclient(){ SOCK="$1" python3 "$WORK/client.py" "$2"; }

# C1: daemon expects THIS uid ⇒ served
DR="$WORK/run-accept"; mkdir -p "$DR"; DSOCK="$DR/sock"
SHREK_EGRESS_RUN="$DR" SHREK_EGRESS_SOCK="$DSOCK" SHREK_EGRESS_DESKTOP_UID="$(id -u)" \
  setsid "$B" daemon >"$WORK/d-accept.log" 2>&1 & DP=$!
out=$(runclient "$DSOCK" "status")
echo "$out" | grep -q '^OK status'; check "G-sock-uid(accept)" "peer==desktop uid ⇒ $out" $?
tier=$(runclient "$DSOCK" "bless web-browsing")
echo "$tier" | grep -q 'ERR denied'; check "G-sock-tier" "uid-1000 bless of non-Tier-B ⇒ $tier" $?
p1=$(runclient "$DSOCK" "bless weather evil.example.com")
echo "$p1" | grep -q 'too many fields'; check "G-sock-parse(3rd)" "smuggled 3rd field ⇒ $p1" $?
p2=$(runclient "$DSOCK" "frobnicate weather")
echo "$p2" | grep -q 'unknown verb'; check "G-sock-parse(verb)" "unknown verb ⇒ $p2" $?
den=0; lim=0
for i in $(seq 1 12); do r=$(runclient "$DSOCK" "bless web-browsing");
  case "$r" in *"rate-limited"*) lim=$((lim+1));; *denied*) den=$((den+1));; esac; done
[ "$lim" -gt 0 ] && [ "$den" -gt 0 ]; check "G-sock-rate" "bless flood ⇒ $den denied then $lim rate-limited" $?
kill $DP 2>/dev/null; wait $DP 2>/dev/null

# C2: daemon expects a DIFFERENT uid ⇒ this peer denied
DR2="$WORK/run-reject"; mkdir -p "$DR2"; DSOCK2="$DR2/sock"
SHREK_EGRESS_RUN="$DR2" SHREK_EGRESS_SOCK="$DSOCK2" SHREK_EGRESS_DESKTOP_UID="$(( $(id -u) + 1 ))" \
  setsid "$B" daemon >"$WORK/d-reject.log" 2>&1 & DP2=$!
rej=$(runclient "$DSOCK2" "status")
echo "$rej" | grep -q 'peer is not the desktop user'; check "G-sock-uid(reject)" "peer!=desktop uid ⇒ $rej" $?
kill $DP2 2>/dev/null; wait $DP2 2>/dev/null

echo "======================================================================"
echo "desktop-egress S2 oracle: PASS=$PASS FAIL=$FAIL"
echo "  (browser-scope stub ACCEPT deferred to S6: nft binds the cgroupv2 path at load — needs a real"
echo "   shrek-browser.slice; the non-browser DROP is proven above via the baked rule-0.)"
[ "$FAIL" -eq 0 ]