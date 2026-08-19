#!/usr/bin/env bash
# Phase-5 slice-6 — GENUINE T2 construction proof through the decision plane, driving the REAL release
# gatekeeperd + the PINNED runsc (image/supply/gvisor.pin) in a privileged debian:trixie oracle (the
# fast path before the ~35-min VM gate). SPIKE-ONLY (strip before ship). Sibling of t0-construct-proof.sh.
#
# What only a real run can show (unit tests cover the pure policy/probe logic):
#   G1  a T2 cell (T-untrust/C-ro-nosec) constructs at GENUINE T2 (gVisor/runsc) — decision says
#       construct-at=T2 — and INSIDE the sandbox: the granted path is readable, the ungranted vault is
#       ABSENT (ENOENT, the T2 absence model), a host sentinel file is NOT visible (host FS isolated),
#       and --network=none yields NO egress.
#   G2  a T0 cell (T-first/C-ro-nosec) still routes to the T0 constructor (construct-at=T0), NOT T2.
#   G3  a T3 cell (T-hostile/C-proj-rw) FAILS CLOSED (no constructor), and a T2 C-net cell FAILS
#       CLOSED (no gVisor egress plane) — never a weaker path.
#   G4  platform selection lands on systrap in this (containerized ⇒ virtualized) host, proving the
#       systemd-detect-virt gate ran and did not pick nested KVM.
set -uo pipefail

PIN_SHA256="670bcd3cbc103f00d8bb5098edc370f32397ee4c134231436bafa659bb3c068e"
PIN_URL="https://storage.googleapis.com/gvisor/releases/release/20260810.0/x86_64/runsc"

if [ "${IN_CONTAINER:-0}" != "1" ]; then
  REPO_ROOT="$(git rev-parse --show-toplevel)"
  echo "=== building release gatekeeperd (host) ==="
  ( cd "$REPO_ROOT" && cargo build --release -p gatekeeperd ) || exit 3

  # Cache the PINNED runsc on the host (verify sha256 against gvisor.pin; never 'latest').
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
    -v "$REPO_ROOT/scripts/t2-construct-proof.sh:/proof.sh:ro" \
    -v "$REPO_ROOT/target/release/gatekeeperd:/gatekeeperd:ro" \
    -v "$RUNSC:/runsc:ro" \
    debian:trixie bash /proof.sh
fi

# ---------------- inside the container ----------------
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
# busybox-static = the sandbox rootfs userland; systemd = systemd-detect-virt (the virt gate);
# ca-certificates only so any TLS in the image is sane. No containerd/shim — runsc drives directly.
apt-get install -y --no-install-recommends busybox-static systemd ca-certificates >/dev/null 2>&1
mount --make-rshared / 2>/dev/null || true

pass(){ echo "  PASS $*"; }
fail(){ echo "  FAIL $*"; fails=$((fails+1)); }
fails=0

# --- Build a minimal, pinned sandbox rootfs (busybox applets). Production seals this; here it is a
#     throwaway (SHREK_T2_ROOTFS override, spike-only). ---
ROOTFS=/rootfs; rm -rf "$ROOTFS"; mkdir -p "$ROOTFS/bin"
cp /usr/bin/busybox "$ROOTFS/bin/busybox"
# RELATIVE symlinks (target "busybox") so /bin/sh → /bin/busybox INSIDE the sandbox. `busybox
# --install -s` would write ABSOLUTE links to /rootfs/bin/busybox, which does not exist inside.
for a in sh cat ls nc timeout echo test; do ln -sf busybox "$ROOTFS/bin/$a"; done

# --- Anchor + grant + ungranted vault; a host sentinel to prove the host FS does not leak in. ---
mkdir -p /srv/project /srv/vault
echo "granted-secret-xyz" > /srv/project/marker
echo "TOP-SECRET-DO-NOT-LEAK" > /srv/vault/secret
echo "HOST-ONLY" > /etc/shrek-host-sentinel

# --- cgroup-v2 delegation for the oracle (same dance as t0): evacuate the container root into an
#     `init` leaf, delegate memory+pids from the root, and launch gatekeeperd alone into an empty
#     `shrek-gk` cgroup so its own `_daemon` self-move can vacate the base. ---
if [ -w /sys/fs/cgroup/cgroup.subtree_control ]; then
  mkdir -p /sys/fs/cgroup/init /sys/fs/cgroup/shrek-gk
  for p in $(cat /sys/fs/cgroup/cgroup.procs 2>/dev/null); do echo "$p" > /sys/fs/cgroup/init/cgroup.procs 2>/dev/null || true; done
  echo "+memory +pids" > /sys/fs/cgroup/cgroup.subtree_control 2>/dev/null || true
fi
cat > /run/gk-t2.sh <<'WRAP'
#!/usr/bin/env bash
# Place the exec'd gatekeeperd alone in shrek-gk so its _daemon self-move can vacate the base.
echo $$ > /sys/fs/cgroup/shrek-gk/cgroup.procs 2>/dev/null || true
exec env SHREK_T2_RUNSC=/runsc SHREK_T2_ROOTFS=/rootfs /gatekeeperd "$@"
WRAP
chmod +x /run/gk-t2.sh

# The in-sandbox probe (single line ⇒ no newline-in-JSON surprises; json_escape handles it anyway).
PROBE='echo PROBE-START; if [ -r /srv/project/marker ]; then echo "GRANT-READ-OK=$(cat /srv/project/marker)"; else echo GRANT-READ-FAIL; fi; if [ -e /srv/vault ]; then echo VAULT-VISIBLE-BAD; else echo VAULT-ABSENT-OK; fi; if [ -e /etc/shrek-host-sentinel ]; then echo HOST-FS-LEAK-BAD; else echo HOST-FS-ISOLATED-OK; fi; if timeout 5 nc -w 2 1.1.1.1 53 </dev/null >/dev/null 2>&1; then echo EGRESS-REACHED-BAD; else echo NO-EGRESS-OK; fi; echo PROBE-END'

echo
echo "=== G1: T-untrust/C-ro-nosec ⇒ GENUINE T2 (gVisor/runsc), caps + isolation enforced ==="
OUT=$(/run/gk-t2.sh sandbox --tier T2 --trust T-untrust --caps C-ro-nosec --profile C-ro-nosec \
        --id t2a --anchor /srv --grant project -- /bin/sh -c "$PROBE" 2>&1)
echo "$OUT" | sed 's/^/    /'
echo "$OUT" | grep -q "construct-at=T2 effective=T2"      && pass "decision=construct-at-T2"     || fail "decision=construct-at-T2"
echo "$OUT" | grep -q "GRANT-READ-OK=granted-secret-xyz"  && pass "grant-readable"               || fail "grant-readable"
echo "$OUT" | grep -q "VAULT-ABSENT-OK"                    && pass "vault-absent(ENOENT)"         || fail "vault-absent"
echo "$OUT" | grep -q "HOST-FS-ISOLATED-OK"               && pass "host-fs-isolated"             || fail "host-fs-isolated"
echo "$OUT" | grep -q "NO-EGRESS-OK"                       && pass "no-egress(--network=none)"    || fail "no-egress"
echo "$OUT" | grep -q "VAULT-VISIBLE-BAD\|HOST-FS-LEAK-BAD\|EGRESS-REACHED-BAD" && fail "LEAK DETECTED" || pass "no-leak-markers"

echo
echo "=== G2: T-first/C-ro-nosec still routes to the T0 constructor (NOT T2) ==="
OUT2=$(/run/gk-t2.sh sandbox --tier T0 --trust T-first --caps C-ro-nosec --profile C-ro-nosec \
        --id g2 --anchor /srv --grant project -- /bin/sh -c 'echo T0-RAN' 2>&1)
echo "$OUT2" | grep -q "construct-at=T0 effective=T0" && pass "decision=construct-at-T0" || fail "decision=construct-at-T0"

echo
echo "=== G3: T3 cell + T2/C-net cell both FAIL CLOSED (no weaker path) ==="
OUT3=$(/run/gk-t2.sh sandbox --tier T3 --trust T-hostile --caps C-proj-rw --profile C-proj-rw \
        --id g3a --anchor /srv --grant project -- /bin/sh -c 'echo LEAKED' 2>&1); rc3=$?
{ [ $rc3 -eq 12 ] && echo "$OUT3" | grep -q "no-constructor"; } && pass "t3-failed-closed rc=$rc3" || fail "t3-failed-closed rc=$rc3"
echo "$OUT3" | grep -q "LEAKED" && fail "t3-workload-ran (LEAK!)" || pass "t3-never-ran"
OUT3b=$(/run/gk-t2.sh sandbox --tier T2 --trust T-untrust --caps C-net --profile C-net --egress-profile github \
        --id g3b --anchor /srv --grant project -- /bin/sh -c 'echo LEAKED' 2>&1); rc3b=$?
{ [ $rc3b -eq 12 ] && echo "$OUT3b" | grep -q "no-gvisor-egress-plane"; } && pass "t2-cnet-failed-closed rc=$rc3b" || fail "t2-cnet-failed-closed rc=$rc3b"
echo "$OUT3b" | grep -q "LEAKED" && fail "t2-cnet-workload-ran (LEAK!)" || pass "t2-cnet-never-ran"

echo
echo "=== G4: platform selection = systrap on this containerized (⇒ virtualized) host ==="
echo "$OUT" | grep -q "platform=systrap" && pass "platform=systrap (systemd-detect-virt gate)" || fail "platform=systrap"
echo "$OUT" | grep -q "platform=kvm" && fail "picked-nested-KVM (policy violation)" || pass "did-not-pick-nested-kvm"

echo
if [ $fails -eq 0 ]; then echo "T2-CONSTRUCT-PROOF: ALL PASS ✅"; exit 0; else echo "T2-CONSTRUCT-PROOF: $fails FAIL ❌"; exit 1; fi
