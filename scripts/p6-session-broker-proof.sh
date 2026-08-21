#!/usr/bin/env bash
# Phase-6 slice-7 — cross-provider stateful sessions proof (HOST-SIDE, deterministic, no quota).
# docs/phase6-slice7-cross-provider-session.md.
#
# WHAT ONLY A REAL RUN SHOWS (unit tests cover the pure pieces — argv shape, handle hygiene, registry):
# the END-TO-END session mechanic across two turns of a real HTTP exchange with the REAL broker binaries.
# The "model" is a DETERMINISTIC FAKE CLI (records its argv, emits a canned reply, and — for codex —
# writes a codex-shaped rollout so the broker captures a native session id). No network, no credential.
#
#   S1  claude-broker: turn-1 (create) invokes `claude --session-id <UUID>` with the FULL transcript; the
#       reply comes back wrapped. Turn-2 (same X-Shrek-Session handle) invokes `claude --resume <SAME UUID>`
#       with ONLY the new tail turn — the earlier turn's text is ABSENT (delta-only forwarding).
#   S2  claude-broker: a DIFFERENT handle mints a DIFFERENT native id (no cross-session bleed); the raw
#       handle never appears in the CLI argv (a broker-owned UUID does).
#   S3  claude-broker: when `--resume` FAILS, the broker falls back to a stateless FULL-transcript flatten
#       (no --resume) and still returns 200 — sessions are an optimization over an always-correct base.
#   S4  codex-broker (REAL bwrap): turn-1 (create) runs a sessioned `codex exec` (NO --ephemeral) with the
#       per-session dir bound rw as $CODEX_HOME/sessions; the broker CAPTURES codex's own session id from
#       the rollout. Turn-2 runs `codex exec resume <captured id>` with ONLY the tail turn.
#   S5  codex-broker (REAL bwrap): DURING the resumed call the sterile view still holds — host vault is
#       ENOENT, auth.json is RO (a write attempt fails), and the auth canary is absent from the reply. The
#       reader-disable flags are present on the resume call too.
set -uo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK=$(mktemp -d /tmp/p6-session-XXXXXX)
CLAUDE_PORT=18300
CODEX_PORT=18301
fails=0
pass(){ echo "  PASS $*"; }
fail(){ echo "  FAIL $*"; fails=$((fails+1)); }

cleanup(){ pkill -x claude-broker 2>/dev/null; pkill -x codex-broker 2>/dev/null; rm -rf "$WORK" "${CXWORK:-}"; }
trap cleanup EXIT

echo "== build brokers =="
cargo build -q -p claude-broker -p codex-broker --bins || { echo "build failed"; exit 2; }
CLAUDE_BROKER="$ROOT/target/debug/claude-broker"
CODEX_BROKER="$ROOT/target/debug/codex-broker"

# NUL-delimited argv helpers (prompts contain newlines; line-based grep is unsafe).
load_args(){ mapfile -d '' -t ARGS < "$1"; }
arg_after(){ local f=$1 i; for ((i=0;i<${#ARGS[@]};i++)); do [ "${ARGS[i]}" = "$f" ] && { printf '%s' "${ARGS[i+1]}"; return 0; }; done; return 1; }
has_exact(){ local f=$1 a; for a in "${ARGS[@]}"; do [ "$a" = "$f" ] && return 0; done; return 1; }

post(){ # port handle body -> response body on stdout
  local port=$1 handle=$2 body=$3; local hdr=()
  [ -n "$handle" ] && hdr=(-H "X-Shrek-Session: $handle")
  curl -s -m 25 "${hdr[@]}" -H 'content-type: application/json' --data-binary "$body" \
    "http://127.0.0.1:$port/v1/messages"
}

########################################################################################################
echo "== S1..S3  claude-broker session mechanic (fake claude, no bwrap) =="
########################################################################################################
mkdir -p "$WORK/cbin"
FAKE_CLAUDE="$WORK/cbin/claude"
cat > "$FAKE_CLAUDE" <<EOS
#!/usr/bin/env bash
# Record argv NUL-delimited to a numbered file, then emit a claude --output-format json result.
n=\$(( \$(cat "$WORK/claude-n" 2>/dev/null || echo 0) + 1 )); echo "\$n" > "$WORK/claude-n"
printf '%s\0' "\$@" > "$WORK/claude-call-\$n"
if [ -f "$WORK/fail-resume" ]; then
  for a in "\$@"; do [ "\$a" = "--resume" ] && { echo '{"is_error":true,"subtype":"auth","result":"boom"}'; exit 1; }; done
fi
echo '{"type":"result","is_error":false,"result":"pong"}'
EOS
chmod +x "$FAKE_CLAUDE"

SHREK_CLAUDE_BIN="$FAKE_CLAUDE" SHREK_CLAUDE_BROKER_LISTEN="127.0.0.1:$CLAUDE_PORT" \
  SHREK_CLAUDE_STATE_DIR="$WORK/claude-state" "$CLAUDE_BROKER" serve >"$WORK/claude-broker.log" 2>&1 &
for _ in $(seq 1 50); do curl -s -m1 "http://127.0.0.1:$CLAUDE_PORT/" >/dev/null 2>&1 && break; sleep 0.1; done

# Turn 1 (create): one user turn with a unique marker.
R1=$(post "$CLAUDE_PORT" "sess-AAA" '{"model":"haiku","system":"SYS-PROTO","messages":[{"role":"user","content":"UNIQUE1-first-task"}]}')
echo "$R1" | grep -q '"text":"pong"' && pass "S1 turn-1 reply wrapped (pong)" || fail "S1 turn-1 no wrapped reply: $R1"
load_args "$WORK/claude-call-1"
UUID=$(arg_after --session-id) || true
if [ -n "${UUID:-}" ]; then pass "S1 turn-1 used --session-id $UUID (create)"; else fail "S1 turn-1 missing --session-id"; fi
P1=$(arg_after -p); echo "$P1" | grep -q "UNIQUE1-first-task" && pass "S1 turn-1 forwarded the full transcript" || fail "S1 turn-1 prompt missing task"
has_exact "--system-prompt" && pass "S1 turn-1 set --system-prompt on create" || fail "S1 turn-1 no --system-prompt"

# Turn 2 (resume, SAME handle): prior turn + CLI's own assistant reply + a NEW tail turn.
R2=$(post "$CLAUDE_PORT" "sess-AAA" '{"model":"haiku","system":"SYS-PROTO","messages":[{"role":"user","content":"UNIQUE1-first-task"},{"role":"assistant","content":"ASSIST1-reply"},{"role":"user","content":"TAIL2-second-turn"}]}')
echo "$R2" | grep -q '"text":"pong"' && pass "S1 turn-2 reply wrapped" || fail "S1 turn-2 no wrapped reply: $R2"
load_args "$WORK/claude-call-2"
RID=$(arg_after --resume) || true
[ "${RID:-}" = "$UUID" ] && pass "S1 turn-2 resumed the SAME native session ($RID)" || fail "S1 turn-2 --resume id mismatch: got '${RID:-}' want '$UUID'"
P2=$(arg_after -p)
echo "$P2" | grep -q "TAIL2-second-turn" && pass "S1 turn-2 forwarded the new tail turn" || fail "S1 turn-2 missing tail"
if echo "$P2" | grep -q "UNIQUE1-first-task"; then fail "S1 turn-2 RE-SENT the prior turn (not delta-only)"; else pass "S1 turn-2 delta-only: prior turn ABSENT"; fi
if echo "$P2" | grep -q "ASSIST1-reply"; then fail "S1 turn-2 re-sent the CLI's own prior reply"; else pass "S1 turn-2 did not re-send the CLI's own reply"; fi

# S2: a different handle → a different broker-owned native id; raw handle never in argv.
post "$CLAUDE_PORT" "sess-BBB" '{"model":"haiku","messages":[{"role":"user","content":"other"}]}' >/dev/null
load_args "$WORK/claude-call-3"
UUID2=$(arg_after --session-id) || true
[ -n "${UUID2:-}" ] && [ "$UUID2" != "$UUID" ] && pass "S2 different handle → different native id" || fail "S2 native id not distinct: $UUID2 vs $UUID"
if has_exact "sess-BBB"; then fail "S2 raw handle leaked into CLI argv"; else pass "S2 raw handle never in argv (broker-owned id only)"; fi

# S3: resume failure → stateless full-transcript flatten fallback, still 200.
touch "$WORK/fail-resume"
post "$CLAUDE_PORT" "sess-CCC" '{"model":"haiku","system":"SYS","messages":[{"role":"user","content":"FB-create"}]}' >/dev/null   # create ok
RF=$(post "$CLAUDE_PORT" "sess-CCC" '{"model":"haiku","system":"SYS","messages":[{"role":"user","content":"FB-create"},{"role":"assistant","content":"a"},{"role":"user","content":"FB-tail"}]}')
rm -f "$WORK/fail-resume"
echo "$RF" | grep -q '"text":"pong"' && pass "S3 resume-failure still returned 200 (fallback)" || fail "S3 fallback did not return: $RF"
# The LAST fake call must be a stateless flatten: NO --resume, and the FULL transcript present.
LAST=$(cat "$WORK/claude-n"); load_args "$WORK/claude-call-$LAST"
if has_exact "--resume"; then fail "S3 fallback still used --resume"; else pass "S3 fallback dropped to stateless (no --resume)"; fi
PF=$(arg_after -p); echo "$PF" | grep -q "FB-create" && echo "$PF" | grep -q "FB-tail" && pass "S3 fallback re-sent the FULL transcript (correct)" || fail "S3 fallback transcript incomplete"

pkill -x claude-broker 2>/dev/null; sleep 0.2

########################################################################################################
echo "== S4..S5  codex-broker session mechanic (REAL bwrap, fake codex) =="
########################################################################################################
command -v bwrap >/dev/null || { echo "  SKIP codex part: bwrap not installed"; echo; [ "$fails" -eq 0 ] && { echo "SESSION-PROOF: ALL PASS"; exit 0; } || { echo "SESSION-PROOF: $fails FAILED"; exit 1; }; }

# Codex fixtures MUST live under $HOME, not /tmp: the sterile view mounts `--tmpfs /home` (before the
# runtime ro-bind, so a runtime under $HOME survives) but also `--tmpfs /tmp` AFTER it — so a fake runtime
# under /tmp would be shadowed (execvp ENOENT). Bonus: a host path under $HOME is naturally tmpfs-hidden
# inside the view, which is exactly the ENOENT we assert.
CXWORK=$(mktemp -d "$HOME/.p6-session-XXXXXX")
RT="$CXWORK/rt"; mkdir -p "$RT/bin"
FAKE_CODEX="$RT/bin/codex"
STUB_HOME="$CXWORK/codexhome"; mkdir -p "$STUB_HOME"
printf '{"tokens":{"access_token":"SECRET-CANARY-DO-NOT-LEAK"}}' > "$STUB_HOME/auth.json"; chmod 600 "$STUB_HOME/auth.json"
# A real host "vault" under $HOME — must be INVISIBLE (ENOENT) inside the sterile view. Its absolute path
# is baked into the fake so the probe targets a genuine host path (hidden by the /home tmpfs).
HOST_VAULT="$CXWORK/host-vault-secret"
echo "HOST-VAULT-SENTINEL" > "$HOST_VAULT"
SESS_BASE="$CXWORK/sessions"; mkdir -p "$SESS_BASE"

cat > "$FAKE_CODEX" <<EOS
#!/usr/bin/env bash
# Runs INSIDE the bwrap sterile view. Records argv to the bound sessions dir (rw, host-visible), probes the
# view (real host vault ENOENT? auth.json writable?), and emits a canned final message to the -o scratch.
# On a fresh 'exec' it also writes a codex-shaped rollout so the broker can capture a native session id.
sess="\$CODEX_HOME/sessions"
mkdir -p "\$sess"
n=\$(( \$(cat "\$sess/n" 2>/dev/null || echo 0) + 1 )); echo "\$n" > "\$sess/n"
printf '%s\0' "\$@" > "\$sess/argv-\$n"
{ ls "$HOST_VAULT" 2>&1 | head -1; } > "\$sess/vault-probe-\$n"
if echo x 2>/dev/null >> "\$CODEX_HOME/auth.json"; then echo WRITABLE > "\$sess/auth-probe-\$n"; else echo RO > "\$sess/auth-probe-\$n"; fi
out=""; mode="exec"; prev=""
for a in "\$@"; do
  [ "\$prev" = "-o" ] && out="\$a"
  [ "\$a" = "resume" ] && mode="resume"
  prev="\$a"
done
if [ "\$mode" = "exec" ]; then
  day="\$sess/2026/08/20"; mkdir -p "\$day"
  id="aaaaaaaa-bbbb-4ccc-8ddd-000000000001"
  printf '{"type":"session_meta","payload":{"session_id":"%s","id":"%s"}}\n' "\$id" "\$id" > "\$day/rollout-2026-08-20T00-00-00-\$id.jsonl"
fi
[ -n "\$out" ] && printf 'pong' > "\$out"
exit 0
EOS
chmod +x "$FAKE_CODEX"

SHREK_CODEX_BIN="$FAKE_CODEX" \
SHREK_CODEX_RUNTIME_DIR="$RT" \
SHREK_CODEX_HOME="$STUB_HOME" \
SHREK_CODEX_SESSION_DIR="$SESS_BASE" \
SHREK_CODEX_BROKER_LISTEN="127.0.0.1:$CODEX_PORT" \
SHREK_CODEX_STATE_DIR="$CXWORK/codex-state" \
  "$CODEX_BROKER" serve >"$WORK/codex-broker.log" 2>&1 &
for _ in $(seq 1 50); do curl -s -m1 "http://127.0.0.1:$CODEX_PORT/" >/dev/null 2>&1 && break; sleep 0.1; done

# Turn 1 (create).
RC1=$(post "$CODEX_PORT" "cx-AAA" '{"model":"gpt-5.5","system":"SYS","messages":[{"role":"user","content":"CX-UNIQUE1"}]}')
echo "$RC1" | grep -q '"text":"pong"' && pass "S4 turn-1 reply wrapped (pong)" || fail "S4 turn-1 no wrapped reply: $RC1 (see codex-broker.log)"
# Locate the single per-session dir the broker created (bound rw AS $CODEX_HOME/sessions, so the fake's
# records land directly under it).
SDIR=$(find "$SESS_BASE" -mindepth 1 -maxdepth 1 -type d | head -1)
[ -n "$SDIR" ] && pass "S4 broker created a per-session state dir" || fail "S4 no per-session dir under $SESS_BASE"
load_args "$SDIR/argv-1"
if has_exact "--ephemeral"; then fail "S4 sessioned create still passed --ephemeral (would break resume)"; else pass "S4 create dropped --ephemeral (rollout persists)"; fi
has_exact "--disable" && pass "S4 reader-disable present on create" || fail "S4 create missing reader-disable"

# Turn 2 (resume): prior + assistant + new tail.
RC2=$(post "$CODEX_PORT" "cx-AAA" '{"model":"gpt-5.5","system":"SYS","messages":[{"role":"user","content":"CX-UNIQUE1"},{"role":"assistant","content":"CX-ASSIST1"},{"role":"user","content":"CX-TAIL2"}]}')
echo "$RC2" | grep -q '"text":"pong"' && pass "S4 turn-2 reply wrapped" || fail "S4 turn-2 no wrapped reply: $RC2"
load_args "$SDIR/argv-2"
has_exact "resume" && pass "S4 turn-2 used codex exec resume" || fail "S4 turn-2 not a resume"
RID=$(arg_after resume) || true
[ "${RID:-}" = "aaaaaaaa-bbbb-4ccc-8ddd-000000000001" ] && pass "S4 resumed the CAPTURED native id ($RID)" || fail "S4 resume id not the captured one: ${RID:-}"
# Delta-only: the resume prompt arg is the token right after the id.
PC2=""; for ((i=0;i<${#ARGS[@]};i++)); do [ "${ARGS[i]}" = "resume" ] && PC2="${ARGS[i+2]}"; done
echo "$PC2" | grep -q "CX-TAIL2" && pass "S4 turn-2 forwarded the new tail" || fail "S4 turn-2 missing tail: $PC2"
if echo "$PC2" | grep -q "CX-UNIQUE1"; then fail "S4 turn-2 RE-SENT the prior turn"; else pass "S4 turn-2 delta-only: prior turn ABSENT"; fi

# S5: sterile view held DURING the resumed call.
VP=$(cat "$SDIR/vault-probe-2" 2>/dev/null)
echo "$VP" | grep -qiE 'no such file|not found|cannot access' && pass "S5 host vault ENOENT inside the resumed view" || fail "S5 vault probe unexpectedly saw: $VP"
AP=$(cat "$SDIR/auth-probe-2" 2>/dev/null)
[ "$AP" = "RO" ] && pass "S5 auth.json is RO on the resume call (write refused)" || fail "S5 auth.json probe: $AP (expected RO)"
if echo "$RC2" | grep -q "SECRET-CANARY"; then fail "S5 auth canary leaked into the reply"; else pass "S5 auth canary absent from the resumed reply"; fi

pkill -x codex-broker 2>/dev/null

echo
if [ "$fails" -eq 0 ]; then echo "SESSION-PROOF: ALL PASS"; else echo "SESSION-PROOF: $fails FAILED"; fi
exit $(( fails > 0 ? 1 : 0 ))
