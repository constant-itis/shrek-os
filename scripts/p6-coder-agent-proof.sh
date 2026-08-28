#!/usr/bin/env bash
# Phase-6 slice-2 — the FIRST REAL CODING-AGENT WORKFLOW proof (docs/phase6-slice2-coder-agent.md).
# A model-driven agent (crates/coder) receives a bounded task and drives inspect→edit→build/test→return
# INSIDE a genuine T2 (gVisor) session it enters EXCLUSIVELY through the shipped `shrek run` front door,
# over a sealed one-destination egress (`model-local`), against a real fs-verity harness, in a
# privileged debian:trixie oracle. SPIKE-ONLY (strip before ship). Fuses p6-coder-proof (T2 ingest RW
# session) + p6-egress-proof (named egress plumbing): the workload is no longer a hardcoded shell string
# but the coder binary talking to a model.
#
# The model is a DETERMINISTIC canned HTTP responder (fixed tool-call sequence ⇒ reproducible outputs);
# LIVE=1 instead points `shrek-model` at a real local model server (a LAN host:8100) — same coder binary,
# same protocol path, only the backing server changes (informational, non-gating).
#
# What only a real run can show (unit tests cover the pure parser/loop pieces):
#   A1  authentic harness ⇒ derived=T-untrust, construct-at=T2 (banding unchanged through `shrek run`).
#   A2  the AGENT LOOP ran: CODER-STEP ≥1 and a write_file + a run tool-call executed (model-driven).
#   A3  real build/test: the edited source compiled with tcc, the ELF RAN, marker+exit == the pass
#       criterion (REAL-COMPILE-RUN-OK / PROG-EXIT=42), and the agent returned CODER-DONE ok=true.
#   A4  write-through: the fixed source + the compiled ELF are visible ON THE HOST afterward.
#   A5  the wall holds DURING a model-driven session: ungranted vault ENOENT, host sentinel absent, and
#       a non-`model-local` dst (1.1.1.1:53) is DROPPED even though egress is UP (sealed to one dst).
#   A6  fail-closed: harness digest ABSENT ⇒ T-hostile ⇒ the SAME `shrek run` refuses, coder never runs.
set -uo pipefail

PIN_SHA256="670bcd3cbc103f00d8bb5098edc370f32397ee4c134231436bafa659bb3c068e"
PIN_URL="https://storage.googleapis.com/gvisor/releases/release/20260810.0/x86_64/runsc"
LIVE="${LIVE:-0}"
# The local model endpoint for the LIVE smoke (a LAN host, no auth). shrek-model resolves
# here when LIVE=1; the deterministic gate maps it to the in-oracle canned responder instead.
# Override for your own setup: LIVE_MODEL_IP=<your LAN model host>.
LIVE_MODEL_IP="${LIVE_MODEL_IP:-127.0.0.1}"

if [ "${IN_CONTAINER:-0}" != "1" ]; then
  REPO_ROOT="$(git rev-parse --show-toplevel)"
  echo "=== building gatekeeperd --features spike + shrek + coder (host) ==="
  ( cd "$REPO_ROOT" && cargo build --release -p gatekeeperd --features spike && \
    cargo build --release -p shrek && cargo build --release -p coder ) || exit 3

  CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/shrek"; mkdir -p "$CACHE"
  RUNSC="$CACHE/runsc-20260810.0"
  if [ ! -f "$RUNSC" ] || [ "$(sha256sum "$RUNSC" | awk '{print $1}')" != "$PIN_SHA256" ]; then
    echo "=== fetching pinned runsc (release-20260810.0) ==="
    curl -fsSL -m 300 -o "$RUNSC" "$PIN_URL" || { echo "runsc fetch failed"; exit 3; }
  fi
  [ "$(sha256sum "$RUNSC" | awk '{print $1}')" = "$PIN_SHA256" ] || { echo "PIN MISMATCH"; exit 3; }
  chmod +x "$RUNSC"

  # Default bridge for BOTH gate and live: the deterministic gate reaches its in-netns responder, and
  # for the live smoke the container reaches the real LAN model via docker's normal NAT egress — the
  # sandbox→eth0 forward stays INSIDE the container netns (plane policy accept), so it never crosses
  # the host's FORWARD DROP (a blocker that `--network host` exposed). No host firewall
  # mutation. (SHREK_FWD_ALLOW remains the documented opt-in if a host needs the explicit ACCEPT.)
  echo "=== launching privileged debian:trixie oracle (LIVE=$LIVE) ==="
  exec docker run --rm --privileged --cgroupns=private \
    -e IN_CONTAINER=1 -e LIVE="$LIVE" -e LIVE_MODEL_IP="$LIVE_MODEL_IP" \
    -v "$REPO_ROOT/scripts/p6-coder-agent-proof.sh:/proof.sh:ro" \
    -v "$REPO_ROOT/target/release/gatekeeperd:/gatekeeperd:ro" \
    -v "$REPO_ROOT/target/release/shrek:/shrek:ro" \
    -v "$REPO_ROOT/target/release/coder:/coder-src:ro" \
    -v "$REPO_ROOT/tests/fixtures/coder-task/buggy.c:/fixture-buggy.c:ro" \
    -v "$RUNSC:/runsc-src:ro" \
    debian:trixie bash /proof.sh
fi

# ---------------- inside the container ----------------
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
# busybox-static = sandbox userland; systemd = systemd-detect-virt; e2fsprogs = mkfs.ext4 -O verity;
# iproute2/nftables = egress plane; tcc = the sealed C compiler the agent drives; python3 = the canned
# model responder; ncat kept for parity. The coder binary is glibc-dynamic (its ldd closure is sealed
# into the rootfs like tcc's).
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
ADMIT_OK=/tmp/ingest-admit.ok; ADMIT_BAD=/tmp/ingest-admit.bad
printf 'shrek-t2-ingest-admit v1\n# authorised T2 untrusted-ingest harness\n%s %s\n' "$ALGO" "$HEX" > "$ADMIT_OK"
printf 'shrek-t2-ingest-admit v1\n' > "$ADMIT_BAD"   # header only ⇒ admits nothing (fail-closed list)
echo "provisioned genuine fs-verity harness: runsc = $ALGO $HEX"

# --- Sealed T2 rootfs: busybox applets + tcc + tcc closure + THE CODER BINARY + its ldd closure. The
#     coder is a first-party TOOL sealed in the rootfs (integrity from the image seal); the task repo
#     lives in the writable project GRANT, not here. RO-remounted to mimic sealed dm-verity /usr. ---
ROOTFS=/rootfs; rm -rf "$ROOTFS"; mkdir -p "$ROOTFS/bin"
cp /usr/bin/busybox "$ROOTFS/bin/busybox" 2>/dev/null || cp /bin/busybox "$ROOTFS/bin/busybox"
for a in sh cat ls nc timeout echo test cp chmod; do ln -sf busybox "$ROOTFS/bin/$a"; done
install -D -m0755 /usr/bin/tcc "$ROOTFS/usr/bin/tcc"
[ -e /lib64/ld-linux-x86-64.so.2 ] && install -D -m0755 /lib64/ld-linux-x86-64.so.2 "$ROOTFS/lib64/ld-linux-x86-64.so.2"
for so in $(ldd /usr/bin/tcc | grep -oE '/[^ ]+\.so[^ ]*'); do install -D -m0755 "$so" "$ROOTFS$so"; done
install -D -m0755 /coder-src "$ROOTFS/usr/bin/coder"
for so in $(ldd /coder-src | grep -oE '/[^ ]+\.so[^ ]*'); do install -D -m0755 "$so" "$ROOTFS$so"; done
mount --bind "$ROOTFS" "$ROOTFS" && mount -o remount,ro,bind "$ROOTFS" || echo "WARN: rootfs not remounted ro (less faithful)"

# --- Anchor + writable project (the untrusted task repo) + build area + ungranted vault + host sentinel. ---
rm -rf /srv; mkdir -p /srv/project /srv/build /srv/vault
cp /fixture-buggy.c /srv/project/buggy.c
echo "pre-existing-project-file" > /srv/project/README
echo "TOP-SECRET-DO-NOT-LEAK" > /srv/vault/secret
echo "HOST-ONLY" > /etc/shrek-host-sentinel

# --- The FIXED reference source the canned model tells the agent to write (json.dumps escapes it). ---
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

# --- The local "internet": a server netns holding the model dst, reached via a masquerading uplink,
#     exactly as p6-egress-proof maps github-https. `shrek-model` → 10.20.0.2 (gate) so gatekeeperd
#     pins it and seeds the sandbox /etc/hosts; the coder dials it by name. ---
ip netns add srv
ip link add up-egr type veth peer name srv0
ip link set srv0 netns srv
ip addr add 10.20.0.1/24 dev up-egr; ip link set up-egr up
ip -n srv addr add 10.20.0.2/24 dev srv0; ip -n srv link set srv0 up; ip -n srv link set lo up
ip -n srv route add default via 10.20.0.1

if [ "$LIVE" = "1" ]; then
  # LIVE smoke: point shrek-model at the real 35B on the host LAN (informational, non-gating).
  printf '%s shrek-model\n' "$LIVE_MODEL_IP" >> /etc/hosts
  echo "LIVE mode: shrek-model -> $LIVE_MODEL_IP (real 35B); canned responder NOT started"
else
  # Deterministic gate: the canned responder serves a fixed tool-call sequence at shrek-model:8100.
  cat > /tmp/responder.py <<'PYEOF'
import http.server, json
FIXED = open('/tmp/fixed.c').read()
COMPILE = ("PATH=/usr/bin:/bin; export PATH; "
           "tcc -nostdlib -static -o /srv/build/prog buggy.c && /srv/build/prog; echo PROG-EXIT=$?")
WALL = ("PATH=/usr/bin:/bin; export PATH; "
        "if [ -e /srv/vault ]; then echo VAULT-VISIBLE-BAD; else echo VAULT-ABSENT-OK; fi; "
        "if [ -e /etc/shrek-host-sentinel ]; then echo HOST-SENTINEL-LEAK-BAD; else echo HOST-SENTINEL-ABSENT-OK; fi; "
        "if timeout 3 nc -w2 1.1.1.1 53 </dev/null >/dev/null 2>&1; then echo NET-1111-REACHED-BAD; else echo NET-1111-BLOCKED-OK; fi")
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
        i = min(H.step, len(STEPS) - 1); H.step += 1
        content = json.dumps(STEPS[i])
        body = json.dumps({"choices": [{"message": {"role": "assistant", "content": content}}]}).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
http.server.HTTPServer(('10.20.0.2', 8100), H).serve_forever()
PYEOF
  ip netns exec srv python3 /tmp/responder.py >/dev/null 2>&1 &
  SRV_PID=$!
  printf '10.20.0.2 shrek-model\n' >> /etc/hosts
  sleep 0.5
fi

# --- cgroup-v2 delegation (same dance as the sibling proofs: vacate root, delegate controllers,
#     per-run parent so back-to-back T2 builds stay clean). ---
if [ -w /sys/fs/cgroup/cgroup.subtree_control ]; then
  mkdir -p /sys/fs/cgroup/init
  for p in $(cat /sys/fs/cgroup/cgroup.procs 2>/dev/null); do echo "$p" > /sys/fs/cgroup/init/cgroup.procs 2>/dev/null || true; done
  echo "+memory +pids" > /sys/fs/cgroup/cgroup.subtree_control 2>/dev/null || true
fi

# Launch EXCLUSIVELY through `shrek run` (the shipped front door). The wrapper only places the process
# in a per-run cgroup + sets the oracle substrate overrides (the sealed image sets none); shrek run
# composes the gatekeeperd argv and execs it, inheriting env + cgroup (PG5 exit fidelity preserved).
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
echo "=== CA: authentic harness + model-local egress ⇒ agent solves the task through \`shrek run\` ==="
OUT=$(runshrek ca "$ADMIT_OK" --project /srv/project --build /srv/build --egress model-local \
        --trust T-hostile --tier T2 -- /usr/bin/coder --task "$TASK" --max-steps 8)
echo "$OUT" | sed 's/^/    /'
echo
# A1 — banding unchanged through the front door.
echo "$OUT" | grep -q "mode=ingest-harness derived=T-untrust" && pass "A1 admission ⇒ derived=T-untrust" || fail "A1 derived=T-untrust"
echo "$OUT" | grep -q "construct-at=T2 effective=T2"          && pass "A1 construct-at=T2"              || fail "A1 construct-at=T2"
echo "$OUT" | grep -q "egress=model-local"                    && pass "A1 decision egress=model-local" || fail "A1 egress name in decision"
if [ "$LIVE" != "1" ]; then
  # A2 — the model-driven loop ran (canned sequence is deterministic).
  echo "$OUT" | grep -q "CODER-STEP 1"                          && pass "A2 agent loop ran (CODER-STEP)"       || fail "A2 no CODER-STEP"
  echo "$OUT" | grep -q "CODER-TOOL write_file path=\"buggy.c\"" && pass "A2 model drove write_file(buggy.c)"   || fail "A2 no write_file tool-call"
  echo "$OUT" | grep -q "CODER-TOOL run "                        && pass "A2 model drove run (build/test)"      || fail "A2 no run tool-call"
  # A3 — real build/test + return.
  echo "$OUT" | grep -q "REAL-COMPILE-RUN-OK"                    && pass "A3 compiled ELF RAN (marker present)" || fail "A3 no run marker"
  echo "$OUT" | grep -q "PROG-EXIT=42"                           && pass "A3 program exited 42 (pass criterion)"|| fail "A3 wrong/absent exit code"
  echo "$OUT" | grep -q "CODER-DONE ok=true"                     && pass "A3 agent returned done ok=true"       || fail "A3 no CODER-DONE ok=true"
  # A4 — write-through to the host across both grants.
  grep -q "REAL-COMPILE-RUN-OK" /srv/project/buggy.c 2>/dev/null && pass "A4 write-through: fixed source on host project inode" || fail "A4 project not written through"
  head -c4 /srv/build/prog 2>/dev/null | grep -qa ELF            && pass "A4 write-through: compiled ELF on host build inode"  || fail "A4 build ELF not written through"
  grep -q "pre-existing-project-file" /srv/project/README 2>/dev/null && pass "A4 teardown non-destructive (README intact)" || fail "A4 teardown destroyed project content"
  # A5 — the wall held DURING the model-driven session.
  echo "$OUT" | grep -q "VAULT-ABSENT-OK"                        && pass "A5 ungranted vault ENOENT"            || fail "A5 vault visible"
  echo "$OUT" | grep -q "HOST-SENTINEL-ABSENT-OK"                && pass "A5 host sentinel absent"              || fail "A5 host fs leaked"
  echo "$OUT" | grep -q "NET-1111-BLOCKED-OK"                    && pass "A5 non-model dst DROPPED (egress sealed to one dst)" || fail "A5 non-model dst reachable"
  # Anchor to standalone RESULT lines: a real leak marker is command OUTPUT on its own line, whereas the
  # bad-marker strings also appear MID-LINE inside the echoed wall-probe cmd (`…else echo VAULT-VISIBLE-BAD…`)
  # — matching those would be an argv-echo false positive (the M4 `SHREK_GATE:FAIL` lesson).
  echo "$OUT" | grep -qE '^(VAULT-VISIBLE-BAD|HOST-SENTINEL-LEAK-BAD|NET-1111-REACHED-BAD)$' && fail "A5 LEAK MARKER present" || pass "A5 no leak markers (on result lines)"
else
  echo "  (LIVE) transcript above is informational — a real model may solve it differently; not gated."
fi

if [ "$LIVE" != "1" ]; then
  echo
  echo "=== A6: harness digest ABSENT ⇒ T-hostile ⇒ the SAME \`shrek run\` refuses, coder never runs ==="
  OUT6=$(runshrek cb "$ADMIT_BAD" --project /srv/project --build /srv/build --egress model-local \
          --trust T-hostile --tier T2 -- /usr/bin/coder --task "$TASK" --max-steps 8); rc6=$?
  echo "$OUT6" | grep -oE 'derived=[^ ]+|reason=[^ ]+|construct-at=[^ ]+' | sed 's/^/    /'
  echo "$OUT6" | grep -q "mode=ingest-harness derived=T-hostile" && pass "A6 no-admit ⇒ derived=T-hostile" || fail "A6 derived=T-hostile"
  { { [ $rc6 -eq 10 ] || [ $rc6 -eq 12 ]; } && echo "$OUT6" | grep -q "refused"; } && pass "A6 failed-closed rc=$rc6 (refused)" || fail "A6 failed-closed rc=$rc6"
  echo "$OUT6" | grep -q "construct-at=" && fail "A6 constructed (should refuse!)" || pass "A6 never-constructed"
  echo "$OUT6" | grep -q "CODER-START"   && fail "A6 coder ran (should never start!)" || pass "A6 coder never started"
fi

# --- teardown + residual check ---
[ "${SRV_PID:-}" ] && kill "$SRV_PID" 2>/dev/null
umount "$MNT" 2>/dev/null || true; losetup -d "$LOOP" 2>/dev/null || true
resid_ns=$(ip netns list 2>/dev/null | grep -oE 'shrek-[a-z0-9]+' | tr '\n' ',')
resid_tb=$(nft list tables 2>/dev/null | grep -o 'shrek_egress_[a-z0-9_]*' | tr '\n' ',')
if [ "$LIVE" != "1" ]; then
  { [ -z "$resid_ns" ] && [ -z "$resid_tb" ]; } && pass "teardown: no residual sandbox netns / nft table" || fail "teardown residual ns=[$resid_ns] tb=[$resid_tb]"
fi

echo
if [ "$LIVE" = "1" ]; then
  echo "P6-CODER-AGENT-PROOF (LIVE smoke): see transcript above (non-gating)"; exit 0
elif [ $fails -eq 0 ]; then echo "P6-CODER-AGENT-PROOF: ALL PASS ✅"; exit 0
else echo "P6-CODER-AGENT-PROOF: $fails FAIL ❌"; exit 1; fi
