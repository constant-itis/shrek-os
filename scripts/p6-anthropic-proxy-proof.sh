#!/usr/bin/env bash
# Phase-6 slice-3 — the HOSTED-MODEL (Anthropic) provider via the broker-side authenticated egress
# proxy (docs/phase6-slice3-provider-abstraction.md). Sibling of p6-coder-agent-proof.sh: SAME sealed
# coder binary, SAME `shrek run` front door, SAME T2 wall — only the provider changes. The coder speaks
# the Anthropic messages API in PLAINTEXT to the broker proxy (`shrek-model-proxy`, sealed
# `model-anthropic` egress); the proxy injects the API key + terminates TLS to Anthropic. The box never
# holds the secret and never reaches Anthropic directly.
#
# The "model" is a DETERMINISTIC canned Anthropic-shaped HTTPS responder (fixed tool-call sequence). The
# proxy TLS-forwards to it exactly as it would to the real api.anthropic.com (self-signed cert the proxy
# trusts via SHREK_PROXY_EXTRA_CA; SNI=api.anthropic.com). A real Claude smoke is a LATER, non-gating
# opt-in — NOT this deterministic gate.
#
# What only a real run can show (unit tests cover the pure adapter pieces):
#   B1  authentic harness ⇒ derived=T-untrust, construct-at=T2, egress=model-anthropic.
#   B2  the AGENT LOOP ran driven by the MESSAGES-API adapter: CODER-STEP ≥1 + write_file + run.
#   B3  real build/test: tcc compiled the edited source, the ELF ran, marker+exit == pass, done ok=true.
#   B4  write-through: the fixed source + compiled ELF are visible ON THE HOST afterward.
#   B5  the PROXY injected auth (the canned upstream SAW x-api-key) AND the box HELD NO key (in-sandbox
#       `env` has no SHREK_ANTHROPIC_*).
#   B6  the wall holds during a hosted-model session: the box can reach ONLY the proxy — a DIRECT
#       Anthropic dst (proxy-host:443) and a non-model dst (1.1.1.1:53) are DROPPED; vault ENOENT; host
#       sentinel absent.
set -uo pipefail

PIN_SHA256="670bcd3cbc103f00d8bb5098edc370f32397ee4c134231436bafa659bb3c068e"
PIN_URL="https://storage.googleapis.com/gvisor/releases/release/20260810.0/x86_64/runsc"

if [ "${IN_CONTAINER:-0}" != "1" ]; then
  REPO_ROOT="$(git rev-parse --show-toplevel)"
  echo "=== building gatekeeperd --features spike + shrek + coder + model-proxy (host) ==="
  ( cd "$REPO_ROOT" && cargo build --release -p gatekeeperd --features spike && \
    cargo build --release -p shrek && cargo build --release -p coder && \
    cargo build --release -p model-proxy ) || exit 3

  CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/shrek"; mkdir -p "$CACHE"
  RUNSC="$CACHE/runsc-20260810.0"
  if [ ! -f "$RUNSC" ] || [ "$(sha256sum "$RUNSC" | awk '{print $1}')" != "$PIN_SHA256" ]; then
    echo "=== fetching pinned runsc (release-20260810.0) ==="
    curl -fsSL -m 300 -o "$RUNSC" "$PIN_URL" || { echo "runsc fetch failed"; exit 3; }
  fi
  [ "$(sha256sum "$RUNSC" | awk '{print $1}')" = "$PIN_SHA256" ] || { echo "PIN MISMATCH"; exit 3; }
  chmod +x "$RUNSC"

  # Default bridge (never --network host): the sandbox→eth0 forward stays INSIDE the container netns
  # (plane policy accept), so it never crosses the host FORWARD DROP (#2651). No host firewall mutation.
  echo "=== launching privileged debian:trixie oracle ==="
  exec docker run --rm --privileged --cgroupns=private \
    -e IN_CONTAINER=1 \
    -v "$REPO_ROOT/scripts/p6-anthropic-proxy-proof.sh:/proof.sh:ro" \
    -v "$REPO_ROOT/target/release/gatekeeperd:/gatekeeperd:ro" \
    -v "$REPO_ROOT/target/release/shrek:/shrek:ro" \
    -v "$REPO_ROOT/target/release/coder:/coder-src:ro" \
    -v "$REPO_ROOT/target/release/model-proxy:/model-proxy:ro" \
    -v "$REPO_ROOT/tests/fixtures/coder-task/buggy.c:/fixture-buggy.c:ro" \
    -v "$RUNSC:/runsc-src:ro" \
    debian:trixie bash /proof.sh
fi

# ---------------- inside the container ----------------
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
# python3 = the canned Anthropic responder; openssl = its self-signed TLS cert; the rest as in the
# sibling proof. The coder + model-proxy are glibc-dynamic; their ldd closures are sealed/available.
apt-get install -y --no-install-recommends busybox-static systemd ca-certificates e2fsprogs \
  iproute2 nftables ncat tcc python3 openssl >/dev/null 2>&1
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

# --- Sealed T2 rootfs: busybox + tcc + tcc closure + THE CODER BINARY + its ldd closure (as sibling). ---
ROOTFS=/rootfs; rm -rf "$ROOTFS"; mkdir -p "$ROOTFS/bin"
cp /usr/bin/busybox "$ROOTFS/bin/busybox" 2>/dev/null || cp /bin/busybox "$ROOTFS/bin/busybox"
for a in sh cat ls nc timeout echo test cp chmod env grep; do ln -sf busybox "$ROOTFS/bin/$a"; done
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

# --- The FIXED reference source the canned model tells the agent to write. ---
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

# --- The BROKER-SIDE key file. It lives ONLY here, is passed ONLY to the proxy (never the coder), and
#     never enters the sandbox. A dummy value for the deterministic gate. ---
BROKER_KEY=/tmp/broker/anthropic.key
mkdir -p /tmp/broker; printf 'sk-ant-CANNED-broker-side-only-0000' > "$BROKER_KEY"; chmod 600 "$BROKER_KEY"

# --- The local "internet": a server netns holding BOTH the broker proxy (10.20.0.2:8200, plaintext) and
#     the canned Anthropic HTTPS responder (10.20.0.2:8443, TLS). `shrek-model-proxy` → 10.20.0.2 so the
#     box reaches the proxy; `api.anthropic.com` → 10.20.0.2 so the PROXY's TLS lands on the canned api. ---
ip netns add srv
ip link add up-egr type veth peer name srv0
ip link set srv0 netns srv
ip addr add 10.20.0.1/24 dev up-egr; ip link set up-egr up
ip -n srv addr add 10.20.0.2/24 dev srv0; ip -n srv link set srv0 up; ip -n srv link set lo up
ip -n srv route add default via 10.20.0.1
printf '10.20.0.2 shrek-model-proxy api.anthropic.com\n' >> /etc/hosts

# Test PKI for the canned api: a CA (trust anchor, CA:TRUE) that signs a LEAF (CA:FALSE,
# SAN=api.anthropic.com). The responder presents the LEAF; the proxy trusts the CA via
# SHREK_PROXY_EXTRA_CA. rustls (webpki) rejects a CA cert presented AS the end-entity
# (CaUsedAsEndEntity), so the leaf must be a proper non-CA cert — exactly like a real server cert.
openssl req -x509 -newkey rsa:2048 -nodes -keyout /tmp/ca.key -out /tmp/ca.crt -days 1 \
  -subj "/CN=Shrek Canned Test CA" -addext "basicConstraints=critical,CA:TRUE" >/dev/null 2>&1
openssl req -newkey rsa:2048 -nodes -keyout /tmp/canned.key -out /tmp/canned.csr \
  -subj "/CN=api.anthropic.com" >/dev/null 2>&1
printf 'subjectAltName=DNS:api.anthropic.com\nbasicConstraints=critical,CA:FALSE\n' > /tmp/leaf.ext
openssl x509 -req -in /tmp/canned.csr -CA /tmp/ca.crt -CAkey /tmp/ca.key -CAcreateserial \
  -out /tmp/canned.crt -days 1 -extfile /tmp/leaf.ext >/dev/null 2>&1

# The canned Anthropic messages-API responder: fixed tool-call sequence, in Anthropic reply shape. It
# VERIFIES the injected x-api-key (logs X-API-KEY-SEEN) — proving the proxy authenticated the call.
cat > /tmp/anthropic_canned.py <<'PYEOF'
import http.server, ssl, json
FIXED = open('/tmp/fixed.c').read()
COMPILE = ("PATH=/usr/bin:/bin; export PATH; "
           "tcc -nostdlib -static -o /srv/build/prog buggy.c && /srv/build/prog; echo PROG-EXIT=$?")
# WALL, driven by the model: the box must hold NO key and reach ONLY the proxy.
WALL = ("PATH=/usr/bin:/bin; export PATH; "
        "if env | grep -q SHREK_ANTHROPIC; then echo KEY-IN-BOX-BAD; else echo NO-KEY-IN-BOX-OK; fi; "
        "if timeout 3 nc -w2 10.20.0.2 443 </dev/null >/dev/null 2>&1; then echo DIRECT-ANTHROPIC-REACHED-BAD; else echo DIRECT-ANTHROPIC-BLOCKED-OK; fi; "
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
class H(http.server.BaseHTTPRequestHandler):
    step = 0
    def log_message(self, *a): pass
    def do_POST(self):
        n = int(self.headers.get('Content-Length', 0))
        self.rfile.read(n)
        key = self.headers.get('x-api-key', '')
        ver = self.headers.get('anthropic-version', '')
        # PROOF: the proxy injected auth. Print to the log the oracle greps (never the value).
        if key:
            print("X-API-KEY-SEEN len=%d anthropic-version=%s" % (len(key), ver), flush=True)
        else:
            print("X-API-KEY-MISSING", flush=True)
        i = min(H.step, len(STEPS) - 1); H.step += 1
        tool_json = json.dumps(STEPS[i])
        # Anthropic messages-API reply shape: the tool-call JSON is the text of a single content block.
        body = json.dumps({
            "id": "msg_canned", "type": "message", "role": "assistant",
            "content": [{"type": "text", "text": tool_json}],
            "stop_reason": "end_turn",
        }).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
httpd = http.server.HTTPServer(('10.20.0.2', 8443), H)
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.load_cert_chain('/tmp/canned.crt', '/tmp/canned.key')
httpd.socket = ctx.wrap_socket(httpd.socket, server_side=True)
httpd.serve_forever()
PYEOF
ip netns exec srv python3 /tmp/anthropic_canned.py > /tmp/canned.log 2>&1 &
CANNED_PID=$!

# The broker proxy: plaintext in from the box (:8200), TLS out to the canned api. The KEY FILE is the
# proxy's alone; SHREK_ANTHROPIC_* is NEVER exported into the coder's launch env.
ip netns exec srv env \
  SHREK_PROXY_LISTEN=10.20.0.2:8200 \
  SHREK_PROXY_UPSTREAM=api.anthropic.com:8443 \
  SHREK_ANTHROPIC_KEY_FILE="$BROKER_KEY" \
  SHREK_PROXY_EXTRA_CA=/tmp/ca.crt \
  /model-proxy > /tmp/proxy.log 2>&1 &
PROXY_PID=$!
sleep 0.8

# --- cgroup-v2 delegation (same dance as the sibling proofs). ---
if [ -w /sys/fs/cgroup/cgroup.subtree_control ]; then
  mkdir -p /sys/fs/cgroup/init
  for p in $(cat /sys/fs/cgroup/cgroup.procs 2>/dev/null); do echo "$p" > /sys/fs/cgroup/init/cgroup.procs 2>/dev/null || true; done
  echo "+memory +pids" > /sys/fs/cgroup/cgroup.subtree_control 2>/dev/null || true
fi

# Launch EXCLUSIVELY through `shrek run` (the shipped front door). NOTE: SHREK_ANTHROPIC_* is deliberately
# ABSENT from this env — the secret is the proxy's alone; the box must prove it holds none.
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
echo "=== CB: hosted-model (Anthropic) session solves the task through \`shrek run\` via the proxy ==="
OUT=$(runshrek cb "$ADMIT_OK" --project /srv/project --build /srv/build --egress model-anthropic \
        --trust T-hostile --tier T2 -- /usr/bin/coder --provider anthropic --task "$TASK" --max-steps 8)
echo "$OUT" | sed 's/^/    /'
echo
echo "--- proxy log ---"; sed 's/^/    /' /tmp/proxy.log
echo "--- canned api log ---"; sed 's/^/    /' /tmp/canned.log

# B1 — banding + the hosted-model egress name in the decision.
echo "$OUT" | grep -q "mode=ingest-harness derived=T-untrust" && pass "B1 admission ⇒ derived=T-untrust" || fail "B1 derived=T-untrust"
echo "$OUT" | grep -q "construct-at=T2 effective=T2"          && pass "B1 construct-at=T2"              || fail "B1 construct-at=T2"
echo "$OUT" | grep -q "egress=model-anthropic"                && pass "B1 decision egress=model-anthropic" || fail "B1 egress name in decision"
# B1 — the coder announced the anthropic provider + its sealed egress contract.
echo "$OUT" | grep -q 'CODER-START provider=anthropic egress=model-anthropic' && pass "B1 coder ran the anthropic provider" || fail "B1 provider=anthropic not selected"
# B2 — the messages-API adapter drove the loop.
echo "$OUT" | grep -q "CODER-STEP 1"                          && pass "B2 agent loop ran (CODER-STEP)"       || fail "B2 no CODER-STEP"
echo "$OUT" | grep -q 'CODER-TOOL write_file path="buggy.c"'  && pass "B2 messages-API adapter drove write_file" || fail "B2 no write_file tool-call"
echo "$OUT" | grep -q "CODER-TOOL run "                        && pass "B2 messages-API adapter drove run"     || fail "B2 no run tool-call"
# B3 — real build/test + return.
echo "$OUT" | grep -q "REAL-COMPILE-RUN-OK"                    && pass "B3 compiled ELF RAN (marker present)" || fail "B3 no run marker"
echo "$OUT" | grep -q "PROG-EXIT=42"                           && pass "B3 program exited 42 (pass criterion)"|| fail "B3 wrong/absent exit code"
echo "$OUT" | grep -q "CODER-DONE ok=true"                     && pass "B3 agent returned done ok=true"       || fail "B3 no CODER-DONE ok=true"
# B4 — write-through across both grants.
grep -q "REAL-COMPILE-RUN-OK" /srv/project/buggy.c 2>/dev/null && pass "B4 write-through: fixed source on host project inode" || fail "B4 project not written through"
head -c4 /srv/build/prog 2>/dev/null | grep -qa ELF            && pass "B4 write-through: compiled ELF on host build inode"  || fail "B4 build ELF not written through"
grep -q "pre-existing-project-file" /srv/project/README 2>/dev/null && pass "B4 teardown non-destructive (README intact)" || fail "B4 teardown destroyed project content"
# B5 — the PROXY injected auth (canned upstream SAW the key) AND the box HELD NO key.
grep -q "X-API-KEY-SEEN" /tmp/canned.log 2>/dev/null           && pass "B5 proxy injected auth (canned upstream saw x-api-key)" || fail "B5 upstream never saw x-api-key"
grep -q "X-API-KEY-MISSING" /tmp/canned.log 2>/dev/null        && fail "B5 an upstream call arrived WITHOUT the key" || pass "B5 every upstream call was authenticated"
echo "$OUT" | grep -q "NO-KEY-IN-BOX-OK"                        && pass "B5 box held NO secret (in-sandbox env has no SHREK_ANTHROPIC_*)" || fail "B5 key leaked into the box"
# Anchor to a standalone RESULT line: the WALL cmd's own echoed `…echo KEY-IN-BOX-BAD…` is an argv-echo
# false positive mid-line (the M4/SHREK_GATE:FAIL lesson) — a real leak is the marker ALONE on a line.
echo "$OUT" | grep -qE '^KEY-IN-BOX-BAD$'                       && fail "B5 KEY-IN-BOX (secret reached the sandbox)" || pass "B5 no key-in-box marker (on result lines)"
# B6 — the wall held; the box reaches ONLY the proxy.
echo "$OUT" | grep -q "DIRECT-ANTHROPIC-BLOCKED-OK"            && pass "B6 direct Anthropic dst DROPPED (box reaches only the proxy)" || fail "B6 box reached Anthropic directly"
echo "$OUT" | grep -q "NET-1111-BLOCKED-OK"                    && pass "B6 non-model dst DROPPED (egress sealed to the proxy)" || fail "B6 non-model dst reachable"
echo "$OUT" | grep -q "VAULT-ABSENT-OK"                        && pass "B6 ungranted vault ENOENT"            || fail "B6 vault visible"
echo "$OUT" | grep -q "HOST-SENTINEL-ABSENT-OK"                && pass "B6 host sentinel absent"              || fail "B6 host fs leaked"
# Anchor leak markers to standalone RESULT lines (the M4 argv-echo lesson).
echo "$OUT" | grep -qE '^(KEY-IN-BOX-BAD|DIRECT-ANTHROPIC-REACHED-BAD|NET-1111-REACHED-BAD|VAULT-VISIBLE-BAD|HOST-SENTINEL-LEAK-BAD)$' && fail "B6 LEAK MARKER present" || pass "B6 no leak markers (on result lines)"

# --- teardown + residual check ---
kill "$CANNED_PID" "$PROXY_PID" 2>/dev/null
umount "$MNT" 2>/dev/null || true; losetup -d "$LOOP" 2>/dev/null || true
resid_ns=$(ip netns list 2>/dev/null | grep -oE 'shrek-[a-z0-9]+' | tr '\n' ',')
resid_tb=$(nft list tables 2>/dev/null | grep -o 'shrek_egress_[a-z0-9_]*' | tr '\n' ',')
{ [ -z "$resid_ns" ] && [ -z "$resid_tb" ]; } && pass "teardown: no residual sandbox netns / nft table" || fail "teardown residual ns=[$resid_ns] tb=[$resid_tb]"

echo
if [ $fails -eq 0 ]; then echo "P6-ANTHROPIC-PROXY-PROOF: ALL PASS ✅"; exit 0
else echo "P6-ANTHROPIC-PROXY-PROOF: $fails FAIL ❌"; exit 1; fi
