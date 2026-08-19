#!/usr/bin/env bash
# Phase-5 slice-8 — sealed pin-manifest (B1 `T-pinned`) CLASSIFICATION proof. Drives the REAL release
# gatekeeperd in a privileged debian:trixie oracle on a GENUINE fs-verity filesystem — the fast path
# before the ~35-min VM gate. SPIKE-ONLY (strip before ship, with the gate scaffolding).
#
# What only a real fs-verity run can show (the unit tests cover the pure parser/lattice):
#   (1) a closed-world pinned digest ⇒ gatekeeperd DERIVES `T-pinned`, the measured fd is BOUND
#       (exec_fd_bound=true), and the construction REFUSES deterministically
#       (reason=pinned-exec-home-unavailable, rc=15) with NO constructor — execution is fail-closed and
#       T0's MS_NOEXEC/Landlock posture is untouched (classification-only slice).
#   (2) anti-spoof: a one-byte-different SECOND verity inode ⇒ `T-hostile` (its digest isn't pinned;
#       an fs-verity file is itself immutable, so a distinct inode is the real attack, not in-place tamper).
#   (3) an open-world class matches the digest but FAILS the domain gate ⇒ `T-hostile` (no laundering).
#   (4) an empty manifest ⇒ no pins ⇒ `T-hostile`.
#   (5) a malformed manifest is REJECTED fail-high ⇒ no pins ⇒ `T-hostile`.
set -uo pipefail

if [ "${IN_CONTAINER:-0}" != "1" ]; then
  REPO_ROOT="$(git rev-parse --show-toplevel)"
  echo "=== building release gatekeeperd + gate-probe (host) ==="
  ( cd "$REPO_ROOT" && cargo build --release -p gatekeeperd -p shrek-gate-probe ) || exit 3
  echo "=== launching privileged debian:trixie oracle ==="
  exec docker run --rm --privileged -e IN_CONTAINER=1 \
    -v "$REPO_ROOT/scripts/pin-manifest-proof.sh:/proof.sh:ro" \
    -v "$REPO_ROOT/target/release/gatekeeperd:/gatekeeperd:ro" \
    -v "$REPO_ROOT/target/release/gate-probe:/gate-probe:ro" \
    debian:trixie bash /proof.sh
fi

# ---------------- inside the container ----------------
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
apt-get install -y --no-install-recommends e2fsprogs util-linux >/dev/null 2>&1

GK=/gatekeeperd
GP=/gate-probe
fails=0
pass() { echo "PASS [$1]"; }
fail() { echo "FAIL [$1]"; [ -n "${2:-}" ] && sed 's/^/    /' "$2"; fails=$((fails + 1)); }

# --- provision a DEDICATED fs-verity-capable ext4 on a loopback (FAIL, never skip — amendment D) ---
IMG=/tmp/pinfs.img
MNT=/mnt/pinfs
dd if=/dev/zero of="$IMG" bs=1M count=64 status=none
# -b 4096: fs-verity requires the Merkle block size to equal the page size (4096); a small ext4
# otherwise defaults to 1K blocks and FS_IOC_ENABLE_VERITY returns EINVAL.
mkfs.ext4 -q -b 4096 -O verity "$IMG" || { echo "FAIL provision: mkfs.ext4 -O verity unavailable"; exit 2; }
# docker: /dev/loopN nodes may be absent even when privileged — pre-create a range (STM lesson).
for i in $(seq 0 63); do [ -e /dev/loop$i ] || mknod -m660 /dev/loop$i b 7 "$i"; done
mkdir -p "$MNT"
LOOP=$(losetup -f --show "$IMG") || { echo "FAIL provision: losetup"; exit 2; }
mount "$LOOP" "$MNT" || { echo "FAIL provision: mount"; exit 2; }

# The pinned artifact and a one-byte-different rogue. Make the rogue differ BEFORE enabling verity
# (an fs-verity file is immutable afterwards).
cp "$GP" "$MNT/pinned-probe"
cp "$GP" "$MNT/rogue-probe"
printf '\x00' >> "$MNT/rogue-probe"
"$GK" pin-verity enable "$MNT/pinned-probe" || { echo "FAIL provision: enable-verity (fs/kernel lacks verity)"; exit 2; }
"$GK" pin-verity enable "$MNT/rogue-probe" || { echo "FAIL provision: enable-verity rogue"; exit 2; }
DL=$("$GK" pin-verity measure "$MNT/pinned-probe") || { echo "FAIL provision: measure"; exit 2; }
ALGO=${DL%% *}; HEX=${DL##* }
echo "provisioned genuine fs-verity: pinned-probe = $ALGO $HEX"

# Write the sealed manifest at the REAL production path (container /usr is writable; the VM bakes it
# under dm-verity /usr at build time — same code path, same binary).
mkdir -p /usr/lib/shrek
manifest() { printf 'shrek-pin-manifest v1\n%s\n' "$1" > /usr/lib/shrek/pin-manifest; }

# Request T0/C-ro-nosec: (Pinned,RoNosec)=T0 and floor(Pinned)=T0, so a T-pinned band clears the
# re-check as Construct{T0} and hits the pin refusal guard (rather than a floor/caps refusal). For a
# T-hostile derivation the same request is a downgrade-below-floor(T2) refusal — we assert the DERIVED
# band on the always-printed SANDBOX-PROVENANCE line, not the rc, for those cases.
run() { OUT=$("$GK" sandbox --tier T0 --trust "$1" --caps C-ro-nosec --profile C-ro-nosec \
        --anchor /tmp --grant x -- "$2" 2>&1); RC=$?; printf '%s\n' "$OUT" > /tmp/out; }

echo "== (1) closed-world pin ⇒ T-pinned, fd bound, exec REFUSED fail-closed =="
manifest "$ALGO $HEX closed-world"
run T-pinned "$MNT/pinned-probe"
if grep -q 'derived=T-pinned' /tmp/out && grep -q 'pinned=true' /tmp/out && grep -q 'exec_fd_bound=true' /tmp/out; then
  pass "classification: derived=T-pinned pinned=true exec_fd_bound=true"
else fail "classification T-pinned" /tmp/out; fi
if [ "$RC" = 15 ] && grep -q 'reason=pinned-exec-home-unavailable' /tmp/out; then
  pass "execution refused deterministically (rc=15 pinned-exec-home-unavailable)"
else fail "exec refusal (rc=$RC)" /tmp/out; fi
if ! grep -q 'construct-at=' /tmp/out; then pass "no constructor ran (no up/down workaround)"; else fail "a constructor ran" /tmp/out; fi

echo "== (2) anti-spoof: one-byte-different verity inode ⇒ T-hostile =="
run T-pinned "$MNT/rogue-probe"
if grep -q 'derived=T-hostile' /tmp/out && grep -q 'pinned=false' /tmp/out; then
  pass "anti-spoof: unpinned second verity inode ⇒ T-hostile"
else fail "anti-spoof" /tmp/out; fi

echo "== (3) open-world class ⇒ T-hostile (no laundering) =="
manifest "$ALGO $HEX open-world"
run T-pinned "$MNT/pinned-probe"
if grep -q 'derived=T-hostile' /tmp/out && grep -q 'pinned=true' /tmp/out; then
  pass "open-world pin matches digest but domain gate fails ⇒ T-hostile"
else fail "open-world no-laundering" /tmp/out; fi

echo "== (4) empty manifest ⇒ no pins ⇒ T-hostile =="
manifest "# nothing pinned"
run T-pinned "$MNT/pinned-probe"
if grep -q 'derived=T-hostile' /tmp/out && grep -q 'pinned=false' /tmp/out; then
  pass "empty manifest ⇒ no pin ⇒ T-hostile"
else fail "empty manifest" /tmp/out; fi

echo "== (5) malformed manifest ⇒ fail-high (rejected, no pins) =="
printf 'shrek-pin-manifest v1\n%s garbage-extra-token\n' "$ALGO $HEX" > /usr/lib/shrek/pin-manifest
run T-pinned "$MNT/pinned-probe"
if grep -q 'MANIFEST REJECTED' /tmp/out && grep -q 'derived=T-hostile' /tmp/out; then
  pass "malformed manifest rejected fail-high ⇒ T-hostile"
else fail "malformed fail-high" /tmp/out; fi

echo
if [ "$fails" = 0 ]; then
  echo "=== PIN-MANIFEST ORACLE: ALL PASS ==="
else
  echo "=== PIN-MANIFEST ORACLE: $fails FAIL ==="; exit 1
fi
