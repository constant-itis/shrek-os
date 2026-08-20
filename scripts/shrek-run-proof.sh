#!/usr/bin/env bash
# Phase-6 front-door proof: the SAME real edit/build/execute coding session as p6-coder-proof.sh, but
# driven end-to-end through the ergonomic `shrek run` CLI instead of a hand-assembled `gatekeeperd
# sandbox …` line. It proves the front door is a faithful THIN COMPOSER: one short command yields the
# identical construct + write-through + wall behaviour, and it wires the previously-unexercised COMBINED
# cell (named egress + project RW + exec build in one sandbox). SPIKE-ONLY (strip before ship).
#
# Gates (G1–G5 mirror p6-coder-proof, now via `shrek run`; G6 is the new combined composition):
#   G1  `shrek run` (authentic harness) DERIVES T-untrust and constructs at GENUINE T2 — the front door
#       passes the real band inputs, the decision plane re-derives (construct-at=T2, harness_authentic).
#   G2  RW write-through across BOTH grants persists on the HOST (edited source + compiled ELF), and
#       teardown is non-destructive (pre-existing README survives) — through the CLI's grant mapping.
#   G3  the exec split holds: the compiled ELF runs from the exec build grant (exit=42) while the SAME
#       ELF copied into the (host-noexec) project grant CANNOT execute.
#   G4  the wall: ungranted vault ABSENT, host sentinel does not leak, loopback-only cell = NO egress.
#   G5  PG5 fail-closed FIDELITY: harness digest ABSENT ⇒ T-hostile ⇒ decision plane refuses; the front
#       door propagates the refuse exit code VERBATIM (exec), the workload never runs, nothing is written.
#   G6  COMBINED cell (new): `shrek run --egress github-https` builds a C-net cell that STILL realizes the
#       project RW + exec build grants (caps lattice: C-net ⊇ C-proj-rw). construct-at=T2, the coding loop
#       runs + persists, the egress path is selected, and a NON-listed destination (1.1.1.1:53) stays
#       DENIED — proving the allow-list is scoped, not open-net. (Live external REACH is the owner of
#       p6-egress-baremetal.sh, FORWARD-gated + opt-in; this gate proves composition, not reachability.)
set -uo pipefail

PIN_SHA256="670bcd3cbc103f00d8bb5098edc370f32397ee4c134231436bafa659bb3c068e"
PIN_URL="https://storage.googleapis.com/gvisor/releases/release/20260810.0/x86_64/runsc"

if [ "${IN_CONTAINER:-0}" != "1" ]; then
  REPO_ROOT="$(git rev-parse --show-toplevel)"
  echo "=== building gatekeeperd --features spike + shrek (host) ==="
  ( cd "$REPO_ROOT" && cargo build --release -p gatekeeperd --features spike && cargo build --release -p shrek ) || exit 3

  CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/shrek"; mkdir -p "$CACHE"
  RUNSC="$CACHE/runsc-20260810.0"
  if [ ! -f "$RUNSC" ] || [ "$(sha256sum "$RUNSC" | awk '{print $1}')" != "$PIN_SHA256" ]; then
    echo "=== fetching pinned runsc (release-20260810.0) ==="
    curl -fsSL -m 300 -o "$RUNSC" "$PIN_URL" || { echo "runsc fetch failed"; exit 3; }
  fi
  GOT="$(sha256sum "$RUNSC" | awk '{print $1}')"
  [ "$GOT" = "$PIN_SHA256" ] || { echo "PIN MISMATCH: $GOT != $PIN_SHA256"; exit 3; }
  echo "=== runsc pinned + verified ($PIN_SHA256) ==="
  chmod +x "$RUNSC"

  echo "=== launching privileged debian:trixie oracle ==="
  exec docker run --rm --privileged --cgroupns=private -e IN_CONTAINER=1 \
    -v "$REPO_ROOT/scripts/shrek-run-proof.sh:/proof.sh:ro" \
    -v "$REPO_ROOT/target/release/gatekeeperd:/gatekeeperd:ro" \
    -v "$REPO_ROOT/target/release/shrek:/shrek:ro" \
    -v "$RUNSC:/runsc-src:ro" \
    debian:trixie bash /proof.sh
fi

# ---------------- inside the container ----------------
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
# busybox/tcc = sandbox userland + real compiler; e2fsprogs -O verity = the fs-verity harness;
# iproute2 + nftables = the egress plane's `ip`/`nft` tools (G6's C-net cell; absent ⇒ ENOENT on spawn).
apt-get install -y --no-install-recommends busybox-static systemd ca-certificates e2fsprogs tcc iproute2 nftables >/dev/null 2>&1
mount --make-rshared / 2>/dev/null || true
GK=/gatekeeperd
SHREK=/shrek

pass(){ echo "  PASS $*"; }
fail(){ echo "  FAIL $*"; fails=$((fails+1)); }
fails=0

# --- fs-verity harness: verity-capable ext4 on a loopback; enable+measure the runsc harness via gatekeeperd. ---
IMG=/tmp/harness-verity.img; MNT=/mnt/harness
mkdir -p "$MNT"
dd if=/dev/zero of="$IMG" bs=1M count=256 status=none
mkfs.ext4 -q -b 4096 -O verity "$IMG" || { echo "FAIL provision: mkfs.ext4 -O verity unavailable"; exit 2; }
for i in $(seq 0 63); do [ -e /dev/loop$i ] || mknod -m660 /dev/loop$i b 7 "$i"; done
LOOP=$(losetup -f --show "$IMG") || { echo "FAIL provision: losetup"; exit 2; }
# Detach the loopback on EVERY exit path (privileged containers share the HOST's loop devices, so a
# skipped teardown leaks them host-wide and eventually exhausts /dev/loop*). Covers the provision-fail
# exits below, not just the happy-path teardown.
trap 'umount "$MNT" 2>/dev/null || true; losetup -d "$LOOP" 2>/dev/null || true' EXIT
mount -o exec "$LOOP" "$MNT" || { echo "FAIL provision: mount verity fs"; exit 2; }
cp /runsc-src "$MNT/runsc"; chmod +x "$MNT/runsc"
"$GK" pin-verity enable "$MNT/runsc" || { echo "FAIL provision: enable-verity on runsc"; exit 2; }
DL=$("$GK" pin-verity measure "$MNT/runsc") || { echo "FAIL provision: measure runsc"; exit 2; }
ALGO=${DL%% *}; HEX=${DL##* }
echo "provisioned genuine fs-verity harness: runsc = $ALGO $HEX"
RUNSC="$MNT/runsc"

ADMIT_OK=/tmp/ingest-admit.ok
ADMIT_BAD=/tmp/ingest-admit.bad
printf 'shrek-t2-ingest-admit v1\n# authorised T2 untrusted-ingest harness\n%s %s\n' "$ALGO" "$HEX" > "$ADMIT_OK"
printf 'shrek-t2-ingest-admit v1\n' > "$ADMIT_BAD"

# --- Minimal pinned sandbox rootfs (busybox + real freestanding tcc + closure), remounted RO to mirror
#     the sealed dm-verity /usr path. ---
ROOTFS=/rootfs; rm -rf "$ROOTFS"; mkdir -p "$ROOTFS/bin"
cp /usr/bin/busybox "$ROOTFS/bin/busybox"
for a in sh cat ls nc timeout echo test chmod cp; do ln -sf busybox "$ROOTFS/bin/$a"; done
install -D -m0755 /usr/bin/tcc "$ROOTFS/usr/bin/tcc"
[ -e /lib64/ld-linux-x86-64.so.2 ] && install -D -m0755 /lib64/ld-linux-x86-64.so.2 "$ROOTFS/lib64/ld-linux-x86-64.so.2"
for so in $(ldd /usr/bin/tcc | grep -oE '/[^ ]+\.so[^ ]*'); do install -D -m0755 "$so" "$ROOTFS$so"; done
mkdir -p "$ROOTFS/coder-src"
cat > "$ROOTFS/coder-src/hello.c" <<'CEOF'
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
mount --bind "$ROOTFS" "$ROOTFS" && mount -o remount,ro,bind "$ROOTFS" || echo "WARN: could not remount rootfs ro (oracle less faithful)"

# --- Anchor /srv + writable project (noexec) + writable build (exec) + ungranted vault + host sentinel. ---
rm -rf /srv; mkdir -p /srv/project /srv/build /srv/vault
echo "pre-existing-project-file" > /srv/project/README
echo "TOP-SECRET-DO-NOT-LEAK" > /srv/vault/secret
echo "HOST-ONLY" > /etc/shrek-host-sentinel

if [ -w /sys/fs/cgroup/cgroup.subtree_control ]; then
  mkdir -p /sys/fs/cgroup/init
  for p in $(cat /sys/fs/cgroup/cgroup.procs 2>/dev/null); do echo "$p" > /sys/fs/cgroup/init/cgroup.procs 2>/dev/null || true; done
  echo "+memory +pids" > /sys/fs/cgroup/cgroup.subtree_control 2>/dev/null || true
fi

# runshrek: drive the front door. gatekeeperd's spike env (SHREK_T2_*, SHREK_INGEST_ADMIT) is set here and
# inherited straight through `shrek`'s exec of gatekeeperd — the CLI adds no env of its own.
# Each call gets a FRESH per-run cgroup (shrek-run-N) as gatekeeperd's base, mirroring the per-invocation
# systemd scope production gives it — so gatekeeperd's cgroup-delegation dance starts from a clean base
# every time. (Reusing ONE base across constructs breaks the 2nd: once construct-1 enables subtree_control
# on it, a delegated parent can no longer hold the next gatekeeperd's process.)
runshrek(){ # $1=admit-list ; remaining args are `shrek run` args
  local admit="$1"; shift
  # Name the per-run cgroup by the unique sandbox --id (a shell counter would be lost — runshrek runs
  # inside OUT=$(...) command substitution, a subshell). A FRESH cgroup per run is required: once one
  # construct enables subtree_control on it, that cgroup becomes a delegated parent and can no longer
  # hold the next gatekeeperd's process (EBUSY → fallback to a controller-less cgroup → memory.max ENOENT).
  local id="run"; local a; local prev=""
  for a in "$@"; do [ "$prev" = "--id" ] && id="$a"; prev="$a"; done
  local cg="/sys/fs/cgroup/shrek-run-$id"; mkdir -p "$cg"
  cat > /run/gk-run.sh <<WRAP
#!/usr/bin/env bash
echo \$\$ > $cg/cgroup.procs 2>/dev/null || echo "  WARN: cgroup-place failed into $cg (gatekeeperd will fall back to its inherited cgroup)" >&2
exec env SHREK_GATEKEEPERD=/gatekeeperd SHREK_T2_RUNSC=$RUNSC SHREK_T2_ROOTFS=/rootfs SHREK_INGEST_ADMIT=$admit /shrek "\$@"
WRAP
  chmod +x /run/gk-run.sh
  /run/gk-run.sh "$@" 2>&1
}

# The same coding loop as p6-coder-proof (edit → tcc compile to build grant → run from build → prove the
# same ELF cannot run from the noexec project → probe the wall). The 1.1.1.1:53 egress probe is OUTSIDE
# any allow-list, so it must be denied in BOTH the loopback-only (G1) and the C-net (G6) cells.
LOOP='echo PROBE-START; PATH=/usr/bin:/bin:/sbin; export PATH; cd /srv/project || { echo NO-PROJECT; exit 40; }; cp /coder-src/hello.c ./hello.c && echo "/* edit-9f3a */" >> ./hello.c && echo EDIT-WROTE-OK; if /usr/bin/tcc -nostdlib -static -o /srv/build/hello ./hello.c 2>/srv/build/tcc.err; then echo TCC-COMPILE-OK; BO=$(/srv/build/hello); echo "BUILD-RAN out=[$BO] exit=$?"; else echo TCC-COMPILE-FAIL; cat /srv/build/tcc.err; fi; cp /srv/build/hello ./proj-copy 2>/dev/null; PJ=$(./proj-copy 2>&1); case "$PJ" in *REAL-COMPILE-RUN-OK*) echo PROJECT-EXEC-LEAK-BAD;; *) echo PROJECT-NOEXEC-ENFORCED-OK;; esac; if [ -e /srv/vault ]; then echo VAULT-VISIBLE-BAD; else echo VAULT-ABSENT-OK; fi; if [ -e /etc/shrek-host-sentinel ]; then echo HOST-FS-LEAK-BAD; else echo HOST-FS-ISOLATED-OK; fi; if timeout 5 nc -w 2 1.1.1.1 53 </dev/null >/dev/null 2>&1; then echo EGRESS-REACHED-BAD; else echo NO-EGRESS-OK; fi; echo PROBE-END'

echo
echo "=== G1–G4: shrek run (authentic harness) ⇒ T-untrust ⇒ T2 RW coding session ==="
rm -f /srv/project/hello.c /srv/project/proj-copy /srv/build/hello /srv/build/tcc.err
OUT=$(runshrek "$ADMIT_OK" run --project /srv/project --build /srv/build --id p6a -- /bin/sh -c "$LOOP")
echo "$OUT" | sed 's/^/    /'
echo "$OUT" | grep -q "mode=ingest-harness derived=T-untrust"        && pass "G1 shrek run ⇒ derived=T-untrust" || fail "G1 derived=T-untrust"
echo "$OUT" | grep -q "harness_authentic=true"                       && pass "G1 harness_authentic=true"       || fail "G1 harness_authentic"
echo "$OUT" | grep -q "construct-at=T2 effective=T2"                 && pass "G1 construct-at=T2"              || fail "G1 construct-at=T2"
echo "$OUT" | grep -q "TCC-COMPILE-OK"                               && pass "REAL-COMPILE via CLI"           || fail "REAL-COMPILE"
echo "$OUT" | grep -q "BUILD-RAN out=\[REAL-COMPILE-RUN-OK\] exit=42" && pass "G3 compiled ELF ran from build grant (exit=42)" || fail "G3 build-exec"
echo "$OUT" | grep -q "PROJECT-NOEXEC-ENFORCED-OK"                    && pass "G3 project noexec ENFORCED"     || fail "G3 project-noexec"
echo "$OUT" | grep -q "PROJECT-EXEC-LEAK-BAD"                         && fail "G3 PROJECT-EXEC-LEAK"           || pass "G3 no project-exec leak"
echo "$OUT" | grep -q "EDIT-WROTE-OK"                                && pass "G2 workload wrote into project grant" || fail "G2 workload-wrote"
{ grep -q "edit-9f3a" /srv/project/hello.c 2>/dev/null && head -c4 /srv/build/hello 2>/dev/null | grep -qa ELF; } \
  && pass "G2 write-through: edited source + compiled ELF on host" || fail "G2 write-through-to-host"
grep -q "pre-existing-project-file" /srv/project/README 2>/dev/null \
  && pass "G2 teardown non-destructive: README intact" || fail "G2 teardown DESTROYED content"
echo "$OUT" | grep -q "VAULT-ABSENT-OK"                              && pass "G4 vault-absent(ENOENT)"        || fail "G4 vault-absent"
echo "$OUT" | grep -q "HOST-FS-ISOLATED-OK"                          && pass "G4 host-fs-isolated"            || fail "G4 host-fs-isolated"
echo "$OUT" | grep -q "NO-EGRESS-OK"                                 && pass "G4 no-egress(loopback-only)"    || fail "G4 no-egress"
echo "$OUT" | grep -q "VAULT-VISIBLE-BAD\|HOST-FS-LEAK-BAD\|EGRESS-REACHED-BAD" && fail "G4 LEAK DETECTED" || pass "G4 no-leak-markers"

echo
echo "=== G5: harness digest ABSENT ⇒ T-hostile ⇒ shrek run propagates the fail-closed refuse VERBATIM ==="
OUT5=$(runshrek "$ADMIT_BAD" run --project /srv/project --build /srv/build --id p6b -- /bin/sh -c 'echo LEAKED > /srv/project/leak.txt; echo LEAKED'); rc5=$?
echo "$OUT5" | grep -q "mode=ingest-harness derived=T-hostile"       && pass "G5 no-admit ⇒ derived=T-hostile" || fail "G5 derived=T-hostile"
{ { [ $rc5 -eq 10 ] || [ $rc5 -eq 12 ]; } && echo "$OUT5" | grep -q "SANDBOX-DECISION refused"; } \
  && pass "G5 fail-closed rc=$rc5 propagated by shrek (PG5 fidelity)" || fail "G5 fail-closed rc=$rc5"
echo "$OUT5" | grep -q "construct-at=" && fail "G5 constructed (should refuse!)" || pass "G5 never-constructed"
echo "$OUT5" | grep -q "LEAKED" && fail "G5 workload-ran (LEAK!)"    || pass "G5 workload-never-ran"
[ -e /srv/project/leak.txt ] && fail "G5 workload wrote to host (LEAK!)" || pass "G5 no-host-write"

echo
echo "=== G6: shrek run --egress github-https ⇒ COMBINED C-net + project RW + exec build in one cell ==="
# Pre-resolve the sealed profile's hosts so the construction-time getaddrinfo succeeds (net_plane resolves,
# it does not reach). Live external REACH is p6-egress-baremetal.sh's job — here we prove composition.
for h in github.com codeload.github.com objects.githubusercontent.com; do echo "127.0.0.1 $h" >> /etc/hosts; done
rm -f /srv/project/hello.c /srv/project/proj-copy /srv/build/hello /srv/build/tcc.err
echo "pre-existing-project-file" > /srv/project/README
OUT6=$(runshrek "$ADMIT_OK" run --project /srv/project --build /srv/build --egress github-https --id p6c -- /bin/sh -c "$LOOP")
echo "$OUT6" | sed 's/^/    /'
echo "$OUT6" | grep -q "construct-at=T2 effective=T2"               && pass "G6 combined cell constructs at T2" || fail "G6 construct-at=T2"
echo "$OUT6" | grep -q "netstack --network=sandbox"                 && pass "G6 egress path selected (netstack)" || fail "G6 egress-path"
echo "$OUT6" | grep -q "BUILD-RAN out=\[REAL-COMPILE-RUN-OK\] exit=42" && pass "G6 coding loop ran in the C-net cell (RW build+exec intact)" || fail "G6 coding-loop"
{ grep -q "edit-9f3a" /srv/project/hello.c 2>/dev/null && head -c4 /srv/build/hello 2>/dev/null | grep -qa ELF; } \
  && pass "G6 write-through persists under C-net (project + build on host)" || fail "G6 write-through"
echo "$OUT6" | grep -q "PROJECT-NOEXEC-ENFORCED-OK"                 && pass "G6 project noexec still enforced under C-net" || fail "G6 project-noexec"
echo "$OUT6" | grep -q "VAULT-ABSENT-OK"                            && pass "G6 vault still absent under C-net" || fail "G6 vault-absent"
echo "$OUT6" | grep -q "NO-EGRESS-OK"                               && pass "G6 non-listed dst (1.1.1.1:53) DENIED ⇒ allow-list scoped, not open-net" || fail "G6 allow-list-not-open"

# --- teardown ---
umount "$MNT" 2>/dev/null || true; losetup -d "$LOOP" 2>/dev/null || true

echo
if [ $fails -eq 0 ]; then echo "SHREK-RUN-PROOF: ALL PASS ✅"; exit 0; else echo "SHREK-RUN-PROOF: $fails FAIL ❌"; exit 1; fi
