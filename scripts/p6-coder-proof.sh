#!/usr/bin/env bash
# Phase-6 slice-1a — the CODING-AGENT vertical-slice proof: an integrity-bound untrusted-ingest session
# runs a real edit/build/execute loop in a GENUINE T2 (gVisor/runsc) sandbox with a WRITABLE, write-
# through project grant, over a real fs-verity harness, in a privileged debian:trixie oracle (the fast
# path before the ~35-min sealed-VM gate). SPIKE-ONLY (strip before ship). Sibling of t2-construct-proof.sh.
#
# What only a real run can show (unit tests cover the pure parser/lattice + config shape):
#   G1  integrity-bound admission: with runsc's fs-verity digest in the sealed admit-list, an
#       --ingest-harness session DERIVES T-untrust (not the T-hostile floor) and a C-proj-rw cell
#       constructs at GENUINE T2 (construct-at=T2, derived=T-untrust, harness_authentic=true).
#   G2  RW write-through across BOTH grants: the edited source under /srv/project and the compiled ELF
#       under /srv/build are visible ON THE HOST afterwards (the real inodes were mutated — PN1 write-back,
#       direct mode), and teardown is non-destructive (pre-existing README survives).
#   G3  the coding loop runs, with the exec split the real-ELF result forced (owner build-grant design,
#       superseding the earlier shell-script G3): a real ELF compiled from project source EXECUTES from the
#       exec-capable /srv/build grant (BUILD-RAN … REAL-COMPILE-RUN-OK exit=42), while the SAME ELF copied
#       into the /srv/project grant CANNOT execute (PROJECT-NOEXEC-ENFORCED) — because gVisor must
#       mmap(PROT_EXEC) the gofer file and the project's MS_NOEXEC host mount denies that. The exec surface
#       is confined to the one build grant; NOSUID|NODEV preserved on both. (The prior shell-script G3
#       "in-sandbox exec OK" was a #!/bin/sh stand-in that never needed PROT_EXEC, so it never tested this.)
#   G4  the wall: the ungranted vault is ABSENT (ENOENT), a host sentinel does NOT leak, --network=none
#       yields NO egress — unchanged from slice-6, re-asserted for the RW/ingest session.
#   G5  fail-closed: with the harness digest ABSENT from the admit-list, the SAME request derives
#       T-hostile ⇒ (T-hostile,C-proj-rw)=T3 ⇒ FAILS CLOSED (no constructor) and the workload never runs.
set -uo pipefail

PIN_SHA256="670bcd3cbc103f00d8bb5098edc370f32397ee4c134231436bafa659bb3c068e"
PIN_URL="https://storage.googleapis.com/gvisor/releases/release/20260810.0/x86_64/runsc"

if [ "${IN_CONTAINER:-0}" != "1" ]; then
  REPO_ROOT="$(git rev-parse --show-toplevel)"
  echo "=== building gatekeeperd --features spike (host) [pin-verity enable/measure provisions the harness verity] ==="
  ( cd "$REPO_ROOT" && cargo build --release -p gatekeeperd --features spike ) || exit 3

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
    -v "$REPO_ROOT/scripts/p6-coder-proof.sh:/proof.sh:ro" \
    -v "$REPO_ROOT/target/release/gatekeeperd:/gatekeeperd:ro" \
    -v "$RUNSC:/runsc-src:ro" \
    debian:trixie bash /proof.sh
fi

# ---------------- inside the container ----------------
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
# busybox-static = sandbox userland; systemd = systemd-detect-virt (virt gate); e2fsprogs = mkfs.ext4
# -O verity (the fs-verity-capable harness fs); tcc = the sealed freestanding C compiler (real build).
# No containerd/shim — runsc drives directly.
apt-get install -y --no-install-recommends busybox-static systemd ca-certificates e2fsprogs tcc >/dev/null 2>&1
mount --make-rshared / 2>/dev/null || true
GK=/gatekeeperd

pass(){ echo "  PASS $*"; }
fail(){ echo "  FAIL $*"; fails=$((fails+1)); }
fails=0

# --- Provision a DEDICATED fs-verity ext4 on a loopback and enable verity on the runsc HARNESS (FAIL,
#     never skip). This is the integrity anchor of the admission: gatekeeperd measures THIS inode. ---
IMG=/tmp/harness-verity.img; MNT=/mnt/harness
mkdir -p "$MNT"
dd if=/dev/zero of="$IMG" bs=1M count=256 status=none
mkfs.ext4 -q -b 4096 -O verity "$IMG" || { echo "FAIL provision: mkfs.ext4 -O verity unavailable"; exit 2; }
for i in $(seq 0 63); do [ -e /dev/loop$i ] || mknod -m660 /dev/loop$i b 7 "$i"; done
LOOP=$(losetup -f --show "$IMG") || { echo "FAIL provision: losetup"; exit 2; }
mount -o exec "$LOOP" "$MNT" || { echo "FAIL provision: mount verity fs"; exit 2; }
cp /runsc-src "$MNT/runsc"; chmod +x "$MNT/runsc"
"$GK" pin-verity enable "$MNT/runsc" || { echo "FAIL provision: enable-verity on runsc (fs/kernel lacks verity)"; exit 2; }
DL=$("$GK" pin-verity measure "$MNT/runsc") || { echo "FAIL provision: measure runsc"; exit 2; }
ALGO=${DL%% *}; HEX=${DL##* }
echo "provisioned genuine fs-verity harness: runsc = $ALGO $HEX"
RUNSC="$MNT/runsc"

# The sealed admit-list (SHREK_INGEST_ADMIT is the ORACLE relocation of the /usr default, like SHREK_T2_*).
ADMIT_OK=/tmp/ingest-admit.ok
ADMIT_BAD=/tmp/ingest-admit.bad
printf 'shrek-t2-ingest-admit v1\n# authorised T2 untrusted-ingest harness\n%s %s\n' "$ALGO" "$HEX" > "$ADMIT_OK"
# The fail-closed list: header only (admits nothing) — a well-formed but non-admitting sealed policy.
printf 'shrek-t2-ingest-admit v1\n' > "$ADMIT_BAD"

# --- Minimal pinned sandbox rootfs (busybox applets). Throwaway (SHREK_T2_ROOTFS override, spike-only).
#     Mirrors seal-t2-artifacts.sh: busybox applets + tcc + its dynamic closure + the freestanding source
#     template, so this oracle exercises the SAME real-compiler rootfs the sealed VM gate does. ---
ROOTFS=/rootfs; rm -rf "$ROOTFS"; mkdir -p "$ROOTFS/bin"
cp /usr/bin/busybox "$ROOTFS/bin/busybox"
for a in sh cat ls nc timeout echo test chmod cp; do ln -sf busybox "$ROOTFS/bin/$a"; done
# The real freestanding C compiler + its closure. -nostdlib -static needs no libtcc1.a / includes, only
# tcc + its interpreter + libc/libm (verified). install dereferences SONAME symlinks to regular files.
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
# Reproduce the SEALED condition: on the image the rootfs is on read-only dm-verity /usr; bind+remount-ro
# so this oracle exercises the same gofer-EROFS path the sealed VM does (construct copies rootfs to /run).
mount --bind "$ROOTFS" "$ROOTFS" && mount -o remount,ro,bind "$ROOTFS" || echo "WARN: could not remount rootfs ro (oracle less faithful)"

# --- Anchor + WRITABLE project (source, noexec) + WRITABLE build area (exec) + ungranted vault; a host
#     sentinel to prove the host FS does not leak in. ---
rm -rf /srv; mkdir -p /srv/project /srv/build /srv/vault
echo "pre-existing-project-file" > /srv/project/README
echo "TOP-SECRET-DO-NOT-LEAK" > /srv/vault/secret
echo "HOST-ONLY" > /etc/shrek-host-sentinel

# --- cgroup-v2 delegation for the oracle (same dance as t2-construct-proof). ---
if [ -w /sys/fs/cgroup/cgroup.subtree_control ]; then
  mkdir -p /sys/fs/cgroup/init /sys/fs/cgroup/shrek-gk
  for p in $(cat /sys/fs/cgroup/cgroup.procs 2>/dev/null); do echo "$p" > /sys/fs/cgroup/init/cgroup.procs 2>/dev/null || true; done
  echo "+memory +pids" > /sys/fs/cgroup/cgroup.subtree_control 2>/dev/null || true
fi
runsandbox(){ # $1=admit-list  $2=id ; remaining args after -- are the sandbox args
  local admit="$1" id="$2"; shift 2
  cat > /run/gk-run.sh <<WRAP
#!/usr/bin/env bash
echo \$\$ > /sys/fs/cgroup/shrek-gk/cgroup.procs 2>/dev/null || true
exec env SHREK_T2_RUNSC=$RUNSC SHREK_T2_ROOTFS=/rootfs SHREK_INGEST_ADMIT=$admit /gatekeeperd "\$@"
WRAP
  chmod +x /run/gk-run.sh
  /run/gk-run.sh sandbox "$@" 2>&1
}

# The in-sandbox coding loop, per the owner build-grant design: EDIT a real .c into the project grant
# (noexec, write-through) + COMPILE it with the sealed tcc (freestanding -nostdlib -static → static ELF,
# no libc in the guest) directing OUTPUT to the exec-capable /srv/build grant + EXECUTE it from there
# (works) + prove that the SAME ELF copied into the noexec project CANNOT execute (the wall) + probe the
# rest of the wall. Single line so there are no newline-in-JSON surprises (json_escape handles it anyway).
# The compiled program prints REAL-COMPILE-RUN-OK and exits 42; project-exec is judged on the run MARKER
# (not the exit code — the ELF's own success exit is 42, itself nonzero), so a leak cannot hide.
LOOP='echo PROBE-START; PATH=/usr/bin:/bin:/sbin; export PATH; cd /srv/project || { echo NO-PROJECT; exit 40; }; cp /coder-src/hello.c ./hello.c && echo "/* edit-9f3a */" >> ./hello.c && echo EDIT-WROTE-OK; if /usr/bin/tcc -nostdlib -static -o /srv/build/hello ./hello.c 2>/srv/build/tcc.err; then echo TCC-COMPILE-OK; BO=$(/srv/build/hello); echo "BUILD-RAN out=[$BO] exit=$?"; else echo TCC-COMPILE-FAIL; cat /srv/build/tcc.err; fi; cp /srv/build/hello ./proj-copy 2>/dev/null; PJ=$(./proj-copy 2>&1); case "$PJ" in *REAL-COMPILE-RUN-OK*) echo PROJECT-EXEC-LEAK-BAD;; *) echo PROJECT-NOEXEC-ENFORCED-OK;; esac; if [ -e /srv/vault ]; then echo VAULT-VISIBLE-BAD; else echo VAULT-ABSENT-OK; fi; if [ -e /etc/shrek-host-sentinel ]; then echo HOST-FS-LEAK-BAD; else echo HOST-FS-ISOLATED-OK; fi; if timeout 5 nc -w 2 1.1.1.1 53 </dev/null >/dev/null 2>&1; then echo EGRESS-REACHED-BAD; else echo NO-EGRESS-OK; fi; echo PROBE-END'

echo
echo "=== G1/G2/G3/G4: authentic harness ⇒ T-untrust ⇒ T2 RW coding session ==="
rm -f /srv/project/hello.c /srv/project/proj-copy /srv/build/hello /srv/build/tcc.err
OUT=$(runsandbox "$ADMIT_OK" p6a --tier T2 --trust T-hostile --caps C-proj-rw --profile C-proj-rw \
        --ingest-harness --id p6a --anchor /srv --rw-grant project --build-grant build -- /bin/sh -c "$LOOP")
echo "$OUT" | sed 's/^/    /'
echo "$OUT" | grep -q "mode=ingest-harness derived=T-untrust"        && pass "G1 admission⇒derived=T-untrust (proposed T-hostile ignored)" || fail "G1 admission⇒T-untrust"
echo "$OUT" | grep -q "harness_authentic=true"                       && pass "G1 harness_authentic=true"           || fail "G1 harness_authentic"
echo "$OUT" | grep -q "construct-at=T2 effective=T2"                 && pass "G1 construct-at=T2"                  || fail "G1 construct-at=T2"
echo "$OUT" | grep -q "TCC-COMPILE-OK"                               && pass "REAL-COMPILE tcc built an ELF from freshly-written project source" || fail "REAL-COMPILE (tcc did not compile)"
echo "$OUT" | grep -q "BUILD-RAN out=\[REAL-COMPILE-RUN-OK\] exit=42" && pass "G3 REAL-COMPILE-OK: compiled ELF ran from the exec build grant + controlled exit (=42)" || fail "G3 build-exec (compiled ELF did not run from build grant)"
echo "$OUT" | grep -q "PROJECT-NOEXEC-ENFORCED-OK"                    && pass "G3 project noexec ENFORCED: the same ELF cannot execute from the project grant" || fail "G3 project-noexec (execution from project grant was NOT blocked)"
echo "$OUT" | grep -q "PROJECT-EXEC-LEAK-BAD"                         && fail "G3 PROJECT-EXEC-LEAK (ran from noexec project!)" || pass "G3 no project-exec leak"
echo "$OUT" | grep -q "EDIT-WROTE-OK"                                && pass "G2 workload wrote source into project grant" || fail "G2 workload-wrote"
# Host-side write-through across BOTH grants: the edited source on the project inode + the freshly-compiled
# ELF on the build inode (two separate write-through grants).
{ grep -q "edit-9f3a" /srv/project/hello.c 2>/dev/null && head -c4 /srv/build/hello 2>/dev/null | grep -qa ELF; } \
  && pass "G2 write-through: edited hello.c on project + compiled ELF on build (host)" || fail "G2 write-through-to-host"
# Regression: teardown must NOT delete project content through the rw bind — the pre-existing file survives.
grep -q "pre-existing-project-file" /srv/project/README 2>/dev/null \
  && pass "G2 teardown non-destructive: pre-existing README intact" || fail "G2 teardown DESTROYED project content"
echo "$OUT" | grep -q "VAULT-ABSENT-OK"                              && pass "G4 vault-absent(ENOENT)"             || fail "G4 vault-absent"
echo "$OUT" | grep -q "HOST-FS-ISOLATED-OK"                          && pass "G4 host-fs-isolated"                 || fail "G4 host-fs-isolated"
echo "$OUT" | grep -q "NO-EGRESS-OK"                                 && pass "G4 no-egress(--network=none)"        || fail "G4 no-egress"
echo "$OUT" | grep -q "VAULT-VISIBLE-BAD\|HOST-FS-LEAK-BAD\|EGRESS-REACHED-BAD" && fail "G4 LEAK DETECTED" || pass "G4 no-leak-markers"

echo
# Same request as G1 — ONLY the admit-list differs. An unauthenticated harness derives T-hostile, whose
# floor is T2 and whose (T-hostile,C-proj-rw) matrix cell is T3, so the T2 request is refused
# downward-below-floor (rc=10). Either refusal (below-floor rc=10 / no-constructor rc=12) is fail-closed;
# what matters is the SAME cell flips construct→refuse purely on harness integrity, and nothing runs.
echo "=== G5: harness digest ABSENT from admit-list ⇒ T-hostile ⇒ FAIL CLOSED (same request as G1) ==="
OUT5=$(runsandbox "$ADMIT_BAD" p6b --tier T2 --trust T-hostile --caps C-proj-rw --profile C-proj-rw \
        --ingest-harness --id p6b --anchor /srv --rw-grant project -- /bin/sh -c 'echo LEAKED > /srv/project/leak.txt; echo LEAKED'); rc5=$?
echo "$OUT5" | grep -q "mode=ingest-harness derived=T-hostile"       && pass "G5 no-admit⇒derived=T-hostile"       || fail "G5 no-admit⇒T-hostile"
{ { [ $rc5 -eq 10 ] || [ $rc5 -eq 12 ]; } && echo "$OUT5" | grep -q "SANDBOX-DECISION refused"; } \
  && pass "G5 failed-closed rc=$rc5 (refused)" || fail "G5 failed-closed rc=$rc5"
echo "$OUT5" | grep -q "construct-at=" && fail "G5 constructed (should refuse!)" || pass "G5 never-constructed"
echo "$OUT5" | grep -q "LEAKED" && fail "G5 workload-ran (LEAK!)"    || pass "G5 workload-never-ran"
[ -e /srv/project/leak.txt ] && fail "G5 workload wrote to host (LEAK!)" || pass "G5 no-host-write"

# --- teardown ---
umount "$MNT" 2>/dev/null || true; losetup -d "$LOOP" 2>/dev/null || true

echo
if [ $fails -eq 0 ]; then echo "P6-CODER-PROOF: ALL PASS ✅"; exit 0; else echo "P6-CODER-PROOF: $fails FAIL ❌"; exit 1; fi
