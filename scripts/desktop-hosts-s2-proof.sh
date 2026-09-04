#!/usr/bin/env bash
# ADR-008 S2 — root-authoritative hosts composition + the owner's bounded provider-bind, host oracle.
#
# The inline `--lib` tests already prove the pure store/compose/migration logic against redirected paths.
# This proof adds what a unit test cannot: the REAL `egressd compose-hosts` CLI writing a REAL /run
# projection, REAL on-disk migration of a pre-fix legacy file, and the REAL uid-1000 `egressd ask bind`
# client driving the supervisor over a REAL SO_PEERCRED socket. Native (oracle-env) build + temp dirs;
# no root, no nft, no VM.
#
# Gates (each independent so we learn WHERE it breaks):
#   HOSTS-baseline     compose with an empty store installs /run/shrek/hosts = localhost baseline only
#   HOSTS-migrate      a pre-fix legacy hosts file: the 4 model lines migrate, EVERY other line is
#                      stripped (attacker/public/NTP/swamp-broker), and the legacy path is re-owned to
#                      a root localhost baseline for rollback-compat
#   HOSTS-symlink      a symlinked binding store is ignored (baseline-only), and the secret is untouched
#   HOSTS-bind         `egressd ask bind local <ip>` over the socket -> <ip> shrek-model in the projection
#   HOSTS-event        the bind is journaled to /run/shrek/egress/events WITH the bound address [R1-MF8c]
#   HOSTS-unbind       `egressd ask unbind local` removes the line; the projection returns to baseline
#   HOSTS-idempotent   a second unbind of the now-unbound provider is a clean OK
#   HOSTS-deny-token   `egressd ask bind swamp <ip>` is REFUSED server-side (not a sealed provider)
#   HOSTS-deny-addr    `egressd ask bind local 0x7f000001` is refused (not an IPv4 literal)
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

BASE="out/desktop-hosts-s2-proof"
rm -rf "$BASE"; mkdir -p "$BASE"
ABS="$(readlink -f "$BASE")"

PASS=0; FAIL=0
gate() { if [ "$1" = ok ]; then echo "SHREK_GATE: PASS $2"; PASS=$((PASS+1)); else echo "SHREK_GATE: FAIL $2"; FAIL=$((FAIL+1)); fi; }

BASELINE=$'127.0.0.1 localhost\n::1 localhost'

echo "=== building egressd (release, oracle-env) ==="
CARGO_NET_OFFLINE=true cargo build --release -p egressd --features oracle-env >/dev/null 2>&1 || \
  cargo build --release -p egressd --features oracle-env
BIN="target/release/egressd"

# Redirect BOTH planes: the hosts store/projection (SHREK_HOSTS_*) and the egress store/socket/events
# (SHREK_EGRESS_*), all under the temp base so nothing touches the real /home or /run.
export SHREK_HOSTS_HOME="$ABS/home"          # /home/.shrek-system stand-in (hosts-bindings + legacy)
export SHREK_HOSTS_RUN="$ABS/run"            # /run/shrek stand-in (the projection)
export SHREK_EGRESS_STORE="$ABS/egress-store"
export SHREK_EGRESS_RUN="$ABS/egress-run"    # the events file lands here
export SHREK_EGRESS_SOCK="$ABS/egress-run/sock"
export SHREK_EGRESS_DESKTOP_UID="$(id -u)"
mkdir -p "$SHREK_HOSTS_HOME" "$SHREK_HOSTS_RUN" "$SHREK_EGRESS_RUN"
PROJ="$SHREK_HOSTS_RUN/hosts"
BINDINGS="$SHREK_HOSTS_HOME/hosts-bindings"
LEGACY="$SHREK_HOSTS_HOME/hosts"

# ── Phase 1: the compose CLI on real files (no daemon needed) ─────────────────────────────────────
echo "--- HOSTS-baseline ---"
"$BIN" compose-hosts >/dev/null
if [ "$(cat "$PROJ")" = "$BASELINE" ]; then gate ok HOSTS-baseline; else gate no HOSTS-baseline; echo "  got: $(cat "$PROJ")"; fi

echo "--- HOSTS-migrate (pre-fix legacy file: model lines absorbed, the rest stripped) ---"
cat > "$LEGACY" <<'LEGACYEOF'
127.0.0.1 localhost
192.168.1.10 shrek-model
10.0.0.2 shrek-model-proxy
6.6.6.6 github.com
6.6.6.6 time.cloudflare.com
7.7.7.7 shrek-model
10.0.0.3 shrek-claude-cli
10.0.0.4 shrek-codex-cli
6.6.6.6 shrek-swamp-broker
LEGACYEOF
"$BIN" compose-hosts >/dev/null
PROJ_BODY="$(cat "$PROJ")"
if grep -q '^192.168.1.10 shrek-model$' "$PROJ" \
   && grep -q '^10.0.0.2 shrek-model-proxy$' "$PROJ" \
   && grep -q '^10.0.0.3 shrek-claude-cli$' "$PROJ" \
   && grep -q '^10.0.0.4 shrek-codex-cli$' "$PROJ" \
   && ! grep -q '6.6.6.6' "$PROJ" \
   && ! grep -q '7.7.7.7' "$PROJ" \
   && ! grep -q 'github.com' "$PROJ" \
   && ! grep -q 'shrek-swamp-broker' "$PROJ" \
   && [ "$(cat "$LEGACY")" = "$BASELINE" ] \
   && grep -q '^local 192.168.1.10$' "$BINDINGS"; then
  gate ok HOSTS-migrate
else
  gate no HOSTS-migrate; echo "  projection:"; sed 's/^/    /' "$PROJ"; echo "  legacy: $(cat "$LEGACY")"
fi

echo "--- HOSTS-symlink (a symlinked binding store is ignored, secret untouched) ---"
rm -f "$BINDINGS" "$LEGACY"
SECRET="$ABS/secret"; echo 'codex 6.6.6.6' > "$SECRET"
ln -s "$SECRET" "$BINDINGS"
"$BIN" compose-hosts >/dev/null
if [ "$(cat "$PROJ")" = "$BASELINE" ] && [ "$(cat "$SECRET")" = 'codex 6.6.6.6' ] && [ ! -L "$BINDINGS" -o -f "$BINDINGS" ]; then
  # the projection ignored the symlinked store; the secret was never followed for a write.
  gate ok HOSTS-symlink
else
  gate no HOSTS-symlink; echo "  proj: $(cat "$PROJ")  secret: $(cat "$SECRET")"
fi
rm -f "$BINDINGS" "$LEGACY" "$SECRET"

# ── Phase 2: the real uid-1000 socket client ↔ supervisor ─────────────────────────────────────────
echo "=== starting supervisor ==="
"$BIN" store init >/dev/null
"$BIN" daemon >"$BASE/daemon.log" 2>&1 &
DPID=$!
cleanup() { kill "$DPID" 2>/dev/null || true; wait "$DPID" 2>/dev/null || true; }
trap cleanup EXIT
for _ in $(seq 1 60); do [ -S "$SHREK_EGRESS_SOCK" ] && break; sleep 0.05; done

echo "--- HOSTS-bind (ask bind local -> projection maps token to sealed host) ---"
BIND_OUT="$("$BIN" ask bind local 192.168.1.152 2>&1 || true)"
echo "  $BIND_OUT"
if echo "$BIND_OUT" | grep -q '^OK bind local 192.168.1.152' && grep -q '^192.168.1.152 shrek-model$' "$PROJ"; then
  gate ok HOSTS-bind
else
  gate no HOSTS-bind; echo "  proj:"; sed 's/^/    /' "$PROJ"
fi

echo "--- HOSTS-event (the bind is audited WITH the address) ---"
if grep -q 'bind local 192.168.1.152' "$SHREK_EGRESS_RUN/events" 2>/dev/null; then
  gate ok HOSTS-event
else
  gate no HOSTS-event; echo "  events: $(cat "$SHREK_EGRESS_RUN/events" 2>/dev/null || echo MISSING)"
fi

echo "--- HOSTS-unbind (ask unbind local -> back to baseline) ---"
UNBIND_OUT="$("$BIN" ask unbind local 2>&1 || true)"
echo "  $UNBIND_OUT"
if echo "$UNBIND_OUT" | grep -q '^OK unbind local' && [ "$(cat "$PROJ")" = "$BASELINE" ]; then
  gate ok HOSTS-unbind
else
  gate no HOSTS-unbind; echo "  proj: $(cat "$PROJ")"
fi

echo "--- HOSTS-idempotent (a second unbind is a clean OK) ---"
UNBIND2="$("$BIN" ask unbind local 2>&1 || true)"
if echo "$UNBIND2" | grep -q '^OK unbind local'; then gate ok HOSTS-idempotent; else gate no HOSTS-idempotent; echo "  got: $UNBIND2"; fi

echo "--- HOSTS-deny-token (bind swamp -> refused server-side, not a sealed provider) ---"
DENY_TOK="$("$BIN" ask bind swamp 1.2.3.4 2>&1 || true)"
echo "  $DENY_TOK"
# either the client refuses (exit 2, no socket) OR the daemon replies ERR — both are a refusal; assert
# NOT bound and no OK.
if ! echo "$DENY_TOK" | grep -q '^OK' && ! grep -q 'shrek-swamp-broker\|swamp' "$PROJ" 2>/dev/null; then
  gate ok HOSTS-deny-token
else
  gate no HOSTS-deny-token
fi

echo "--- HOSTS-deny-addr (bind local 0x7f000001 -> refused, not an IPv4 literal) ---"
DENY_ADDR="$("$BIN" ask bind local 0x7f000001 2>&1 || true)"
echo "  $DENY_ADDR"
if ! echo "$DENY_ADDR" | grep -q '^OK' && ! grep -q '0x7f000001\|127.0.0.1 shrek-model' "$PROJ" 2>/dev/null; then
  gate ok HOSTS-deny-addr
else
  gate no HOSTS-deny-addr
fi

echo ""
echo "==================================================================="
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
