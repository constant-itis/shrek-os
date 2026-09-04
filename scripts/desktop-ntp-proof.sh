#!/usr/bin/env bash
# Shrek OS — ADR-007 S5: the NTP baseline wiring proof.
#
# S5 ships one artifact — the sealed timesyncd.conf drop-in that pins the clock source to desktop-ntp's
# LITERAL Cloudflare IPs — and asserts the three sources of that pin never drift apart. It also proves,
# on the host net, that the sealed IP IS a working clock source (boot clock recovery) and that a
# sealed-DoT re-pin succeeds once the clock is sane (the NTP-good → DoT ordering, ADR §5/§11.5).
#
# What S5 does NOT prove — deferred to the sealed-VM dogfood (S6): the COLD-BOOT sequence live — a 2012
# RTC boots with a wrong clock, timesyncd corrects it off the sealed IPs with zero resolution, and only
# THEN does the supervisor's re-pin of weather succeed — under a real compositor. This oracle proves each
# link of that chain fast; S6 proves them in series on real hardware/VM.
#
#   G-conf-exists    the sealed timesyncd.conf drop-in ships at the /usr/lib vendor path
#   G-conf-literals  NTP= is dotted-quad IPv4 literals ONLY — zero hostnames ([R2-MF-C]: no resolution)
#   G-conf-fallback  FallbackNTP= is present and EMPTY — no name-based pool fallback under timesyncd's uid
#   G-consistency    the NTP= IP set == shrek-policy DESKTOP_NTP == baked @ntp_pinned (anti-drift, 3-way)
#   G-updates-inert  desktop-updates stays a fail-closed stub — baked @updates_pinned empty, policy rules 0
#   G-ntp-set-live   the baked table loads in a netns and @ntp_pinned holds exactly the two sealed IPs
#   G-clock-source   a real SNTP round-trip to a sealed IP returns a sane offset (the clock source works)
#   G-ntp-then-dot   with the clock sane, a sealed-DoT resolve of weather returns a real IPv4 (ordering)
set -uo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

PASS=0; FAIL=0; SKIP=0
check() { if [ "$3" -eq 0 ]; then echo "  PASS $1 — $2"; PASS=$((PASS+1)); else echo "  FAIL $1 — $2"; FAIL=$((FAIL+1)); fi; }
skip()  { echo "  SKIP $1 — $2"; SKIP=$((SKIP+1)); }

CONF="$REPO_ROOT/image/overlay/usr/lib/systemd/timesyncd.conf.d/10-shrek-sealed-ntp.conf"
NFT_FILE="$REPO_ROOT/image/overlay/usr/lib/shrek/desktop-egress.nft"
POLICY="$REPO_ROOT/crates/shrek-policy/src/desktop_egress.rs"
ips() { grep -oE '([0-9]{1,3}\.){3}[0-9]{1,3}' | sort -u; }

# ── static arena: the config ships, is name-free, and agrees with policy + the baked table ───────────
echo "=== static (config ↔ policy ↔ nft) ==="
[ -f "$CONF" ]; check "G-conf-exists" "timesyncd drop-in present at sealed /usr/lib path" $?

NTP_LINE="$(grep -E '^NTP=' "$CONF" || true)"
conf_ips="$(printf '%s\n' "$NTP_LINE" | ips)"
# name-free: every token after NTP= must be an IPv4 literal (count of IPs == count of tokens)
ntp_tokens="$(printf '%s' "${NTP_LINE#NTP=}" | tr ' ' '\n' | grep -c . )"
ntp_ipcount="$(printf '%s\n' "$conf_ips" | grep -c . )"
[ -n "$conf_ips" ] && [ "$ntp_tokens" -eq "$ntp_ipcount" ]; check "G-conf-literals" "NTP= is IPv4 literals only ($ntp_ipcount), no names" $?

grep -Eq '^FallbackNTP=[[:space:]]*$' "$CONF"; check "G-conf-fallback" "FallbackNTP= present and empty (no name fallback)" $?

policy_ips="$(sed -n '/const DESKTOP_NTP/,/];/p' "$POLICY" | ips)"
nft_ntp_ips="$(grep -E 'set[[:space:]]+ntp_pinned' "$NFT_FILE" | ips)"
if [ "$conf_ips" = "$policy_ips" ] && [ "$conf_ips" = "$nft_ntp_ips" ]; then r=0; else r=1; fi
check "G-consistency" "NTP= == DESKTOP_NTP == @ntp_pinned {$(echo $conf_ips)}" $r

# desktop-updates: still a deferred, inert, fail-closed stub (Q6b) — empty baked set, zero policy rules.
updates_empty=1
grep -E 'set[[:space:]]+updates_pinned' "$NFT_FILE" | grep -q 'elements' && updates_empty=0
# the stub is `const DESKTOP_UPDATES: &[EgressRule] = &[];` on ONE line — an empty array literal.
policy_updates_empty=0; grep -qE 'const DESKTOP_UPDATES[^=]*=[[:space:]]*&\[\];' "$POLICY" && policy_updates_empty=1
[ "$updates_empty" -eq 1 ] && [ "$policy_updates_empty" -eq 1 ]; check "G-updates-inert" "desktop-updates inert stub (Q6b deferred): @updates_pinned empty, 0 rules" $?

# ── netns arena: the baked table loads and @ntp_pinned carries exactly the sealed IPs ────────────────
echo "=== netns (baked table liveness) ==="
NS_OUT="$(unshare -rn sh -c '
  nft -f "'"$NFT_FILE"'" || { echo "NFT-LOAD-FAIL"; exit 1; }
  nft list set inet shrek_desktop_egress ntp_pinned | tr -d "\n\t "
' 2>&1)"
live_ntp="$(printf '%s' "$NS_OUT" | ips)"
[ "$live_ntp" = "$conf_ips" ]; check "G-ntp-set-live" "kernel-loaded @ntp_pinned == sealed IPs {$(echo $live_ntp)}" $?

# ── host-net arena: the sealed IP is a live clock source + DoT re-pin works once time is sane ─────────
echo "=== host net (live clock source + NTP → DoT ordering) ==="
have_net=0; [ -n "$(ip route show default 2>/dev/null)" ] && have_net=1
first_ip="$(printf '%s\n' "$conf_ips" | head -n1)"

SNTP_PY="$(mktemp --suffix=.py)"
cat > "$SNTP_PY" <<'PYEOF'
import socket, struct, sys, time
NTP_DELTA = 2208988800
p = bytearray(48); p[0] = 0x1b
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.settimeout(5)
try:
    t0 = time.time(); s.sendto(bytes(p), (sys.argv[1], 123))
    data, _ = s.recvfrom(48); t3 = time.time()
except Exception as e:
    print("NONET", e); sys.exit(2)
finally:
    s.close()
stratum = data[1]
sec, frac = struct.unpack('!I', data[40:44])[0], struct.unpack('!I', data[44:48])[0]
server = sec - NTP_DELTA + frac / 2**32
offset = server - (t0 + t3) / 2
if stratum == 0 or stratum > 15:
    print("BADSTRATUM", stratum); sys.exit(3)
print("OK stratum=%d offset=%.3f" % (stratum, offset))
sys.exit(0 if abs(offset) < 5 else 4)
PYEOF

if [ "$have_net" -eq 1 ]; then
  out="$(python3 "$SNTP_PY" "$first_ip" 2>&1)"; rc=$?
  echo "    $out"
  [ "$rc" -eq 0 ]; check "G-clock-source" "SNTP to $first_ip:123 ⇒ sane offset (clock source live)" $?
else
  skip "G-clock-source" "no default route ⇒ offline; sealed IP round-trip is an S6/live gate"
fi

if [ "$have_net" -eq 1 ]; then
  B="$REPO_ROOT/target/release/egressd"
  echo "=== building egressd (release, oracle-env) ===" >&2
  CARGO_NET_OFFLINE=true cargo build --release -p egressd --features oracle-env >/dev/null 2>&1 || \
    cargo build --release -p egressd --features oracle-env >/dev/null 2>&1
  W="$(mktemp -d)"; S="$W/store"; R="$W/run"; mkdir -p "$R"
  export SHREK_EGRESS_STORE="$S" SHREK_EGRESS_RUN="$R" \
         SHREK_EGRESS_DOT_CA="$REPO_ROOT/image/overlay/usr/lib/shrek/dot-ca-roots.pem"
  "$B" store init >/dev/null 2>&1
  "$B" store bless --profile weather --tier one-click --at 500 >/dev/null 2>&1
  # clock is sane (just proven) ⇒ the DoT handshake can succeed ⇒ re-pin resolves a real IPv4
  "$B" resolve --profile weather --at 500 >/dev/null 2>&1
  ip="$(grep '^pin ' "$S/pinned/weather" 2>/dev/null | awk '{print $3}' | head -1)"
  [ -n "$ip" ]; check "G-ntp-then-dot" "clock sane ⇒ sealed-DoT weather re-pin ⇒ real IPv4 ($ip)" $?
  rm -rf "$W"
else
  skip "G-ntp-then-dot" "no default route ⇒ offline; sealed-DoT re-pin is proven in S2/live in S6"
fi
rm -f "$SNTP_PY"

echo
echo "=== S5 NTP baseline: PASS=$PASS FAIL=$FAIL SKIP=$SKIP ==="
[ "$FAIL" -eq 0 ]
