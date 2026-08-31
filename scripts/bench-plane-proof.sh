#!/usr/bin/env bash
# bench-plane-proof.sh — host oracle for the Bench lifecycle supervisor (ADR-003 Part 2 step 4).
#
# Proves gatekeeperd's `bench` verb manages the DURABLE record + ext4 PROJECT quota + data dir correctly,
# and that a Bench's quota is EDQUOT-enforced against a non-root writer — all in a privileged debian:trixie
# container in seconds, the fast gate before the ~10-min sealed-VM stage (which additionally proves the
# rootless-podman `run`/`enter` container ops, already green in the Bench-0 stage).
#
# Method: build gatekeeperd on the host, mount that binary into a --privileged container, stand up a
# loop ext4 pool (-O quota,project, mounted prjquota) as the Bench pool + fs, and drive the verb via the
# SHREK_BENCH_* env overrides. The enforcement write runs as a NON-ROOT user (root is quota-exempt).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

echo "=== building gatekeeperd (host, --features oracle-env) + the socket clients ==="
# oracle-env re-enables the SHREK_BENCH_* path overrides this oracle drives (records/pool/fs/anchor/run/
# seed). The SHIPPED image build compiles them OUT (must-fix 1), so this feature is oracle-only.
cargo build -p gatekeeperd --features oracle-env >/dev/null 2>&1
GK="$(pwd)/target/debug/gatekeeperd"
[ -x "$GK" ] || { echo "no gatekeeperd binary at $GK" >&2; exit 1; }
# the authz slice routes bench verbs over the socket — build the real clients so the oracle exercises them
# (shrek bench = the control-plane client; shrek-bench-run = the exported-.desktop launcher client).
cargo build -p shrek -p shrek-bench-run >/dev/null 2>&1
SHREK="$(pwd)/target/debug/shrek"; SBR="$(pwd)/target/debug/shrek-bench-run"
{ [ -x "$SHREK" ] && [ -x "$SBR" ]; } || { echo "no shrek / shrek-bench-run binaries" >&2; exit 1; }

cat > /tmp/bench-proof-inner.sh <<'INNER'
set -u
pass=0; fail=0
ok(){ echo "  PASS $*"; pass=$((pass+1)); }
bad(){ echo "  FAIL $*"; fail=$((fail+1)); }

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
# step 5 adds real rootless-podman + ip/nft so grants + egress are proven live, not just recorded.
apt-get install -y -qq e2fsprogs quota podman uidmap iproute2 nftables socat >/dev/null 2>&1

# --- stand up a loop ext4 Bench pool: -O quota,project + mounted prjquota, exactly as shrek-data /home.
truncate -s 256M /pool.img
mkfs.ext4 -F -q -O quota,project /pool.img
mkdir -p /mnt/pool
# Pre-create loop device NODES: a --privileged container grants the loop capability but not the /dev/loopN
# nodes; under host loop pressure `mount -o loop` picks a HIGH free number whose node is absent and fails
# intermittently ("no such bench" cascade — the mount silently didn't take). Create a wide range so any
# free loop the kernel picks has a node. (STM docker-loop-mount-mkosi-image.)
for i in $(seq 0 63); do [ -e /dev/loop$i ] || mknod -m660 /dev/loop$i b 7 $i; done
mount -o loop,prjquota /pool.img /mnt/pool
# FS grants relocate a bind into the HOST mount ns and rely on it PROPAGATING into dev's already-running
# rootless-podman pause mount-ns. That propagation needs `/` to be rshared BEFORE the first podman command
# creates the pause ns (real systemd makes / rshared at boot; a bare docker container's / is private, so
# make it rshared here to match the sealed system — proven: without it the relocate bind is invisible to
# podman and it binds the empty underlying dir instead).
mount --make-rshared / 2>/dev/null || true
# a real desktop user with a home + subuid range (rootless podman + the grant round-trip need both).
useradd -m -u 1000 dev 2>/dev/null || true
echo 'dev:100000:65536' > /etc/subuid
echo 'dev:100000:65536' > /etc/subgid
chown -R dev:dev /home/dev
RT=/run/user/1000; mkdir -p "$RT"; chown dev:dev "$RT"
bdev(){ runuser -u dev -- env HOME=/home/dev XDG_RUNTIME_DIR="$RT" PATH=/usr/bin:/bin "$@"; }
# offline seed the supervisor's `run` uses (a real image, tagged into dev's local store as localhost/scratch;
# busybox stands in for the shipped alpine+ffmpeg seed — same load/run mechanics, no 52M pull in the oracle).
bdev podman pull -q docker.io/library/busybox:latest >/dev/null 2>&1 && bdev podman tag docker.io/library/busybox:latest localhost/scratch >/dev/null 2>&1
# step 6: stage the seed as a sysext-style OCI-archive + digest sidecar (the built image Id) so ensure_seed's
# product loader can be proven — it re-loads localhost/scratch from this tar when the image is absent/stale.
mkdir -p /seed; chown dev:dev /seed   # dev's rootless podman writes the archive here
bdev podman save --format oci-archive -o /seed/scratch.tar localhost/scratch >/dev/null 2>&1
bdev podman image inspect localhost/scratch --format '{{.Id}}' > /seed/scratch.tar.digest 2>/dev/null

export SHREK_BENCH_POOL=/mnt/pool
export SHREK_BENCH_DIR=/mnt/pool/records
export SHREK_BENCH_FS=/mnt/pool
export SHREK_BENCH_ANCHOR=/home/dev
export SHREK_BENCH_SEED=localhost/scratch
export SHREK_BENCH_SEED_TAR=/seed/scratch.tar
GK=/gatekeeperd

echo "--- create two benches (distinct auto-allocated project ids) ---"
$GK bench create alpha --quota 1024 >/dev/null 2>&1
$GK bench create beta  --quota 2048 >/dev/null 2>&1
RA=/mnt/pool/records/alpha; RB=/mnt/pool/records/beta
[ -f "$RA" ] && [ -f "$RB" ] && ok "records written for both benches" || bad "record(s) missing"
grep -q '^SHREK-BENCH 1' "$RA" && ok "record has the versioned header" || bad "record header wrong"
PA=$(sed -n 's/^project //p' "$RA"); PB=$(sed -n 's/^project //p' "$RB")
[ "$PA" = 100000 ] && [ "$PB" = 100001 ] && ok "project ids allocated from base, distinct ($PA,$PB)" || bad "project ids wrong ($PA,$PB)"
[ "$(sed -n 's/^state //p' "$RA")" = created ] && ok "initial state=created" || bad "state not created"
[ "$(sed -n 's/^quota_kib //p' "$RA")" = 1024 ] && ok "quota_kib recorded" || bad "quota_kib wrong"

echo "--- data dir is project-tagged (chattr) ---"
lsattr -pd /mnt/pool/b/alpha 2>/dev/null | grep -q "^ *100000.*P" && ok "alpha data dir carries project id 100000 + inherit flag" || bad "project id not on data dir [$(lsattr -pd /mnt/pool/b/alpha 2>/dev/null)]"
[ "$(stat -c %u /mnt/pool/b/alpha)" = 1000 ] && ok "data dir chowned to dev" || bad "data dir not owned by dev"

echo "--- QUOTA ENFORCEMENT: a NON-ROOT write past alpha's 1 MiB cap must EDQUOT ---"
# root is quota-exempt, so write as dev; conv=fsync forces allocation (ext4 delayed alloc).
if su dev -s /bin/sh -c 'dd if=/dev/zero of=/mnt/pool/b/alpha/fill bs=64k count=64 conv=fsync' 2>/tmp/dd.err; then
  bad "write past cap SUCCEEDED (no enforcement) [$(tail -1 /tmp/dd.err)]"
else
  grep -qi quota /tmp/dd.err && ok "write blocked EDQUOT at the cap [$(tail -1 /tmp/dd.err)]" || bad "blocked but not by quota [$(tail -1 /tmp/dd.err)]"
fi

echo "--- quota <name> KiB re-caps + records it ---"
$GK bench quota alpha 512 >/dev/null 2>&1
[ "$(sed -n 's/^quota_kib //p' "$RA")" = 512 ] && ok "quota re-set to 512 + recorded" || bad "quota not updated in record"

echo "--- list shows both ---"
$GK bench list 2>/dev/null | grep -q alpha && $GK bench list 2>/dev/null | grep -q beta && ok "list shows both benches" || bad "list incomplete"

echo "--- reset clears data, keeps the record ---"
su dev -s /bin/sh -c 'echo hi > /mnt/pool/b/beta/f' 2>/dev/null || true
$GK bench reset beta >/dev/null 2>&1
[ -f "$RB" ] && [ -z "$(ls -A /mnt/pool/b/beta 2>/dev/null)" ] && ok "reset wiped data, kept record" || bad "reset wrong (record gone or data left)"

echo "--- destroy removes record + data + frees the quota id ---"
$GK bench destroy alpha >/dev/null 2>&1
[ ! -f "$RA" ] && [ ! -d /mnt/pool/b/alpha ] && ok "destroy removed record + data dir" || bad "destroy left residue"
# a fresh create now reuses alpha's freed project id (100000), since the record set no longer holds it.
$GK bench create gamma >/dev/null 2>&1
[ "$(sed -n 's/^project //p' /mnt/pool/records/gamma)" = 100000 ] && ok "destroyed bench's project id is reused" || bad "project id not reused after destroy"

echo "===== STEP 5: route FS + egress grants through the existing Gatekeeper ====="

echo "--- FS grant: dev-owned dirs under the anchor, grant ro + rw, a run binds them at /grants/<leaf> ---"
mkdir -p /home/dev/media_in /home/dev/media_out
echo SOURCE > /home/dev/media_in/clip.txt
chown -R dev:dev /home/dev/media_in /home/dev/media_out
$GK bench grant gamma /home/dev/media_in  --ro >/tmp/g1.log 2>&1 && ok "grant --ro accepted" || bad "grant ro failed [$(tail -1 /tmp/g1.log)]"
$GK bench grant gamma /home/dev/media_out --rw >/tmp/g2.log 2>&1 && ok "grant --rw accepted" || bad "grant rw failed [$(tail -1 /tmp/g2.log)]"
{ grep -q '^grant fs-ro /home/dev/media_in' /mnt/pool/records/gamma && grep -q '^grant fs-rw /home/dev/media_out' /mnt/pool/records/gamma; } && ok "grants recorded durably (fs-ro / fs-rw lines)" || bad "grants not in the record"
findmnt -no OPTIONS /run/shrek/bench/gamma/grants/media_in 2>/dev/null | grep -q noexec && ok "grant materialized as a host-ns noexec bind" || bad "grant not materialized noexec"
# ProtectHome fix (Fable 1): the per-bench grants dir is dev-owned 0700 (no side-door into /home for others).
{ [ "$(stat -c '%u %a' /run/shrek/bench/gamma/grants 2>/dev/null)" = "1000 700" ]; } && ok "grants dir is dev-owned 0700" || bad "grants dir perms wrong [$(stat -c '%u %a' /run/shrek/bench/gamma/grants 2>/dev/null)]"
# run THROUGH the supervisor: read the ro grant, write the rw grant; the write must round-trip to dev on the host.
$GK bench run gamma -- sh -c 'cat /grants/media_in/clip.txt > /grants/media_out/copy.txt' >/tmp/g3.log 2>&1
{ [ -f /home/dev/media_out/copy.txt ] && [ "$(stat -c %u /home/dev/media_out/copy.txt)" = 1000 ]; } && ok "rw grant write round-trips to dev on the host" || bad "rw write did not round-trip [$(tail -2 /tmp/g3.log|tr '\n' '|')]"
$GK bench run gamma -- sh -c 'echo NO > /grants/media_in/nope.txt' >/tmp/g4.log 2>&1
[ ! -f /home/dev/media_in/nope.txt ] && ok "ro grant denies writes from inside the bench" || bad "ro grant was writable"

echo "--- grant refusals (fail-closed) ---"
$GK bench grant gamma /etc --rw >/dev/null 2>&1; [ $? -ne 0 ] && ok "grant outside the anchor refused" || bad "grant /etc should refuse"
$GK bench grant gamma /home/dev/media_in --ro >/dev/null 2>&1; [ $? -ne 0 ] && ok "duplicate grant refused" || bad "dup grant should refuse"

echo "--- reissue re-materializes FS grants after a simulated reboot (/run wiped) ---"
umount /run/shrek/bench/gamma/grants/media_in /run/shrek/bench/gamma/grants/media_out 2>/dev/null
rm -rf /run/shrek/bench/gamma
$GK bench reissue >/tmp/g5.log 2>&1
findmnt -no OPTIONS /run/shrek/bench/gamma/grants/media_out 2>/dev/null | grep -q noexec && ok "reissue re-pinned + re-materialized the FS grants" || bad "reissue did not re-materialize [$(tail -1 /tmp/g5.log)]"

echo "--- egress grant: the network verb is default-deny (only sealed profiles) ---"
$GK bench network gamma allow-all >/dev/null 2>&1; [ $? -ne 0 ] && ok "unknown egress profile refused" || bad "allow-all should refuse"
{ $GK bench network gamma github-https >/tmp/n1.log 2>&1 && grep -q '^grant net github-https' /mnt/pool/records/gamma; } && ok "sealed egress profile recorded" || bad "network verb failed [$(tail -1 /tmp/n1.log)]"

echo "--- egress LIVE: a networked run late-attaches a veth + the sealed nft allow-list into the bench netns ---"
$GK bench run gamma -- sleep 6 >/tmp/n2.log 2>&1 &
GKRUN=$!
INJ=no
for _ in $(seq 1 50); do ip netns list 2>/dev/null | grep -q bench_gamma && { INJ=yes; break; }; sleep 0.2; done
if [ "$INJ" = yes ]; then
  ok "networked run created the per-bench netns (rootless late-attach)"
  NFT=$(nft list table ip shrek_egress_bench_gamma 2>/dev/null)
  echo "$NFT" | grep -q 'hook input' && ok "fix-3 host-local input-drop installed in the bench netns" || bad "no input-drop [$(echo "$NFT"|tr '\n' '|'|tail -c 160)]"
  echo "$NFT" | grep -q 'dport 443 accept' && ok "sealed github-https allow-list installed" || bad "no allow rule [$(echo "$NFT"|tr '\n' '|'|tail -c 200)]"
else
  bad "no per-bench netns appeared [$(tail -2 /tmp/n2.log|tr '\n' '|')]"
fi
wait $GKRUN 2>/dev/null
ip netns list 2>/dev/null | grep -q bench_gamma && bad "egress plumbing not torn down after run" || ok "egress plumbing torn down after the run (fail-closed default)"

echo "--- destroy tears down grants + the /run bench dir ---"
$GK bench destroy gamma >/dev/null 2>&1
{ [ ! -d /run/shrek/bench/gamma ] && [ ! -f /mnt/pool/records/gamma ]; } && ok "destroy removed grant mounts + /run dir + record" || bad "destroy left residue"

echo "===== STEP 6: the offline-seed product loader (ensure_seed, digest-keyed) ====="
echo "--- ensure_seed re-loads the seed from the sysext archive when the image is absent ---"
bdev podman rmi -f localhost/scratch >/dev/null 2>&1
bdev podman image exists localhost/scratch && bad "seed image should be absent before the load test" || ok "seed image removed (simulates a fresh boot with nothing pre-loaded)"
$GK bench create seedt >/dev/null 2>&1
$GK bench run seedt -- true >/tmp/seed.log 2>&1; SRC=$?
{ [ "$SRC" -eq 0 ] && bdev podman image exists localhost/scratch; } && ok "ensure_seed loaded localhost/scratch from the tar + ran (rc0)" || bad "ensure_seed did not load the seed [rc=$SRC $(tail -1 /tmp/seed.log)]"
# freshness: with the image now loaded at the sidecar's id, a second run must NOT re-load (idempotent).
ID1=$(bdev podman image inspect localhost/scratch --format '{{.Id}}' 2>/dev/null)
$GK bench run seedt -- true >/dev/null 2>&1
ID2=$(bdev podman image inspect localhost/scratch --format '{{.Id}}' 2>/dev/null)
[ -n "$ID1" ] && [ "$ID1" = "$ID2" ] && ok "a fresh seed is not re-loaded (digest match = idempotent)" || bad "seed id churned across runs ($ID1 -> $ID2)"
$GK bench destroy seedt >/dev/null 2>&1

echo "===== STEP 7: constrained .desktop export (fixed-baked-key) ====="
APPS=/home/dev/.local/share/applications
$GK bench create exp1 >/dev/null 2>&1
echo "--- fix 5: a grant into a dot-leading dir (~/.local) is refused ---"
install -d -o dev -g dev "$APPS"
$GK bench grant exp1 "$APPS" --rw >/dev/null 2>&1 && bad "dot-path grant (~/.local/...) should be refused" || ok "dot-path grant refused (a workload can't plant an unconstrained .desktop)"
echo "--- export writes a CONSTRAINED .desktop (as dev) + records key->workload in the root-owned record ---"
$GK bench export exp1 hello --label "Hello Bench" -- sh -c 'echo hi' >/tmp/e1.log 2>&1 && ok "export accepted" || bad "export failed [$(tail -1 /tmp/e1.log)]"
DF="$APPS/shrek-bench-exp1-hello.desktop"
[ -f "$DF" ] && ok ".desktop materialized" || bad ".desktop missing"
[ "$(stat -c %U "$DF" 2>/dev/null)" = dev ] && ok ".desktop is dev-owned (written AS DEV, not root — fix 2)" || bad ".desktop not dev-owned [$(stat -c %U "$DF" 2>/dev/null)]"
grep -qx 'Exec=/usr/bin/shrek-bench-run exp1 hello' "$DF" && ok ".desktop Exec is the fixed wrapper + exactly 2 tokens" || bad ".desktop Exec wrong [$(grep '^Exec=' "$DF")]"
{ grep -qiE '^(DBusActivatable|Actions|MimeType)=' "$DF" || grep '^Exec=' "$DF" | grep -qE '%[fFuUick]'; } && bad ".desktop has a field code / risky directive [$(grep -iE '^(DBusActivatable|Actions|MimeType|Exec)=' "$DF"|tr '\n' '|')]" || ok ".desktop carries NO command + NO field codes + no risky directives (key discipline holds)"
grep -q '^export hello ' /mnt/pool/records/exp1 && ok "export recorded in the root-owned record" || bad "export not recorded [$(grep export /mnt/pool/records/exp1|tr '\n' '|')]"
echo "--- run-export resolves the key SERVER-SIDE and runs the workload; a forged key is refused ---"
$GK bench run-export exp1 hello >/tmp/e2.log 2>&1 && ok "run-export resolves the registered key + runs (rc0)" || bad "run-export failed [$(tail -2 /tmp/e2.log|tr '\n' '|')]"
$GK bench run-export exp1 bogus >/dev/null 2>&1 && bad "an unregistered key must be refused" || ok "unregistered launcher key refused (a forged .desktop can inject nothing)"
echo "--- unexport + destroy sweep the .desktop ---"
$GK bench unexport exp1 hello >/dev/null 2>&1 && ok "unexport accepted" || bad "unexport failed"
{ [ ! -f "$DF" ] && ! grep -q '^export hello ' /mnt/pool/records/exp1; } && ok "unexport removed the .desktop + the record entry" || bad "unexport left residue"
$GK bench export exp1 hello2 -- true >/dev/null 2>&1
DF2="$APPS/shrek-bench-exp1-hello2.desktop"
$GK bench destroy exp1 >/dev/null 2>&1
[ ! -f "$DF2" ] && ok "destroy swept the bench's exported .desktop(s)" || bad "destroy left a .desktop behind"

echo "===== STEP 8: NORTH-STAR — a real offline Media transcode E2E through the Bench ====="
if [ "${MEDIA_SEED:-0}" = 1 ] && [ -f /host-seed/scratch.tar ]; then
  # Use the ACTUAL shipped Scratch seed (alpine+coreutils+ffmpeg+exit42), not a stand-in — this is the real
  # product ffmpeg doing a real transcode via REAL rootless podman (the run path gatekeeperd drives as dev).
  # The step-6 gotcha was a ROOT podman-in-docker nested-cgroup artifact; the rootless path is the real one.
  cp /host-seed/scratch.tar /seed/scratch-media.tar; chown dev:dev /seed/scratch-media.tar
  bdev podman rmi -f localhost/scratch >/dev/null 2>&1     # drop the busybox stand-in from steps 1-7
  bdev podman load -i /seed/scratch-media.tar >/dev/null 2>&1   # load the real seed to fabricate the input
  bdev podman image inspect localhost/scratch --format '{{.Id}}' > /seed/scratch-media.tar.digest 2>/dev/null
  mkdir -p /home/dev/m_in /home/dev/m_out; chown -R dev:dev /home/dev/m_in /home/dev/m_out
  # fabricate a real input video OFFLINE (testsrc -> mpeg4, a built-in encoder), owned by dev.
  bdev podman run --rm --network=none --no-hosts --runtime crun -v /home/dev/m_in:/o localhost/scratch \
    ffmpeg -hide_banner -y -f lavfi -i testsrc=duration=1:size=128x96:rate=12 -pix_fmt yuv420p -c:v mpeg4 /o/clip.mp4 >/tmp/m0.log 2>&1
  { [ -s /home/dev/m_in/clip.mp4 ] && [ "$(stat -c %u /home/dev/m_in/clip.mp4)" = 1000 ]; } && ok "offline input video fixture created ($(stat -c %s /home/dev/m_in/clip.mp4) bytes, dev-owned)" || bad "input fixture failed [$(tail -2 /tmp/m0.log|tr '\n' '|')]"
  # drop the image so the bench run's ensure_seed must RE-LOAD the real seed from the archive (product path).
  bdev podman rmi -f localhost/scratch >/dev/null 2>&1
  export SHREK_BENCH_SEED=localhost/scratch
  export SHREK_BENCH_SEED_TAR=/seed/scratch-media.tar
  # the interaction model: create a media bench, grant ONLY input (ro) + dest (rw), transcode inside it.
  $GK bench create media --quota 8192 >/tmp/m1.log 2>&1 && ok "media bench created" || bad "create failed [$(tail -1 /tmp/m1.log)]"
  $GK bench grant media /home/dev/m_in  --ro >/tmp/m2.log 2>&1 && ok "input dir granted --ro" || bad "grant ro failed [$(tail -1 /tmp/m2.log)]"
  $GK bench grant media /home/dev/m_out --rw >/tmp/m3.log 2>&1 && ok "dest dir granted --rw" || bad "grant rw failed [$(tail -1 /tmp/m3.log)]"
  # THE north-star: a real ffmpeg transcode INSIDE the bench, reading the ro input, writing the rw dest.
  $GK bench run media -- ffmpeg -hide_banner -y -i /grants/m_in/clip.mp4 -c:v libvpx -b:v 200k -an /grants/m_out/out.webm >/tmp/m4.log 2>&1; MRC=$?
  OUT=/home/dev/m_out/out.webm
  { [ "$MRC" -eq 0 ] && [ -s "$OUT" ]; } && ok "transcode ran INSIDE the bench + output landed on the host ($(stat -c %s "$OUT" 2>/dev/null) bytes)" || bad "transcode failed [rc=$MRC $(tail -3 /tmp/m4.log|tr '\n' '|')]"
  [ "$(stat -c %u "$OUT" 2>/dev/null)" = 1000 ] && ok "output round-trips to the host owned by dev (rw grant)" || bad "output not dev-owned [$(stat -c %u "$OUT" 2>/dev/null)]"
  bdev podman image exists localhost/scratch && ok "the run's ensure_seed re-loaded the real seed from the archive" || bad "ensure_seed did not re-load the media seed"
  # validate it's a REAL decodable video (probe with the seed's own ffprobe), not an empty/garbage file.
  VC=$(bdev podman run --rm --network=none --no-hosts --runtime crun -v /home/dev/m_out:/o:ro localhost/scratch ffprobe -v error -select_streams v:0 -show_entries stream=codec_name -of default=nw=1:nk=1 /o/out.webm 2>/dev/null | tr -d '\r')
  [ "$VC" = vp8 ] && ok "output is a real VP8/webm video stream (ffprobe: $VC)" || bad "output not a valid VP8 video [$VC]"
  # host stays SEALED: a write to the read-only input grant from inside the bench is denied.
  $GK bench run media -- sh -c 'echo pwn > /grants/m_in/pwn.txt' >/dev/null 2>&1
  [ ! -e /home/dev/m_in/pwn.txt ] && ok "the ro input grant is not writable from the bench (host stays sealed)" || bad "ro input grant was writable from the bench"
  # destroy removes ALL the bench's tooling + mutable state, but the delivered output PERSISTS on the host.
  $GK bench destroy media >/dev/null 2>&1
  { [ ! -f /mnt/pool/records/media ] && [ ! -d /mnt/pool/b/media ] && [ ! -d /run/shrek/bench/media ] && [ -s "$OUT" ]; } && ok "destroy removed the bench (record+data+/run) yet kept the transcoded output" || bad "destroy left residue or lost the output"
  unset SHREK_BENCH_SEED SHREK_BENCH_SEED_TAR
else
  echo "  SKIP STEP 8: no shipped Scratch seed mounted (build it: scripts/build-bench-seed.sh) — the sealed-VM dogfood proves the media north-star with the baked seed"
fi

echo "===== AUTHZ SLICE: bench control plane over the authenticated socket (steps 1+2) ====="
# The broker runs as ROOT (it does the privileged work); `dev` is only the authenticated PEER asking. It
# binds the DEFAULT socket so the real clients ($SHREK / $SBR, whose paths are fixed) reach it. A busybox
# seed (tagged localhost/scratch, tar at /seed/scratch.tar from step 5) lets run/run-export execute.
bdev podman tag docker.io/library/busybox:latest localhost/scratch >/dev/null 2>&1 || bdev podman load -i /seed/scratch.tar >/dev/null 2>&1
SHREK=/shrek; SBR=/shrek-bench-run   # the real clients, mounted into the container by the outer harness
SHREK_BROKER_NOMOUNT=1 \
  SHREK_BENCH_POOL=/mnt/pool SHREK_BENCH_DIR=/mnt/pool/records SHREK_BENCH_FS=/mnt/pool \
  SHREK_BENCH_ANCHOR=/home/dev SHREK_BENCH_SEED=localhost/scratch SHREK_BENCH_SEED_TAR=/seed/scratch.tar \
  "$GK" >/tmp/gkd.log 2>&1 &
GKD=$!
GKSOCK=/run/shrek-gk.sock
for _ in $(seq 1 50); do [ -S "$GKSOCK" ] && break; sleep 0.1; done
[ -S "$GKSOCK" ] && ok "broker daemon bound the default socket (allowlist: $(grep -o 'allowed uids.*' /tmp/gkd.log | tail -1))" || bad "broker did not bind [$(tail -2 /tmp/gkd.log | tr '\n' '|')]"
# the clients connect as dev; shrek reads SHREK_BROKER_SOCK, shrek-bench-run uses its fixed default path.
sdev() { runuser -u dev -- env HOME=/home/dev XDG_RUNTIME_DIR="$RT" SHREK_BROKER_SOCK="$GKSOCK" PATH=/usr/bin:/bin "$@"; }

echo "--- step 1 (de-risk): destroy over the socket, refused-uid, onion gate ---"
# (i) a non-allowlisted uid is refused by SO_PEERCRED (not root/shrek/dev, no SHREK_BROKER_ALLOW_UID here).
useradd -u 5555 -M -s /usr/sbin/nologin nogk 2>/dev/null || true
REF=$(printf 'BENCH destroy 1\nzzz\n' | runuser -u nogk -- timeout 5 socat - "UNIX-CONNECT:$GKSOCK" 2>/dev/null)
echo "$REF" | grep -q 'END 1' && ok "non-allowlisted uid (5555) refused at the socket by SO_PEERCRED" || bad "unallowlisted uid not refused [$REF]"
# (ii) destroy sockA over the SOCKET (as dev, via $SHREK); destroy sockB via the ROOT binary path — identical state.
$GK bench create sockA --quota 1024 >/dev/null 2>&1; $GK bench create sockB --quota 1024 >/dev/null 2>&1
sdev "$SHREK" bench destroy sockA >/tmp/sd.log 2>&1 && ok "dev drove 'shrek bench destroy' over the socket (rc0, no sudo)" || bad "socket destroy failed [$(cat /tmp/sd.log)]"
$GK bench destroy sockB >/dev/null 2>&1
{ [ ! -f /mnt/pool/records/sockA ] && [ ! -d /mnt/pool/b/sockA ] && [ ! -f /mnt/pool/records/sockB ] && [ ! -d /mnt/pool/b/sockB ]; } && ok "socket-destroy and binary-destroy leave byte-identical state (record + data gone both ways)" || bad "socket vs binary destroy diverged"
# dev must NOT reach the privileged onion verbs (bench-only peer).
ON=$(printf 'status\n' | runuser -u dev -- timeout 5 socat - "UNIX-CONNECT:$GKSOCK" 2>/dev/null)
echo "$ON" | grep -q 'onion-not-permitted' && ok "dev is gated OUT of the onion verbs (bench-only peer)" || bad "dev reached an onion verb [$(echo "$ON" | tr '\n' '|')]"

echo "--- step 2 (full transport): every neutral verb over the socket via the real clients ---"
sdev "$SHREK" bench create c1 --quota 1024 >/tmp/c1.log 2>&1
{ [ -f /mnt/pool/records/c1 ] && [ "$(sed -n 's/^state //p' /mnt/pool/records/c1)" = created ]; } && ok "'shrek bench create' over the socket (record written, state=created)" || bad "socket create failed [$(cat /tmp/c1.log)]"
sdev "$SHREK" bench list 2>/dev/null | grep -q 'c1' && ok "'shrek bench list' streams the bench over the socket" || bad "socket list missing c1"
sdev "$SHREK" bench quota c1 2048 >/dev/null 2>&1
[ "$(sed -n 's/^quota_kib //p' /mnt/pool/records/c1)" = 2048 ] && ok "'shrek bench quota' re-caps over the socket (record=2048)" || bad "socket quota not applied"
# ARGV-FRAMING regression: a workload after `--` carrying spaces must round-trip EXACTLY (the count-framed
# request preserves it; the old whitespace-split wire would corrupt it).
sdev "$SHREK" bench run c1 -- sh -c 'printf "%s\n" "a b  c" > /work/m.txt' >/tmp/run.log 2>&1
{ [ -f /mnt/pool/b/c1/m.txt ] && grep -qx 'a b  c' /mnt/pool/b/c1/m.txt; } && ok "workload argv with embedded spaces round-trips exactly over the count-framed socket" || bad "argv framing corrupted the workload [$(cat /mnt/pool/b/c1/m.txt 2>/dev/null | cat -A)]"
# shrek-bench-run (the exported-.desktop launcher) drives run-export over the socket, NO sudo. Export via the
# ROOT binary path (export is authority-increasing, ceremony-gated over the socket); the launcher is neutral.
$GK bench create c3 >/dev/null 2>&1
$GK bench export c3 hi -- sh -c 'exit 7' >/tmp/ex.log 2>&1
runuser -u dev -- env HOME=/home/dev XDG_RUNTIME_DIR="$RT" "$SBR" c3 hi >/dev/null 2>&1; XRC=$?
[ "$XRC" -eq 7 ] && ok "shrek-bench-run drives run-export over the socket + propagates the workload rc (7, no sudo)" || bad "launcher run-export wrong rc=$XRC"
runuser -u dev -- env HOME=/home/dev XDG_RUNTIME_DIR="$RT" "$SBR" c3 forged >/dev/null 2>&1 && bad "a forged launcher key must be refused" || ok "shrek-bench-run: an unregistered key is refused server-side"
sdev "$SHREK" bench destroy c1 >/dev/null 2>&1; $GK bench destroy c3 >/dev/null 2>&1

echo "--- step 3 (consent ceremony): authority-increasing verbs gated on a spoof-proof console ---"
# A headless container has NO console seat/VT and NO logind SecureAttentionKey, so the ceremony MUST fail
# closed: an authority-increasing socket request from dev returns 'ceremony-<reason>' and applies NOTHING.
# This is the reachable-headlessly half of the fail-closed matrix; the real VT + a scripted 'y' is the
# sealed-VM dogfood's BENCH-CONSENT stage. (Order matters: probe precheck BEFORE any ceremony deny arms
# the per-(uid,verb) cooldown.)
$GK bench create czt --quota 1024 >/dev/null 2>&1
mkdir -p /home/dev/czt_grant; chown -R dev:dev /home/dev/czt_grant
# (i) an INVALID request is refused at PRECHECK — cheap, local, human NEVER asked, no cooldown armed.
PX=$(sdev "$SHREK" bench grant nosuchbench /home/dev/czt_grant --ro 2>&1)
echo "$PX" | grep -q 'refused precheck' && ok "an invalid authority request is refused at precheck (human never asked)" || bad "invalid request not precheck-refused [$PX]"
# (ii) a VALID grant reaches the ceremony, which fails closed with no seat, and applies NOTHING.
GX=$(sdev "$SHREK" bench grant czt /home/dev/czt_grant --ro 2>&1)
echo "$GX" | grep -q 'refused ceremony-' && ok "a valid grant fails closed at the ceremony with no seat (ceremony-*, headless)" || bad "grant not ceremony-gated [$GX]"
grep -q '^grant fs-' /mnt/pool/records/czt && bad "a denied ceremony still recorded a grant (NOT fail-closed!)" || ok "the denied grant applied NOTHING (no fs grant in the record)"
# (iii) a repeated authority request is rate-limited by the post-deny cooldown (anti SAK-fatigue).
CX=$(sdev "$SHREK" bench grant czt /home/dev/czt_grant --ro 2>&1)
echo "$CX" | grep -q 'refused cooldown' && ok "a repeated authority request is cooldown-limited after the deny" || bad "repeat request not cooldown-limited [$CX]"
# (iv) network to a PROFILE is authority-increasing (ceremony-gated); network none REVOKES (ceremony-free).
NX=$(sdev "$SHREK" bench network czt github-https 2>&1)
{ echo "$NX" | grep -q 'refused ceremony-' && ! grep -q '^grant net ' /mnt/pool/records/czt; } && ok "network <profile> is ceremony-gated and records nothing on deny" || bad "network profile not gated [$NX]"
sdev "$SHREK" bench network czt none >/tmp/nn.log 2>&1 && ok "network none (revoke, reducing authority) is allowed ceremony-free over the socket" || bad "network none should be ceremony-free [$(cat /tmp/nn.log)]"
# (v) root is refused on the socket ceremony path (root drives cli() in-process, never the socket).
RX=$(printf 'BENCH grant 2\nczt\n%s\n' "$(printf /home/dev/czt_grant | sed 's/\//%2F/g')" | timeout 5 socat - "UNIX-CONNECT:$GKSOCK" 2>/dev/null)
echo "$RX" | grep -q 'root-uses-cli' && ok "root is refused on the socket ceremony (must use cli() in-process)" || bad "root not refused on the socket ceremony [$(echo "$RX" | tr '\n' '|')]"
$GK bench destroy czt >/dev/null 2>&1

kill "$GKD" 2>/dev/null; wait "$GKD" 2>/dev/null

umount /mnt/pool 2>/dev/null || true
echo "=== bench-plane oracle: PASS=$pass FAIL=$fail ==="
[ "$fail" -eq 0 ]
INNER

echo "=== running bench-plane oracle in privileged debian:trixie ==="
# Step 8 (the media north-star) uses the ACTUAL shipped Scratch seed (alpine+coreutils+ffmpeg) for a real
# transcode — no stand-in. It is a gitignored build product (scripts/build-bench-seed.sh); mount it if
# present and let the inner script run step 8, else the inner script prints a loud SKIP (the sealed-VM
# dogfood proves the media north-star with the baked seed regardless).
SEED_TAR="$(pwd)/layers/shrek-bench/overlay/usr/share/shrek/bench/seeds/scratch.tar"
DOCKER_ARGS=(--rm --privileged -v "$GK":/gatekeeperd:ro -v "$SHREK":/shrek:ro -v "$SBR":/shrek-bench-run:ro -v /tmp/bench-proof-inner.sh:/inner.sh:ro)
if [ -f "$SEED_TAR" ]; then
  DOCKER_ARGS+=(-v "$SEED_TAR":/host-seed/scratch.tar:ro -e MEDIA_SEED=1)
  echo "  (media north-star: mounting the shipped seed $(du -h "$SEED_TAR" | cut -f1))"
else
  DOCKER_ARGS+=(-e MEDIA_SEED=0)
  echo "  (media north-star: no shipped seed built — step 8 will SKIP in the oracle; build with scripts/build-bench-seed.sh)"
fi
docker run "${DOCKER_ARGS[@]}" debian:trixie bash /inner.sh
