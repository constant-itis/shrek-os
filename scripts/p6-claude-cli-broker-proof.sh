#!/usr/bin/env bash
# Phase-6 slice-4 — the SUBSCRIPTION-model provider via the broker-side Claude-CLI broker
# (docs/phase6-slice4-claude-cli-broker.md). Sibling of p6-anthropic-proxy-proof.sh: SAME sealed coder
# binary, SAME `shrek run` front door, SAME T2 wall, SAME plaintext messages-API wire — only the broker
# BEHIND the sealed egress changes. Here the box speaks the messages API in PLAINTEXT to `crates/claude-
# broker` (`shrek-claude-cli`, sealed `model-claude-cli` egress); the broker TRANSLATES that request into
# an invocation of the LOGGED-IN `claude` CLI (`claude -p --output-format json`) BROKER-SIDE and wraps the
# reply back into messages shape. "Sign in with Claude" = log into the CLI once, Shrek shells it. Shrek
# handles NO OAuth credential (the CLI owns its own login); the box never holds a secret and never even
# has a `claude` binary — it reaches ONLY the broker.
#
# The "model" is a DETERMINISTIC FAKE `claude` on the broker host: it reads `-p <prompt>`, derives the
# step from the transcript (# of `Assistant:` turns), and emits a fixed tool-call sequence in
# `claude --output-format json` shape. It uses NO network and NO credential. A real `claude -p` smoke is
# a LATER, opt-in, quota-spending run (SHREK_CLAUDE_LIVE=1, host-side) — NOT this deterministic gate.
#
# What only a real run can show (unit tests cover the pure translation pieces):
#   C1  authentic harness ⇒ derived=T-untrust, construct-at=T2, egress=model-claude-cli; coder announces
#       provider=anthropic and its model-url points at the broker (NOT the api-key proxy).
#   C2  the AGENT LOOP ran driven by the messages→CLI translation: CODER-STEP ≥1 + write_file + run.
#   C3  real build/test: tcc compiled the edited source, the ELF ran, marker+exit == pass, done ok=true.
#   C4  write-through: the fixed source + compiled ELF are visible ON THE HOST afterward.
#   C5  NO credential AND NO `claude` in the box: in-sandbox `env` has no ANTHROPIC/CLAUDE secret and
#       `claude` is not even on PATH; the CLI ran BROKER-SIDE (broker + fake-claude logs prove it).
#   C6  the wall holds during a subscription session: the box reaches ONLY the broker — a non-broker port
#       on the broker host and a non-model dst (1.1.1.1:53) are DROPPED; vault ENOENT; host sentinel absent.
set -uo pipefail

PIN_SHA256="670bcd3cbc103f00d8bb5098edc370f32397ee4c134231436bafa659bb3c068e"
PIN_URL="https://storage.googleapis.com/gvisor/releases/release/20260810.0/x86_64/runsc"

if [ "${IN_CONTAINER:-0}" != "1" ]; then
  REPO_ROOT="$(git rev-parse --show-toplevel)"
  echo "=== building gatekeeperd --features spike + shrek + coder + claude-broker (host) ==="
  ( cd "$REPO_ROOT" && cargo build --release -p gatekeeperd --features spike && \
    cargo build --release -p shrek && cargo build --release -p coder && \
    cargo build --release -p claude-broker ) || exit 3

  # --- OPT-IN LIVE SMOKE (host-side, non-gating). Runs the REAL logged-in `claude` behind the broker and
  #     confirms a genuine reply flows back through the SAME messages-API seam. Spends subscription quota;
  #     needs the host's claude login. NOT part of the deterministic gate — enable with SHREK_CLAUDE_LIVE=1. ---
  if [ "${SHREK_CLAUDE_LIVE:-0}" = "1" ]; then
    echo "=== LIVE SMOKE: real \`claude -p\` behind the broker (host-side, spends quota) ==="
    BROKER_BIN="$REPO_ROOT/target/release/claude-broker"
    SHREK_CLAUDE_BROKER_LISTEN=127.0.0.1:8399 SHREK_CLAUDE_BIN="${SHREK_CLAUDE_BIN:-claude}" \
      SHREK_CLAUDE_DEFAULT_MODEL="${SHREK_CLAUDE_DEFAULT_MODEL:-haiku}" "$BROKER_BIN" > /tmp/live-broker.log 2>&1 &
    LIVE_PID=$!; sleep 0.6
    REQ='{"model":"claude-haiku-4-5","max_tokens":64,"system":"Reply with exactly the single word: pong.","messages":[{"role":"user","content":"ping"}]}'
    LIVE_RESP=$(printf 'POST /v1/messages HTTP/1.1\r\nHost: shrek-claude-cli\r\nContent-Length: %d\r\nConnection: close\r\n\r\n%s' "${#REQ}" "$REQ" | timeout 90 ncat 127.0.0.1 8399 2>/dev/null || true)
    kill "$LIVE_PID" 2>/dev/null
    echo "--- live broker log ---"; sed 's/^/    /' /tmp/live-broker.log
    if echo "$LIVE_RESP" | grep -q '"content"' && echo "$LIVE_RESP" | grep -qi 'pong'; then
      echo "  LIVE-SMOKE PASS ✅ real claude reply came back through the broker seam"
    else
      echo "  LIVE-SMOKE FAIL ❌ (no wrapped reply — check login / quota). Response tail:"; echo "$LIVE_RESP" | tail -3 | sed 's/^/    /'
    fi
    echo "=== (continuing to the deterministic gate) ==="
  fi

  CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/shrek"; mkdir -p "$CACHE"
  RUNSC="$CACHE/runsc-20260810.0"
  if [ ! -f "$RUNSC" ] || [ "$(sha256sum "$RUNSC" | awk '{print $1}')" != "$PIN_SHA256" ]; then
    echo "=== fetching pinned runsc (release-20260810.0) ==="
    curl -fsSL -m 300 -o "$RUNSC" "$PIN_URL" || { echo "runsc fetch failed"; exit 3; }
  fi
  [ "$(sha256sum "$RUNSC" | awk '{print $1}')" = "$PIN_SHA256" ] || { echo "PIN MISMATCH"; exit 3; }
  chmod +x "$RUNSC"

  # Default bridge (never --network host, #2651): the sandbox→eth0 forward stays INSIDE the container
  # netns (plane policy accept), so it never crosses the host FORWARD DROP. No host firewall mutation.
  echo "=== launching privileged debian:trixie oracle ==="
  exec docker run --rm --privileged --cgroupns=private \
    -e IN_CONTAINER=1 \
    -v "$REPO_ROOT/scripts/p6-claude-cli-broker-proof.sh:/proof.sh:ro" \
    -v "$REPO_ROOT/target/release/gatekeeperd:/gatekeeperd:ro" \
    -v "$REPO_ROOT/target/release/shrek:/shrek:ro" \
    -v "$REPO_ROOT/target/release/coder:/coder-src:ro" \
    -v "$REPO_ROOT/target/release/claude-broker:/claude-broker:ro" \
    -v "$REPO_ROOT/tests/fixtures/coder-task/buggy.c:/fixture-buggy.c:ro" \
    -v "$RUNSC:/runsc-src:ro" \
    debian:trixie bash /proof.sh
fi

# ---------------- inside the container ----------------
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
# python3 = the FAKE claude CLI (canned --output-format json); the rest as in the sibling proof. The
# coder + claude-broker are glibc-dynamic; their ldd closures are sealed/available. No openssl/TLS here.
apt-get install -y --no-install-recommends busybox-static systemd ca-certificates e2fsprogs \
  iproute2 nftables ncat tcc python3 >/dev/null 2>&1
mount --make-rshared / 2>/dev/null || true
mount -t tmpfs tmpfs /run 2>/dev/null || true
echo 1 > /proc/sys/net/ipv4/ip_forward 2>/dev/null || true

pass(){ echo "  PASS $*"; }
fail(){ echo "  FAIL $*"; fails=$((fails+1)); }
fails=0

# --- Genuine fs-verity harness (same anchor as the sibling proofs). ---
IMG=/tmp/harness-verity.img; MNT=/mnt/harness; mkdir -p "$MNT"
dd if=/dev/zero of="$IMG" bs=1M count=256 status=none
mkfs.ext4 -q -b 4096 -O verity "$IMG" || { echo "FAIL provision: mkfs.ext4 -O verity unavailable"; exit 2; }
for i in $(seq 0 63); do [ -e /dev/loop$i ] || mknod -m660 /dev/loop$i b 7 "$i"; done
LOOP=$(losetup -f --show "$IMG") || { echo "FAIL provision: losetup"; exit 2; }
mount -o exec "$LOOP" "$MNT" || { echo "FAIL provision: mount verity fs"; exit 2; }
cp /runsc-src "$MNT/runsc"; chmod +x "$MNT/runsc"
/gatekeeperd pin-verity enable "$MNT/runsc" || { echo "FAIL provision: enable-verity on runsc"; exit 2; }
DL=$(/gatekeeperd pin-verity measure "$MNT/runsc") || { echo "FAIL provision: measure runsc"; exit 2; }
ALGO=${DL%% *}; HEX=${DL##* }; RUNSC="$MNT/runsc"
ADMIT_OK=/tmp/ingest-admit.ok
printf 'shrek-t2-ingest-admit v1\n# authorised T2 untrusted-ingest harness\n%s %s\n' "$ALGO" "$HEX" > "$ADMIT_OK"
echo "provisioned genuine fs-verity harness: runsc = $ALGO $HEX"

# --- Sealed T2 rootfs: busybox + tcc + tcc closure + THE CODER BINARY + its ldd closure (as sibling).
#     DELIBERATELY NO `claude` binary in the box — the CLI runs BROKER-SIDE only. ---
ROOTFS=/rootfs; rm -rf "$ROOTFS"; mkdir -p "$ROOTFS/bin"
cp /usr/bin/busybox "$ROOTFS/bin/busybox" 2>/dev/null || cp /bin/busybox "$ROOTFS/bin/busybox"
for a in sh cat ls nc timeout echo test cp chmod env grep command; do ln -sf busybox "$ROOTFS/bin/$a"; done
install -D -m0755 /usr/bin/tcc "$ROOTFS/usr/bin/tcc"
[ -e /lib64/ld-linux-x86-64.so.2 ] && install -D -m0755 /lib64/ld-linux-x86-64.so.2 "$ROOTFS/lib64/ld-linux-x86-64.so.2"
for so in $(ldd /usr/bin/tcc | grep -oE '/[^ ]+\.so[^ ]*'); do install -D -m0755 "$so" "$ROOTFS$so"; done
install -D -m0755 /coder-src "$ROOTFS/usr/bin/coder"
for so in $(ldd /coder-src | grep -oE '/[^ ]+\.so[^ ]*'); do install -D -m0755 "$so" "$ROOTFS$so"; done
mount --bind "$ROOTFS" "$ROOTFS" && mount -o remount,ro,bind "$ROOTFS" || echo "WARN: rootfs not remounted ro (less faithful)"

# --- Anchor + writable project + build area + ungranted vault + host sentinel. ---
rm -rf /srv; mkdir -p /srv/project /srv/build /srv/vault
cp /fixture-buggy.c /srv/project/buggy.c
echo "pre-existing-project-file" > /srv/project/README
echo "TOP-SECRET-DO-NOT-LEAK" > /srv/vault/secret
echo "HOST-ONLY" > /etc/shrek-host-sentinel

# --- The FIXED reference source the (fake) model tells the agent to write. ---
cat > /tmp/fixed.c <<'CEOF'
static long s(long n, long a, long b, long c) {
    long r;
    __asm__ volatile("syscall" : "=a"(r) : "a"(n), "D"(a), "S"(b), "d"(c) : "rcx", "r11", "memory");
    return r;
}
void _start(void) {
    const char m[] = "REAL-COMPILE-RUN-OK";
    s(1, 1, (long)m, sizeof(m) - 1);
    s(60, 42, 0, 0);
}
CEOF

# --- The FAKE `claude` CLI (broker-side). Reads `-p <prompt>`, derives the step from the transcript
#     (# of `Assistant:` turns already present), emits the next tool-call in `claude --output-format json`
#     shape. Uses NO network, NO credential. Logs each invocation → proves the CLI ran BROKER-SIDE. ---
cat > /tmp/fake-claude <<'PYEOF'
#!/usr/bin/env python3
import sys, json
argv = sys.argv[1:]
prompt = ""
for i, a in enumerate(argv):
    if a == "-p" and i + 1 < len(argv):
        prompt = argv[i + 1]
step = prompt.count("Assistant:")  # 0 before any assistant turn, then 1, 2, ...
with open('/tmp/fake-claude.log', 'a') as f:
    f.write("FAKE-CLAUDE-INVOKED step=%d promptB=%d argc=%d\n" % (step, len(prompt), len(argv)))
FIXED = open('/tmp/fixed.c').read()
COMPILE = ("PATH=/usr/bin:/bin; export PATH; "
           "tcc -nostdlib -static -o /srv/build/prog buggy.c && /srv/build/prog; echo PROG-EXIT=$?")
# WALL, driven by the model: the box must hold NO credential, have NO `claude`, reach ONLY the broker.
WALL = ("PATH=/usr/bin:/bin; export PATH; "
        "if env | grep -qiE 'ANTHROPIC|CLAUDE|sk-ant|OAUTH'; then echo CRED-IN-BOX-BAD; else echo NO-CRED-IN-BOX-OK; fi; "
        "if command -v claude >/dev/null 2>&1; then echo CLAUDE-IN-BOX-BAD; else echo CLAUDE-ABSENT-BOX-OK; fi; "
        "if timeout 3 nc -w2 10.20.0.2 443 </dev/null >/dev/null 2>&1; then echo BROKER-BYPASS-REACHED-BAD; else echo BROKER-BYPASS-BLOCKED-OK; fi; "
        "if timeout 3 nc -w2 1.1.1.1 53 </dev/null >/dev/null 2>&1; then echo NET-1111-REACHED-BAD; else echo NET-1111-BLOCKED-OK; fi; "
        "if [ -e /srv/vault ]; then echo VAULT-VISIBLE-BAD; else echo VAULT-ABSENT-OK; fi; "
        "if [ -e /etc/shrek-host-sentinel ]; then echo HOST-SENTINEL-LEAK-BAD; else echo HOST-SENTINEL-ABSENT-OK; fi")
STEPS = [
    {"tool": "read_file",  "args": {"path": "buggy.c"}},
    {"tool": "write_file", "args": {"path": "buggy.c", "content": FIXED}},
    {"tool": "run",        "args": {"cmd": COMPILE}},
    {"tool": "run",        "args": {"cmd": WALL}},
    {"tool": "done",       "args": {"ok": True, "summary": "rewrote buggy.c; compiled+ran; marker+exit verified"}},
]
i = min(step, len(STEPS) - 1)
tool_json = json.dumps(STEPS[i])
# `claude -p --output-format json` reply shape: the tool-call JSON is the `result` string.
print(json.dumps({"type": "result", "subtype": "success", "is_error": False, "result": tool_json}))
PYEOF
chmod +x /tmp/fake-claude

# --- The local "internet": a server netns holding the CLI broker (10.20.0.2:8300, plaintext). The broker
#     shells the FAKE claude broker-side (in THIS netns), never in the box. `shrek-claude-cli` → 10.20.0.2
#     so the box reaches the broker; there is NO api.anthropic.com dst — the box never talks to Anthropic. ---
ip netns add srv
ip link add up-egr type veth peer name srv0
ip link set srv0 netns srv
ip addr add 10.20.0.1/24 dev up-egr; ip link set up-egr up
ip -n srv addr add 10.20.0.2/24 dev srv0; ip -n srv link set srv0 up; ip -n srv link set lo up
ip -n srv route add default via 10.20.0.1
printf '10.20.0.2 shrek-claude-cli\n' >> /etc/hosts

# The CLI broker: plaintext in from the box (:8300), shells the fake claude broker-side. NO credential
# anywhere — the (fake) CLI owns "auth". SHREK_CLAUDE_* is NEVER exported into the coder's launch env.
ip netns exec srv env \
  SHREK_CLAUDE_BROKER_LISTEN=10.20.0.2:8300 \
  SHREK_CLAUDE_BIN=/tmp/fake-claude \
  SHREK_CLAUDE_DEFAULT_MODEL=sonnet \
  /claude-broker > /tmp/broker.log 2>&1 &
BROKER_PID=$!
sleep 0.6

# --- cgroup-v2 delegation (same dance as the sibling proofs). ---
if [ -w /sys/fs/cgroup/cgroup.subtree_control ]; then
  mkdir -p /sys/fs/cgroup/init
  for p in $(cat /sys/fs/cgroup/cgroup.procs 2>/dev/null); do echo "$p" > /sys/fs/cgroup/init/cgroup.procs 2>/dev/null || true; done
  echo "+memory +pids" > /sys/fs/cgroup/cgroup.subtree_control 2>/dev/null || true
fi

# Launch EXCLUSIVELY through `shrek run`. SHREK_CLAUDE_* / ANTHROPIC* are deliberately ABSENT — there is
# no secret to hold, and the box must prove it has neither a credential nor a `claude` binary.
runshrek(){ # $1=cgroup-tag  $2=admit-list ; rest = `shrek run` args
  local tag="$1" admit="$2"; shift 2
  local cgp="/sys/fs/cgroup/shrek-run-$tag"; mkdir -p "$cgp" 2>/dev/null || true
  cat > /run/shrek-run.sh <<WRAP
#!/usr/bin/env bash
echo \$\$ > $cgp/cgroup.procs 2>/dev/null || true
exec env SHREK_GATEKEEPERD=/gatekeeperd SHREK_T2_RUNSC=$RUNSC SHREK_T2_ROOTFS=/rootfs \
     SHREK_INGEST_ADMIT=$admit /shrek run "\$@"
WRAP
  chmod +x /run/shrek-run.sh
  /run/shrek-run.sh "$@" 2>&1
}

TASK="Make the program print REAL-COMPILE-RUN-OK and exit 42."

echo
echo "=== CB: subscription-model session solves the task through \`shrek run\` via the CLI broker ==="
# Coder UNCHANGED: reuse --provider anthropic (same messages-API wire) but point --model-url at the CLI
# broker and select the DISTINCT sealed egress model-claude-cli.
OUT=$(runshrek cb "$ADMIT_OK" --project /srv/project --build /srv/build --egress model-claude-cli \
        --trust T-hostile --tier T2 -- /usr/bin/coder --provider anthropic \
        --model-url http://shrek-claude-cli:8300/v1/messages --task "$TASK" --max-steps 8)
echo "$OUT" | sed 's/^/    /'
echo
echo "--- broker log ---"; sed 's/^/    /' /tmp/broker.log
echo "--- fake-claude log ---"; sed 's/^/    /' /tmp/fake-claude.log 2>/dev/null

# C1 — banding + the subscription-model egress name in the decision.
echo "$OUT" | grep -q "mode=ingest-harness derived=T-untrust" && pass "C1 admission ⇒ derived=T-untrust" || fail "C1 derived=T-untrust"
echo "$OUT" | grep -q "construct-at=T2 effective=T2"          && pass "C1 construct-at=T2"              || fail "C1 construct-at=T2"
echo "$OUT" | grep -q "egress=model-claude-cli"               && pass "C1 decision egress=model-claude-cli" || fail "C1 egress name in decision"
# C1 — the coder announced the provider + its model-url points at the BROKER (not the api-key proxy).
echo "$OUT" | grep -q 'CODER-START provider=anthropic'        && pass "C1 coder ran the anthropic wire" || fail "C1 provider=anthropic not selected"
echo "$OUT" | grep -q 'model_url=http://shrek-claude-cli:8300' && pass "C1 model-url points at the CLI broker" || fail "C1 model-url not the broker"
# C2 — the messages→CLI translation drove the loop.
echo "$OUT" | grep -q "CODER-STEP 1"                          && pass "C2 agent loop ran (CODER-STEP)"       || fail "C2 no CODER-STEP"
echo "$OUT" | grep -q 'CODER-TOOL write_file path="buggy.c"'  && pass "C2 translation drove write_file"      || fail "C2 no write_file tool-call"
echo "$OUT" | grep -q "CODER-TOOL run "                        && pass "C2 translation drove run"            || fail "C2 no run tool-call"
# C2 — the broker actually translated + shelled the CLI broker-side.
grep -q "CLAUDE-BROKER-FWD" /tmp/broker.log 2>/dev/null        && pass "C2 broker translated messages→CLI"   || fail "C2 broker never forwarded"
grep -q "CLAUDE-BROKER-CLI-OK" /tmp/broker.log 2>/dev/null     && pass "C2 broker shelled claude + wrapped reply" || fail "C2 broker CLI call never succeeded"
# C3 — real build/test + return.
echo "$OUT" | grep -q "REAL-COMPILE-RUN-OK"                    && pass "C3 compiled ELF RAN (marker present)" || fail "C3 no run marker"
echo "$OUT" | grep -q "PROG-EXIT=42"                           && pass "C3 program exited 42 (pass criterion)"|| fail "C3 wrong/absent exit code"
echo "$OUT" | grep -q "CODER-DONE ok=true"                     && pass "C3 agent returned done ok=true"       || fail "C3 no CODER-DONE ok=true"
# C4 — write-through across both grants.
grep -q "REAL-COMPILE-RUN-OK" /srv/project/buggy.c 2>/dev/null && pass "C4 write-through: fixed source on host project inode" || fail "C4 project not written through"
head -c4 /srv/build/prog 2>/dev/null | grep -qa ELF            && pass "C4 write-through: compiled ELF on host build inode"  || fail "C4 build ELF not written through"
grep -q "pre-existing-project-file" /srv/project/README 2>/dev/null && pass "C4 teardown non-destructive (README intact)" || fail "C4 teardown destroyed project content"
# C5 — NO credential AND NO `claude` in the box; the CLI ran BROKER-SIDE.
echo "$OUT" | grep -q "NO-CRED-IN-BOX-OK"                      && pass "C5 box held NO credential (in-sandbox env clean)" || fail "C5 credential leaked into the box"
echo "$OUT" | grep -q "CLAUDE-ABSENT-BOX-OK"                   && pass "C5 no \`claude\` binary in the box (CLI is broker-side)" || fail "C5 claude present in the box"
grep -q "FAKE-CLAUDE-INVOKED" /tmp/fake-claude.log 2>/dev/null && pass "C5 the CLI ran BROKER-SIDE (fake-claude log has invocations)" || fail "C5 CLI never ran broker-side"
# Anchor leak markers to standalone RESULT lines (the M4/SHREK_GATE:FAIL argv-echo lesson): the WALL cmd's
# own echoed `…echo CRED-IN-BOX-BAD…` is a mid-line false positive; a real leak is the marker ALONE.
echo "$OUT" | grep -qE '^(CRED-IN-BOX-BAD|CLAUDE-IN-BOX-BAD)$' && fail "C5 credential/claude reached the sandbox" || pass "C5 no cred/claude-in-box marker (on result lines)"
# C6 — the wall held; the box reaches ONLY the broker.
echo "$OUT" | grep -q "BROKER-BYPASS-BLOCKED-OK"              && pass "C6 non-broker port on broker host DROPPED (box reaches only :8300)" || fail "C6 box reached a non-broker port"
echo "$OUT" | grep -q "NET-1111-BLOCKED-OK"                    && pass "C6 non-model dst DROPPED (egress sealed to the broker)" || fail "C6 non-model dst reachable"
echo "$OUT" | grep -q "VAULT-ABSENT-OK"                        && pass "C6 ungranted vault ENOENT"            || fail "C6 vault visible"
echo "$OUT" | grep -q "HOST-SENTINEL-ABSENT-OK"                && pass "C6 host sentinel absent"              || fail "C6 host fs leaked"
echo "$OUT" | grep -qE '^(BROKER-BYPASS-REACHED-BAD|NET-1111-REACHED-BAD|VAULT-VISIBLE-BAD|HOST-SENTINEL-LEAK-BAD)$' && fail "C6 LEAK MARKER present" || pass "C6 no leak markers (on result lines)"

# --- teardown + residual check ---
kill "$BROKER_PID" 2>/dev/null
umount "$MNT" 2>/dev/null || true; losetup -d "$LOOP" 2>/dev/null || true
resid_ns=$(ip netns list 2>/dev/null | grep -oE 'shrek-[a-z0-9]+' | tr '\n' ',')
resid_tb=$(nft list tables 2>/dev/null | grep -o 'shrek_egress_[a-z0-9_]*' | tr '\n' ',')
{ [ -z "$resid_ns" ] && [ -z "$resid_tb" ]; } && pass "teardown: no residual sandbox netns / nft table" || fail "teardown residual ns=[$resid_ns] tb=[$resid_tb]"

echo
if [ $fails -eq 0 ]; then echo "P6-CLAUDE-CLI-BROKER-PROOF: ALL PASS ✅"; exit 0
else echo "P6-CLAUDE-CLI-BROKER-PROOF: $fails FAIL ❌"; exit 1; fi
