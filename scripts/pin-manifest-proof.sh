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
# slice-9 (T0 exec island) extends the same oracle:
#   (6) a closed-world pinned STATIC PIE now RUNS from a per-inode exec island at T0, while every
#       mutable grant stays MS_NOEXEC — proven by the pinned workload's own mmap(PROT_EXEC)+execve of a
#       mutable grant both returning EPERM (the mmap case is the load-bearing no-laundering control).
#   (7) a rogue (unpinned) verity inode ⇒ T-hostile ⇒ no island is constructed and it never runs.
#   (8) a DYNAMIC (PT_INTERP) pinned binary is rejected — island construction fails closed (Fork A:
#       static-PIE only; no dynamic-loader/library closure is authenticated in v1).
set -uo pipefail

if [ "${IN_CONTAINER:-0}" != "1" ]; then
  REPO_ROOT="$(git rev-parse --show-toplevel)"
  echo "=== building release gatekeeperd + gate-probe (host) ==="
  # The pinned entrypoint MUST be a static PIE (slice-9 Fork A) — build gate-probe with crt-static so
  # it has NO PT_INTERP. Keep a DYNAMIC copy too (default build = PT_INTERP) to prove the static-PIE
  # gate fails closed on a dynamically-linked pin (section 8).
  # gatekeeperd built with `--features spike`: this oracle drives `pin-verity enable/measure` to mint
  # fs-verity fixtures. That surface is default-OFF in production (finding F1); the oracle opts in.
  ( cd "$REPO_ROOT" \
      && cargo build --release -p gatekeeperd --features spike \
      && cargo build --release -p shrek-gate-probe \
      && cp target/release/gate-probe target/release/gate-probe-dyn \
      && RUSTFLAGS='-C link-arg=-Wl,-rpath,$ORIGIN/lib -C link-arg=-Wl,--disable-new-dtags' cargo build --release -p shrek-gate-probe \
      && cp target/release/gate-probe target/release/gate-probe-dynrp \
      && RUSTFLAGS="-C target-feature=+crt-static" cargo build --release -p shrek-gate-probe ) || exit 3
  echo "=== launching privileged debian:trixie oracle ==="
  exec docker run --rm --privileged --cgroupns=private -e IN_CONTAINER=1 \
    -v "$REPO_ROOT/scripts/pin-manifest-proof.sh:/proof.sh:ro" \
    -v "$REPO_ROOT/target/release/gatekeeperd:/gatekeeperd:ro" \
    -v "$REPO_ROOT/target/release/gate-probe:/gate-probe:ro" \
    -v "$REPO_ROOT/target/release/gate-probe-dyn:/gate-probe-dyn:ro" \
    -v "$REPO_ROOT/target/release/gate-probe-dynrp:/gate-probe-dynrp:ro" \
    debian:trixie bash /proof.sh
fi

# ---------------- inside the container ----------------
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
apt-get install -y --no-install-recommends e2fsprogs util-linux binutils >/dev/null 2>&1
mount --make-rshared / 2>/dev/null || true

# cgroup-v2 delegation for the slice-9 exec-island CONSTRUCTION (slice-8's refusal path never built,
# so this is new here). gatekeeperd moves ITSELF into a `_daemon` leaf then creates the per-sandbox
# leaf beside it — a dance that needs its base cgroup to contain ONLY gatekeeperd. So (a) evacuate the
# container root into `init`, (b) delegate memory+pids from the root, and (c) launch each island
# gatekeeperd into a FRESH empty `shrek-gk-<pid>` base via the wrapper below (one per invocation, so
# repeated island runs never collide on a base that already has subtree_control enabled).
if [ -w /sys/fs/cgroup/cgroup.subtree_control ]; then
  mkdir -p /sys/fs/cgroup/init
  for p in $(cat /sys/fs/cgroup/cgroup.procs 2>/dev/null); do
    echo "$p" > /sys/fs/cgroup/init/cgroup.procs 2>/dev/null || true
  done
  echo "+memory +pids" > /sys/fs/cgroup/cgroup.subtree_control 2>/dev/null \
    && echo "  cgroup: root delegates memory+pids" \
    || echo "  [warn] cgroup root subtree_control not writable — island cgroup step will fail closed"
fi
cat > /run/gk-isl.sh <<'EOF'
#!/bin/sh
B=/sys/fs/cgroup/shrek-gk-$$
mkdir -p "$B"
echo $$ > "$B/cgroup.procs" 2>/dev/null || true
exec /gatekeeperd "$@"
EOF
chmod +x /run/gk-isl.sh

GK=/gatekeeperd
GKISL=/run/gk-isl.sh
GP=/gate-probe
GPDYN=/gate-probe-dyn
GPDYNRP=/gate-probe-dynrp
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
# re-check as Construct{T0}. A closed-world static-PIE pin then constructs the exec island; a
# T-hostile derivation is instead a downgrade-below-floor(T2) refusal — we assert the DERIVED band on
# the always-printed SANDBOX-PROVENANCE line, not the rc, for those refusal cases.
run() { OUT=$("$GK" sandbox --tier T0 --trust "$1" --caps C-ro-nosec --profile C-ro-nosec \
        --anchor /tmp --grant x -- "$2" 2>&1); RC=$?; printf '%s\n' "$OUT" > /tmp/out; }

# slice-9 exec-island driver. A mutable grant: a real +x executable on an ORDINARY (writable,
# non-verity) fs, so the ONLY thing that can stop execve/mmap(PROT_EXEC) of it is the sandbox's
# MS_NOEXEC bind (I2/I5), not file perms. Driven through the cgroup wrapper (genuine T0 construction).
GDIR=/tmp/isl; mkdir -p "$GDIR"; cp "$GP" "$GDIR/evil"; chmod +x "$GDIR/evil"
island_run() { OUT=$("$GKISL" sandbox --tier T0 --trust T-pinned --caps C-ro-nosec --profile C-ro-nosec \
        --anchor "$GDIR" --grant evil -- "$1" island "$GDIR/evil" 2>&1); RC=$?; printf '%s\n' "$OUT" > /tmp/out; }

echo "== (1) EXEC ISLAND: closed-world pinned static-PIE ⇒ T-pinned AND RUNS; mutable grant NOEXEC =="
manifest "$ALGO $HEX closed-world"
island_run "$MNT/pinned-probe"
if grep -q 'derived=T-pinned' /tmp/out && grep -q 'pinned=true' /tmp/out && grep -q 'exec_fd_bound=true' /tmp/out; then
  pass "classification: derived=T-pinned pinned=true exec_fd_bound=true"
else fail "classification T-pinned" /tmp/out; fi
if grep -q 'construct-at=T0 island=exec' /tmp/out; then pass "island constructor ran at T0 (exec surface opened for exactly one inode)"; else fail "island route not taken (rc=$RC)" /tmp/out; fi
if grep -q 'SHREK_GATE: PASS gate=island-ran' /tmp/out; then pass "pinned static-PIE executed from the re-verified exec island"; else fail "pinned bytes did not run" /tmp/out; fi
# F2 (docs/phase5-consolidation.md §2): the writable island root is MS_NOEXEC while the entry bind laid
# on top is independently exec-capable. island-ran above is the POSITIVE proof (the pin could not run if
# the member bind were noexec under the sealed parent); this SANDBOX-ISLAND-FLAGS line is the DIRECT
# statfs proof (root noexec + entry not-noexec), emitted only after the fail-closed self-check passes.
if grep -q 'SANDBOX-ISLAND-FLAGS parent-noexec=1 members-exec-ok=1' /tmp/out; then pass "F2: island root NOEXEC, entry mount independently exec-capable (statfs self-check)"; else fail "F2 island-flags self-check absent" /tmp/out; fi
if grep -q 'SHREK_GATE: PASS gate=island-grant-mmap-exec-eperm' /tmp/out; then pass "mutable grant mmap(PROT_EXEC) ⇒ EPERM (MS_NOEXEC blocks library-load laundering)"; else fail "grant mmap-exec not EPERM" /tmp/out; fi
if grep -q 'SHREK_GATE: PASS gate=island-grant-execve-denied' /tmp/out; then pass "mutable grant execve ⇒ denied (NOEXEC + Landlock no-EXECUTE)"; else fail "grant execve not denied" /tmp/out; fi

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

# ================================ slice-9: T0 exec island ========================================
# (section 1 above is the positive island run; these two are the negative/fail-closed island cases.)

echo "== (6) rogue inode: an unpinned verity inode never reaches the island =="
manifest "$ALGO $HEX closed-world"   # section 5 left it malformed; restore the valid closed-world pin
island_run "$MNT/rogue-probe"
if grep -q 'derived=T-hostile' /tmp/out && ! grep -q 'island=exec' /tmp/out && ! grep -q 'gate=island-ran' /tmp/out; then
  pass "rogue (unpinned) inode ⇒ T-hostile, no island constructed, pinned bytes never ran"
else fail "rogue reached the island" /tmp/out; fi

echo "== (7) fail-closed: a DYNAMIC (PT_INTERP) pinned binary is rejected — static-PIE only (Fork A) =="
cp "$GPDYN" "$MNT/dyn-probe"
"$GK" pin-verity enable "$MNT/dyn-probe" >/dev/null || { echo "FAIL provision: enable-verity dyn"; exit 2; }
DDL=$("$GK" pin-verity measure "$MNT/dyn-probe"); DALGO=${DDL%% *}; DHEX=${DDL##* }
manifest "$DALGO $DHEX closed-world"
island_run "$MNT/dyn-probe"
# derived=T-pinned (digest matches, closed-world class) and the island ROUTE is entered, but the
# static-PIE gate rejects PT_INTERP ⇒ construction fails closed, workload never runs, rc!=0.
if grep -q 'derived=T-pinned' /tmp/out && ! grep -q 'gate=island-ran' /tmp/out && [ "$RC" != 0 ]; then
  pass "dynamic pin ⇒ island construction fails closed, pinned bytes never ran (rc=$RC)"
else fail "dynamic pin not fail-closed" /tmp/out; fi

# ============================= slice-10: sealed-dynamic closure island ===========================
# What only a real fs-verity run can show for slice-10: a DYNAMICALLY-linked pinned entrypoint, with an
# authenticated closure (pinned interpreter + every transitive DT_NEEDED library, each identity-pinned
# by fs-verity digest), RUNS from an N-inode island — the loader resolves every object to a re-verified
# pinned inode, while /usr and every mutable grant are MS_NOEXEC so no non-member byte can be mapped.

# Deliver the closure flat in one dir on the verity fs. The entrypoint carries DT_RPATH $ORIGIN/lib
# (transitive), so the pinned loader resolves every library from the island lib dir.
DP="$MNT/dynpkg"; mkdir -p "$DP"
cp "$GPDYNRP" "$DP/dyn-probe"
INTERP=$(readelf -l "$DP/dyn-probe" 2>/dev/null | sed -n 's/.*program interpreter: \([^]]*\)\].*/\1/p')
LIBS=$(ldd "$DP/dyn-probe" | awk '/=>/ && $3 ~ /^\//{print $3}')
if [ -z "$INTERP" ] || [ -z "$LIBS" ]; then echo "FAIL dyn: could not enumerate closure (interp=$INTERP)"; exit 2; fi
cp "$INTERP" "$DP/$(basename "$INTERP")"
for L in $LIBS; do cp "$L" "$DP/$(basename "$L")"; done
# enable is best-effort (idempotent: EEXIST when already verity on a re-bake); measure is the real gate.
em() { "$GK" pin-verity enable "$1" >/dev/null 2>&1; "$GK" pin-verity measure "$1" || { echo "FAIL dyn: measure $1"; exit 2; }; }
build_closure_manifest() {  # $1 = interp digest override (empty = real); writes the sealed v2 manifest
  { printf 'shrek-pin-manifest v2\n'
    printf 'entry %s closed-world\n' "$(em "$DP/dyn-probe")"
    if [ -n "$1" ]; then printf 'interp sha256 %s %s\n' "$1" "$INTERP"; else printf 'interp %s %s\n' "$(em "$DP/$(basename "$INTERP")")" "$INTERP"; fi
    for L in $LIBS; do SO=$(basename "$L"); printf 'lib %s %s\n' "$(em "$DP/$SO")" "$SO"; done
  } > /usr/lib/shrek/pin-manifest
}
dyn_run() { OUT=$("$GKISL" sandbox --tier T0 --trust T-pinned --caps C-ro-nosec --profile C-ro-nosec \
        --anchor "$GDIR" --grant evil -- "$DP/dyn-probe" island "$GDIR/evil" 2>&1); RC=$?; printf '%s\n' "$OUT" > /tmp/out; }

echo "== (8) SEALED-DYNAMIC: closed-world pinned DYNAMIC closure ⇒ T-pinned AND RUNS from the N-inode island =="
build_closure_manifest ""
dyn_run
if grep -q 'derived=T-pinned' /tmp/out && grep -q 'pinned=true' /tmp/out && grep -q 'exec_fd_bound=true' /tmp/out; then
  pass "dynamic classification: derived=T-pinned pinned=true exec_fd_bound=true"
else fail "dynamic classification" /tmp/out; fi
if grep -q 'construct-at=T0 island=closure' /tmp/out; then pass "closure island route taken at T0 (N-inode)"; else fail "closure route not taken (rc=$RC)" /tmp/out; fi
if grep -q 'SHREK_GATE: PASS gate=island-ran' /tmp/out; then pass "pinned DYNAMIC entrypoint executed — full closure (interp + libs) resolved to pinned inodes"; else fail "dynamic pin did not run" /tmp/out; fi
# F2: island root NOEXEC while entry + every closure member (lib) bind is independently exec-capable.
# island-ran is the positive proof (ld.so mmap(PROT_EXEC)-loaded the pinned libs under the sealed
# parent); SANDBOX-ISLAND-FLAGS is the direct statfs proof (root noexec, entry+libs not-noexec).
if grep -q 'SANDBOX-ISLAND-FLAGS parent-noexec=1 members-exec-ok=1' /tmp/out; then pass "F2: island root NOEXEC, all closure-member mounts independently exec-capable (statfs self-check)"; else fail "F2 island-flags self-check absent under closure" /tmp/out; fi
if grep -q 'SHREK_GATE: PASS gate=island-grant-mmap-exec-eperm' /tmp/out; then pass "under the closure island, a mutable-grant mmap(PROT_EXEC) still ⇒ EPERM (non-member NOEXEC)"; else fail "grant mmap-exec not EPERM under closure" /tmp/out; fi

echo "== (9) fail-closed: a TAMPERED closure member (wrong interp digest) ⇒ construction fails, bytes never run =="
build_closure_manifest "0000000000000000000000000000000000000000000000000000000000000000"
dyn_run
# derived=T-pinned (entry digest still matches), closure route entered, but the interp re-measure !=
# manifest digest ⇒ relocate_member fails closed ⇒ island never completes, workload never runs.
if grep -q 'derived=T-pinned' /tmp/out && ! grep -q 'gate=island-ran' /tmp/out && [ "$RC" != 0 ]; then
  pass "tampered closure member ⇒ island construction fails closed, pinned bytes never ran (rc=$RC)"
else fail "tampered closure member not fail-closed" /tmp/out; fi

echo
if [ "$fails" = 0 ]; then
  echo "=== PIN-MANIFEST ORACLE: ALL PASS ==="
else
  echo "=== PIN-MANIFEST ORACLE: $fails FAIL ==="; exit 1
fi
