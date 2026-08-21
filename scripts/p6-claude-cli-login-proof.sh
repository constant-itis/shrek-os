#!/usr/bin/env bash
# Phase-6 slice-5 — the "Sign in with Claude" login UX proof (docs/phase6-slice5-claude-login-ux.md).
#
# Slice-4 (docs/phase6-slice4-claude-cli-broker.md) shipped a subscription provider that ASSUMES a
# pre-logged-in `claude` CLI. Slice-5 closes that gap: `claude-broker login` hands the operator's REAL
# terminal to the official `claude auth login --claudeai` (the CLI owns ALL credential state), then
# VERIFIES completion with ONE real `claude -p` round-trip — never `claude auth status` (#1567 lies) —
# and records an AUDIT-ONLY availability breadcrumb. No token is ever read, parsed, or stored.
#
# The "claude" here is a DETERMINISTIC FAKE on PATH: it logs every invocation, simulates `auth login`
# (exit code via FAKE_LOGIN_RC) and `-p` (via FAKE_PROBE_MODE), uses NO network and NO real credential,
# and even prints a token-SHAPED line during login so we can prove the broker never persists it.
#
# What this gate proves (unit tests cover the pure breadcrumb pieces):
#   L1  a successful login records available=true / reason=verified, 0600, and NO sk-ant token.
#   L2  completion is verified by a REAL `claude -p` round-trip; `auth status` is NEVER consulted.
#   L3  the breadcrumb carries only audit fields — no CLI output (no result/pong/content) leaks in.
#   L4  a failed `auth login` → available=false / reason=login-failed, non-zero rc, and NO round-trip.
#   L5  a non-TTY invocation fails CLOSED fast (never hangs — #595): reason=non-tty, claude NEVER exec'd.
#   L6  the health probe classifies round-trip outcomes: ok→verified, auth→auth-failed, other→probe-failed;
#       and `auth status` is never consulted on any of them.
#
# Runs UNPRIVILEGED on docker's DEFAULT bridge (never --network host, #2651); no gVisor/root needed —
# `claude-broker` is broker-side and off the sealed image, so (like model-proxy) THE ORACLE IS THE GATE.
set -uo pipefail

if [ "${IN_CONTAINER:-0}" != "1" ]; then
  REPO_ROOT="$(git rev-parse --show-toplevel)"
  echo "=== building claude-broker (host) ==="
  ( cd "$REPO_ROOT" && cargo build --release -p claude-broker ) || exit 3
  echo "=== running slice-5 login/health oracle in an UNPRIVILEGED debian:trixie container (default bridge) ==="
  exec docker run --rm -e IN_CONTAINER=1 \
    -v "$REPO_ROOT":/w:ro -w /w \
    debian:trixie bash scripts/p6-claude-cli-login-proof.sh
fi

# -------------------------------------- in-container --------------------------------------
BROKER=/w/target/release/claude-broker
WORK=/tmp/slice5
rm -rf "$WORK"; mkdir -p "$WORK/bin" "$WORK/state"
export INVLOG="$WORK/claude-invocations.log"; : > "$INVLOG"

# A PTY is needed only for the `login` cases (it self-refuses without a real terminal, by design). Prefer
# util-linux `script`; it is essential in Debian base images, but install as a fallback if absent.
if ! command -v script >/dev/null 2>&1; then
  apt-get update -qq >/dev/null 2>&1 && apt-get install -y -qq util-linux >/dev/null 2>&1 || true
fi
have_pty=1; command -v script >/dev/null 2>&1 || have_pty=0

# The deterministic fake `claude`.
cat > "$WORK/bin/claude" <<'FAKE'
#!/usr/bin/env bash
# Deterministic fake `claude`: logs its invocation; simulates `auth login` and `-p`. NO network, NO creds.
echo "INVOKE $*" >> "$INVLOG"
case "$1" in
  auth)
    case "${2:-}" in
      login)
        # The CLI owns the terminal and (here) even prints a token-SHAPED line to ITS OWN stdout — which
        # the broker inherits but must NEVER capture or persist. This is the no-capture tripwire.
        echo "fake claude: OAuth complete. token=sk-ant-oat01-FAKE-DO-NOT-STORE"
        exit "${FAKE_LOGIN_RC:-0}" ;;
      status)
        # If the broker EVER shells this, the proof fails — it is the #1567 liar we must never trust.
        echo "loggedIn: true (THIS MUST NEVER BE CONSULTED)"; exit 0 ;;
      *) echo "fake claude: unknown auth subcommand ${2:-}" >&2; exit 64 ;;
    esac ;;
  -p)
    case "${FAKE_PROBE_MODE:-ok}" in
      ok)   echo '{"type":"result","subtype":"success","is_error":false,"result":"pong"}'; exit 0 ;;
      auth) echo "Error: 401 Unauthorized (invalid oauth token)" >&2; exit 1 ;;
      fail) echo "boom: model produced nothing" >&2; exit 1 ;;
      *)    echo "fake claude: bad FAKE_PROBE_MODE" >&2; exit 64 ;;
    esac ;;
  *) echo "fake claude: unhandled argv: $*" >&2; exit 64 ;;
esac
FAKE
chmod +x "$WORK/bin/claude"

export SHREK_CLAUDE_BIN="$WORK/bin/claude"
export SHREK_CLAUDE_STATE_DIR="$WORK/state"
export SHREK_CLAUDE_DEFAULT_MODEL=haiku
export PATH="$WORK/bin:$PATH"
BC="$WORK/state/availability.json"

PASS=0; FAIL=0
ok(){ echo "  PASS ✅ $1"; PASS=$((PASS+1)); }
no(){ echo "  FAIL ❌ $1"; FAIL=$((FAIL+1)); }
# Run the broker under a PTY (so `login` sees a real terminal) and capture its exit code.
run_login_tty(){ script -qec "$BROKER login" /dev/null >/dev/null 2>&1; }

echo ""
echo "--- L1/L2/L3: successful login → verified breadcrumb, real round-trip, no token/output leak ---"
if [ "$have_pty" = 1 ]; then
  : > "$INVLOG"
  FAKE_LOGIN_RC=0 FAKE_PROBE_MODE=ok run_login_tty; RC=$?
  [ "$RC" = 0 ] && ok "login exits 0 on success" || no "login exit=$RC"
  grep -q '"reason":"verified"' "$BC" 2>/dev/null && grep -q '"available":true' "$BC" \
    && ok "breadcrumb available=true reason=verified" || no "breadcrumb: $(cat "$BC" 2>/dev/null)"
  MODE=$(stat -c '%a' "$BC" 2>/dev/null); [ "$MODE" = 600 ] && ok "breadcrumb mode 0600" || no "breadcrumb mode=$MODE"
  ls "$WORK/state/.availability.json.tmp" >/dev/null 2>&1 && no ".tmp leftover" || ok "atomic rename left no .tmp"
  grep -q 'sk-ant' "$BC" 2>/dev/null && no "TOKEN LEAKED into breadcrumb" || ok "no sk-ant token in breadcrumb"
  grep -q 'INVOKE auth login' "$INVLOG" && ok "auth login was invoked" || no "auth login not invoked"
  grep -q 'INVOKE -p'         "$INVLOG" && ok "login verified by a REAL -p round-trip" || no "no -p round-trip"
  grep -q 'INVOKE auth status' "$INVLOG" && no "auth status was consulted (#1567 liar)" || ok "auth status NEVER consulted"
  grep -Eq '"result"|pong|content' "$BC" 2>/dev/null && no "breadcrumb carries CLI output" || ok "breadcrumb carries no CLI output"
else
  no "no PTY tool (script) available — cannot exercise the login TTY path"
fi

echo ""
echo "--- L4: failed \`claude auth login\` → login-failed, non-zero rc, NO round-trip ---"
if [ "$have_pty" = 1 ]; then
  : > "$INVLOG"
  FAKE_LOGIN_RC=7 FAKE_PROBE_MODE=ok run_login_tty; RC=$?
  [ "$RC" != 0 ] && ok "failed login exits non-zero ($RC)" || no "failed login exited 0"
  grep -q '"reason":"login-failed"' "$BC" && grep -q '"available":false' "$BC" \
    && ok "breadcrumb available=false reason=login-failed" || no "breadcrumb: $(cat "$BC" 2>/dev/null)"
  grep -q 'INVOKE -p' "$INVLOG" && no "-p ran after a FAILED login" || ok "no round-trip after failed login"
else
  no "no PTY tool (script) available — cannot exercise the login failure path"
fi

echo ""
echo "--- L5: non-TTY login → fail-closed fast, reason=non-tty, claude NEVER exec'd ---"
: > "$INVLOG"
FAKE_PROBE_MODE=ok "$BROKER" login < /dev/null > "$WORK/nontty.out" 2>&1; RC=$?
[ "$RC" != 0 ] && ok "non-tty login refused, non-zero rc ($RC)" || no "non-tty login exited 0"
grep -q '"reason":"non-tty"' "$BC" && grep -q '"available":false' "$BC" \
  && ok "breadcrumb reason=non-tty" || no "breadcrumb: $(cat "$BC" 2>/dev/null)"
[ -s "$INVLOG" ] && no "claude was invoked despite non-tty" || ok "claude NEVER exec'd on non-tty (fail-closed before exec)"
grep -qi 'non-tty' "$WORK/nontty.out" && ok "operator got a clear non-tty refusal message" || no "no clear refusal message"

echo ""
echo "--- L6: health probe classifies round-trip outcomes; never consults auth status ---"
: > "$INVLOG"; FAKE_PROBE_MODE=ok  "$BROKER" health >/dev/null 2>&1; RC=$?
[ "$RC" = 0 ] && grep -q '"reason":"verified"' "$BC" && ok "health ok → verified (rc 0)" || no "health ok path"
FAKE_PROBE_MODE=auth "$BROKER" health >/dev/null 2>&1; RC=$?
[ "$RC" != 0 ] && grep -q '"reason":"auth-failed"' "$BC" && grep -q '"available":false' "$BC" \
  && ok "health auth-failure → auth-failed (rc $RC)" || no "health auth path"
FAKE_PROBE_MODE=fail "$BROKER" health >/dev/null 2>&1; RC=$?
[ "$RC" != 0 ] && grep -q '"reason":"probe-failed"' "$BC" && ok "health other-failure → probe-failed (rc $RC)" || no "health fail path"
grep -q 'INVOKE auth status' "$INVLOG" && no "health consulted auth status" || ok "health never consulted auth status"

echo ""
echo "=== slice-5 login/health oracle: $PASS passed, $FAIL failed ==="
if [ "$FAIL" = 0 ]; then echo "ALL PASS ✅"; exit 0; else echo "FAILURES ❌"; exit 1; fi
