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

echo "=== building gatekeeperd (host) ==="
cargo build -p gatekeeperd >/dev/null 2>&1
GK="$(pwd)/target/debug/gatekeeperd"
[ -x "$GK" ] || { echo "no gatekeeperd binary at $GK" >&2; exit 1; }

cat > /tmp/bench-proof-inner.sh <<'INNER'
set -u
pass=0; fail=0
ok(){ echo "  PASS $*"; pass=$((pass+1)); }
bad(){ echo "  FAIL $*"; fail=$((fail+1)); }

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
# step 5 adds real rootless-podman + ip/nft so grants + egress are proven live, not just recorded.
apt-get install -y -qq e2fsprogs quota podman uidmap iproute2 nftables >/dev/null 2>&1

# --- stand up a loop ext4 Bench pool: -O quota,project + mounted prjquota, exactly as shrek-data /home.
truncate -s 256M /pool.img
mkfs.ext4 -F -q -O quota,project /pool.img
mkdir -p /mnt/pool
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

umount /mnt/pool 2>/dev/null || true
echo "=== bench-plane oracle: PASS=$pass FAIL=$fail ==="
[ "$fail" -eq 0 ]
INNER

echo "=== running bench-plane oracle in privileged debian:trixie ==="
docker run --rm --privileged \
  -v "$GK":/gatekeeperd:ro \
  -v /tmp/bench-proof-inner.sh:/inner.sh:ro \
  debian:trixie bash /inner.sh
