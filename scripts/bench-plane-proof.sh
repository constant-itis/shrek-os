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

apt-get update -qq >/dev/null 2>&1
apt-get install -y -qq e2fsprogs quota >/dev/null 2>&1

# --- stand up a loop ext4 Bench pool: -O quota,project + mounted prjquota, exactly as shrek-data /home.
truncate -s 256M /pool.img
mkfs.ext4 -F -q -O quota,project /pool.img
mkdir -p /mnt/pool
mount -o loop,prjquota /pool.img /mnt/pool
useradd -M -u 1000 dev 2>/dev/null || true

export SHREK_BENCH_POOL=/mnt/pool
export SHREK_BENCH_DIR=/mnt/pool/records
export SHREK_BENCH_FS=/mnt/pool
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

echo "--- deferred verbs fail clearly (not silent success) ---"
$GK bench grant alpha /x --rw >/dev/null 2>&1; [ $? -ne 0 ] && ok "grant verb refuses (step 5 stub)" || bad "grant should refuse"

umount /mnt/pool 2>/dev/null || true
echo "=== bench-plane oracle: PASS=$pass FAIL=$fail ==="
[ "$fail" -eq 0 ]
INNER

echo "=== running bench-plane oracle in privileged debian:trixie ==="
docker run --rm --privileged \
  -v "$GK":/gatekeeperd:ro \
  -v /tmp/bench-proof-inner.sh:/inner.sh:ro \
  debian:trixie bash /inner.sh
