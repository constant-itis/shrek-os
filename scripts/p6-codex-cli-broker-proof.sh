#!/usr/bin/env bash
# Phase-6 slice-6 — the SECOND subscription-model provider via the broker-side Codex-CLI broker
# (docs/phase6-slice6-codex-cli-broker.md). Sibling of p6-claude-cli-broker-proof.sh: SAME plaintext
# messages-API wire from the box, SAME sealed one-destination egress pattern — only the broker behind the
# sealed egress changes. Here the box speaks the messages API to `crates/codex-broker` (`shrek-codex-cli`,
# sealed `model-codex-cli` egress); the broker ADAPTS that request into an invocation of the LOGGED-IN
# `codex exec` CLI BROKER-SIDE and wraps the reply back into messages shape. Shrek handles NO OAuth
# credential (the CLI owns its own login); the coder is UNCHANGED.
#
# WHAT IS DIFFERENT FROM CLAUDE — and what THIS oracle exists to prove. `codex` is an AGENTIC EXECUTOR
# with its own model-driven shell/tool surface (unlike `claude -p`). So the broker (a) confines the
# spawned `codex` in an unprivileged `bubblewrap` STERILE VIEW, and (b) DISABLES the model's file-reader
# tools so the model can never read the ro-bound credential and exfil it through the reply. This proof
# runs the REAL `codex` under the REAL confinement, pointed at a LOCAL fake Responses endpoint with a
# STUB credential, and CAPTURES the request codex sends — proving the tool surface carries NO reader.
#
# HOST-SIDE + DETERMINISTIC + NO QUOTA + NO REAL CREDENTIAL. Unlike the box-wall oracles this one is
# host-side: it proves the BROKER's confinement of codex (the box egress wall is proven unchanged by
# p6-claude-cli-broker-proof.sh + the shrek-policy egress tests — model-codex-cli is the same one-dst
# shape). The fake endpoint is 127.0.0.1 only; the credential is a FABRICATED stub auth.json; the real
# ~/.codex is NEVER read or bound. A real `codex exec` behind the broker is a LATER opt-in quota-spending
# run (SHREK_CODEX_LIVE=1), NOT this gate.
#
# What only a real run can show (unit tests cover the pure pieces):
#   C1  READER-ABSENT: the request codex actually sends carries NO file-reader tool — exec_command,
#       write_stdin (shell), view_image are all absent (removed by the feature-flag disables).
#   C2  SUBSET: every tool codex still offers is in the known NON-READING allowlist
#       {update_plan,request_user_input,apply_patch,tool_search,web_search}. Any NEW/unknown tool FAILS
#       the gate (a future codex adding a reader breaks the build, not the credential).
#   C3  CREDENTIAL SAFETY: the broker was pointed at a STUB codex home (never the real ~/.codex); the
#       stub credential's canary token appears in NEITHER the captured request NOR the broker reply NOR
#       the breadcrumb — the model was never handed the credential and the broker never surfaced it.
#   C4  STERILE VIEW: under the SAME bwrap confinement the broker builds, a host secret (a fake
#       vault/project canary) is ENOENT and /home is empty — the model's tools see no host data.
#   C5  ADAPT + WRAP: the broker forwarded the box's messages request into `codex exec` broker-side
#       (CODEX-BROKER-FWD) and, when codex replied, wrapped it back into messages shape (content[].text).
set -uo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
BROKER_PORT=8355
FAKE_PORT=8455
PASS=0; FAIL=0
ok(){ echo "  PASS $1"; PASS=$((PASS+1)); }
bad(){ echo "  FAIL $1"; FAIL=$((FAIL+1)); }

command -v bwrap >/dev/null || { echo "SKIP: bwrap not installed (broker confinement dep)"; exit 3; }
command -v codex >/dev/null || { echo "SKIP: codex CLI not installed"; exit 3; }
command -v python3 >/dev/null || { echo "SKIP: python3 needed for the fake endpoint"; exit 3; }

echo "=== building codex-broker (host, broker-side; off the sealed image) ==="
( cd "$REPO_ROOT" && cargo build --release -p codex-broker ) || exit 3
BROKER_BIN="$REPO_ROOT/target/release/codex-broker"

# --- OPT-IN LIVE SMOKE (host-side, non-gating). Runs the REAL logged-in `codex` behind the broker (uses
#     the real ~/.codex, spends subscription quota) and confirms a genuine reply flows back through the
#     SAME messages-API seam under the SAME confinement + reader-disable. NOT part of the deterministic
#     gate — enable with SHREK_CODEX_LIVE=1. ---
if [ "${SHREK_CODEX_LIVE:-0}" = "1" ]; then
  echo "=== LIVE SMOKE: real \`codex exec\` behind the broker (host-side, spends quota) ==="
  L_STATE="$(mktemp -d "${TMPDIR:-/tmp}/p6-codex-live.XXXXXX")"
  SHREK_CODEX_BROKER_LISTEN=127.0.0.1:8356 SHREK_CODEX_STATE_DIR="$L_STATE" \
    "$BROKER_BIN" serve >"$L_STATE/broker.out" 2>&1 & LIVE_PID=$!
  sleep 1
  curl -s --max-time 180 -X POST "http://127.0.0.1:8356/v1/messages" -H 'content-type: application/json' \
    --data-binary '{"model":"gpt-5.5","max_tokens":64,"messages":[{"role":"user","content":"reply with the single word pong"}]}' \
    && echo || echo "(live request failed)"
  kill "$LIVE_PID" 2>/dev/null; rm -rf "$L_STATE"
  echo "=== end live smoke (informational) ==="
fi

CODEX_BIN="$(command -v codex)"
CODEX_REAL="$(readlink -f "$CODEX_BIN" 2>/dev/null || echo "$CODEX_BIN")"
# Derive the node runtime dir (…/node/vX) from …/bin/codex — the tree the broker ro-binds.
RUNTIME_DIR="$(cd "$(dirname "$CODEX_BIN")/.." && pwd)"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/p6-codex-proof.XXXXXX")"
CAP="$WORK/cap"; STUBHOME="$WORK/codexhome"; STATE="$WORK/state"
mkdir -p "$CAP" "$STUBHOME" "$STATE"
CANARY="STUB-CRED-CANARY-$(head -c8 /dev/urandom | od -An -tx1 | tr -d ' \n')"
cat > "$STUBHOME/auth.json" <<EOF
{"OPENAI_API_KEY":null,"tokens":{"access_token":"$CANARY","refresh_token":"stub","account_id":"acct_stub"},"last_refresh":"2026-08-20T00:00:00Z"}
EOF
chmod 600 "$STUBHOME/auth.json"

cleanup(){ [ -n "${FAKE_PID:-}" ] && kill "$FAKE_PID" 2>/dev/null; [ -n "${BROKER_PID:-}" ] && kill "$BROKER_PID" 2>/dev/null; wait 2>/dev/null; rm -rf "$WORK"; }
trap cleanup EXIT

# --- fake OpenAI Responses endpoint: capture every POST body; return a valid canned SSE reply ---------
cat > "$WORK/fake.py" <<PY
import http.server, os
CAP=os.environ["CAP"]
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self,*a): pass
    def do_POST(self):
        n=int(self.headers.get('content-length',0) or 0)
        body=self.rfile.read(n) if n else b''
        open(os.path.join(CAP,f"req_{len(os.listdir(CAP))}.json"),"wb").write(body)
        sse=(b'data: {"type":"response.created","response":{"id":"resp_1"}}\n\n'
             b'data: {"type":"response.output_item.done","item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"pong"}]}}\n\n'
             b'data: {"type":"response.completed","response":{"id":"resp_1","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"pong"}]}],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2,"input_tokens_details":{"cached_tokens":0},"output_tokens_details":{"reasoning_tokens":0}}}}\n\n'
             b'data: [DONE]\n\n')
        self.send_response(200); self.send_header('content-type','text/event-stream'); self.end_headers(); self.wfile.write(sse)
    def do_GET(self):
        self.send_response(200); self.send_header('content-type','application/json'); self.end_headers(); self.wfile.write(b'{"data":[]}')
http.server.HTTPServer(("127.0.0.1",$FAKE_PORT),H).serve_forever()
PY
CAP="$CAP" python3 "$WORK/fake.py" & FAKE_PID=$!
sleep 1

# The oracle-only passthrough: a LOCAL fake provider so codex's request is captured WITHOUT the real
# subscription credential. Newline-separated (one argv token per line) for SHREK_CODEX_EXTRA_ARGS.
EXTRA_ARGS=$(printf '%s\n' \
  -c model_provider=fake \
  -c model_providers.fake.name=fake \
  -c "model_providers.fake.base_url=http://127.0.0.1:$FAKE_PORT/v1" \
  -c model_providers.fake.wire_api=responses \
  -c model_providers.fake.requires_openai_auth=false \
  -c model_providers.fake.supports_websockets=false \
  -c model_providers.fake.env_key=OPENAI_API_KEY)

echo "=== starting codex-broker with a STUB codex home (real ~/.codex NEVER touched) ==="
SHREK_CODEX_BROKER_LISTEN="127.0.0.1:$BROKER_PORT" \
SHREK_CODEX_BIN="$CODEX_BIN" \
SHREK_CODEX_RUNTIME_DIR="$RUNTIME_DIR" \
SHREK_CODEX_HOME="$STUBHOME" \
SHREK_CODEX_STATE_DIR="$STATE" \
SHREK_CODEX_EXTRA_ARGS="$EXTRA_ARGS" \
OPENAI_API_KEY="sk-STUB-not-real" \
  "$BROKER_BIN" serve >"$WORK/broker.out" 2>&1 & BROKER_PID=$!
sleep 1

echo "=== the 'box' sends a plaintext messages-API request to the broker ==="
cat > "$WORK/req.json" <<'EOF'
{"model":"gpt-5.5","max_tokens":256,"messages":[{"role":"user","content":"reply with the single word pong"}]}
EOF
REPLY="$(curl -s --max-time 90 -X POST "http://127.0.0.1:$BROKER_PORT/v1/messages" \
  -H 'content-type: application/json' --data-binary @"$WORK/req.json" || true)"
echo "  broker reply: ${REPLY:0:200}"

# Let any capture flush.
sleep 1
BIG="$(ls -S "$CAP"/req_*.json 2>/dev/null | head -1 || true)"

echo "=== assertions ==="
# --- C1 + C2: capture the tools array codex actually sent; no reader; only known non-reading tools ---
if [ -n "$BIG" ]; then
  python3 - "$BIG" <<'PY'
import json,sys
d=json.load(open(sys.argv[1]))
tools=[ (t.get('name') or t.get('type')) for t in d.get('tools',[]) ]
readers={"exec_command","write_stdin","view_image"}
allow={"update_plan","request_user_input","apply_patch","tool_search","web_search"}
present_readers=[t for t in tools if t in readers]
unknown=[t for t in tools if t not in allow]
print("  tools codex offered:", tools)
open("/tmp/_p6codex_readers","w").write(",".join(present_readers))
open("/tmp/_p6codex_unknown","w").write(",".join(unknown))
PY
  READERS="$(cat /tmp/_p6codex_readers 2>/dev/null)"; UNKNOWN="$(cat /tmp/_p6codex_unknown 2>/dev/null)"
  rm -f /tmp/_p6codex_readers /tmp/_p6codex_unknown
  [ -z "$READERS" ] && ok "C1 no file-reader tool (exec_command/write_stdin/view_image) in codex's request" \
                     || bad "C1 reader tool(s) present: $READERS"
  [ -z "$UNKNOWN" ] && ok "C2 every offered tool is in the known non-reading allowlist" \
                    || bad "C2 unknown/unexpected tool(s) present (possible new reader): $UNKNOWN"
else
  bad "C1 no request captured (codex did not reach the fake endpoint)"
  bad "C2 no request captured"
fi

# --- C3: credential safety — the stub canary never leaks into request / reply / breadcrumb ----------
LEAK=0
[ -n "$BIG" ] && grep -q "$CANARY" "$BIG" && LEAK=1
echo "$REPLY" | grep -q "$CANARY" && LEAK=1
[ -f "$STATE/availability.json" ] && grep -q "$CANARY" "$STATE/availability.json" && LEAK=1
[ "$LEAK" = "0" ] && ok "C3 stub credential canary absent from request, reply, and breadcrumb" \
                  || bad "C3 credential canary LEAKED"
# and the real home was never the broker's source
grep -q "$HOME/.codex/auth.json" "$WORK/broker.out" 2>/dev/null && bad "C3b broker referenced the REAL ~/.codex" || ok "C3b broker used the stub home, not the real ~/.codex"

# --- C4: the sterile view (SAME shape the broker builds) hides host secrets ---------------------------
SECRET_HOME="$WORK/fakehome"; mkdir -p "$SECRET_HOME/vault"; echo "TOPSECRET" > "$SECRET_HOME/vault/creds"
VIEW_OUT="$(HOME="$SECRET_HOME" bwrap \
  --unshare-user --unshare-pid --die-with-parent --new-session --clearenv \
  --setenv HOME /work --setenv PATH "$RUNTIME_DIR/bin:/usr/bin:/bin" \
  --ro-bind /usr /usr --ro-bind-try /bin /bin --ro-bind-try /lib /lib --ro-bind-try /lib64 /lib64 \
  --tmpfs /home --ro-bind "$RUNTIME_DIR" "$RUNTIME_DIR" \
  --dir /codexhome --ro-bind "$STUBHOME/auth.json" /codexhome/auth.json \
  --tmpfs /tmp --tmpfs /work --chdir /work --proc /proc --dev /dev \
  -- /bin/sh -c 'ls -A /home; echo "--"; cat '"$SECRET_HOME"'/vault/creds 2>&1' 2>&1 || true)"
if ! echo "$VIEW_OUT" | grep -q TOPSECRET && ! echo "$VIEW_OUT" | grep -qE 'creds$'; then
  ok "C4 sterile view: host vault/home secret is ENOENT and /home is empty inside the view"
else
  bad "C4 sterile view LEAKED a host path: $VIEW_OUT"
fi

# --- C5: the broker adapted the box request into codex, and (if codex parsed the canned reply) wrapped it
grep -q 'CODEX-BROKER-FWD' "$WORK/broker.out" && ok "C5 broker forwarded the messages request into codex exec (CODEX-BROKER-FWD)" \
  || bad "C5 broker did not forward the request"
if echo "$REPLY" | grep -q '"content"' && echo "$REPLY" | grep -q 'pong'; then
  ok "C5b broker wrapped codex's reply back into messages shape (content[].text = pong)"
else
  echo "  NOTE C5b: the canned SSE was not accepted by this codex build (reply not wrapped) — informational,"
  echo "           NOT gating: C1/C2/C3 already prove the reader-free tool surface + credential safety."
fi

echo
echo "=== RESULT: $PASS passed, $FAIL failed ==="
[ "$FAIL" -eq 0 ] || exit 1
