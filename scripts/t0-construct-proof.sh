#!/usr/bin/env bash
# Phase-5 slice-4 — GENUINE T0 construction proof through the decision plane, driving the REAL
# release gatekeeperd in a privileged debian:trixie oracle (the fast path before the ~35-min VM
# gate). SPIKE-ONLY (strip before ship). Companion to tier-plane-proof.sh / egress-construct-proof.sh.
#
# What only a real run can show (the unit tests cover the pure policy/ABI logic):
#   G1  a T0 cell (T-first/C-ro-nosec) constructs at GENUINE T0 — decision says construct-at=T0 — and
#       inside the sandbox: the granted path is readable, the ungranted vault is Landlock-DENIED
#       (EACCES, not ENOENT), a deny-list syscall (mount) is seccomp-EPERM, and the fresh netns has
#       no egress. (NO_NEW_PRIVS is implied: seccomp cannot install without it.)
#   G2  a NON-T0 cell (T-pinned/C-proj-rw ⇒ T1) still routes to the T1 nspawn constructor unchanged.
#   G3  a ≥T2 requirement still FAILS CLOSED — the T0 constructor did not open a weaker path.
set -uo pipefail

if [ "${IN_CONTAINER:-0}" != "1" ]; then
  REPO_ROOT="$(git rev-parse --show-toplevel)"
  echo "=== building release gatekeeperd (host) ==="
  ( cd "$REPO_ROOT" && cargo build --release -p gatekeeperd ) || exit 3
  echo "=== launching privileged debian:trixie oracle ==="
  exec docker run --rm --privileged --cgroupns=private -e IN_CONTAINER=1 \
    -v "$REPO_ROOT/scripts/t0-construct-proof.sh:/proof.sh:ro" \
    -v "$REPO_ROOT/target/release/gatekeeperd:/gatekeeperd:ro" \
    debian:trixie bash /proof.sh
fi

# ---------------- inside the container ----------------
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
# systemd-container only for G2 (the T1 nspawn cell); the T0 path itself is dependency-free.
apt-get install -y --no-install-recommends systemd-container >/dev/null 2>&1
mount --make-rshared / 2>/dev/null || true

# cgroup-v2 delegation for the oracle. gatekeeperd moves ITSELF into a `_daemon` leaf and then
# creates the per-sandbox leaf with memory.max/pids.max beside it (fail-closed if the controller
# isn't delegated) — the same dance production does inside a systemd `Delegate=yes` service cgroup.
# That dance needs gatekeeperd's base cgroup to contain ONLY gatekeeperd, so we (a) evacuate the
# container root into an `init` cgroup, (b) delegate memory+pids from the root, and (c) launch
# gatekeeperd for the T0 gate INTO a dedicated empty `shrek-gk` cgroup via a wrapper.
if [ -w /sys/fs/cgroup/cgroup.subtree_control ]; then
  mkdir -p /sys/fs/cgroup/init /sys/fs/cgroup/shrek-gk
  for p in $(cat /sys/fs/cgroup/cgroup.procs 2>/dev/null); do
    echo "$p" > /sys/fs/cgroup/init/cgroup.procs 2>/dev/null || true
  done
  if echo "+memory +pids" > /sys/fs/cgroup/cgroup.subtree_control 2>/dev/null; then
    echo "  cgroup: root delegates [$(cat /sys/fs/cgroup/shrek-gk/cgroup.controllers 2>/dev/null)]"
  else
    echo "  [warn] cgroup root subtree_control not writable — T0 cgroup step will fail closed"
  fi
fi
# Wrapper: place the exec'd gatekeeperd alone in shrek-gk so its _daemon self-move can vacate the base.
cat > /run/gk-t0.sh <<'EOF'
#!/bin/sh
echo $$ > /sys/fs/cgroup/shrek-gk/cgroup.procs 2>/dev/null || true
exec /gatekeeperd "$@"
EOF
chmod +x /run/gk-t0.sh

# Landlock sanity: the oracle kernel must actually offer it, or G1 is meaningless.
if [ ! -d /sys/kernel/security ]; then mount -t securityfs none /sys/kernel/security 2>/dev/null || true; fi
echo "=== oracle kernel: $(uname -r) ==="

mkdir -p /srv/project /srv/vault
echo PROJECT > /srv/project/marker
echo VAULT   > /srv/vault/marker

# Probe executed INSIDE the T0 sandbox. Landlock blocks /proc + /sys reads by design, so every check
# is in-band. Uses absolute /usr paths (no PATH search — Landlock would deny the lookup dirs anyway).
PROBE='
m=$(/usr/bin/cat /srv/project/marker 2>/dev/null)
[ "$m" = PROJECT ] && echo "SHREK_T0: PASS gate=grant-readable m=$m" || echo "SHREK_T0: FAIL gate=grant-readable m=$m"
v=$(/usr/bin/cat /srv/vault/marker 2>&1)
if echo "$v" | grep -qiE "permission denied|not permitted|denied"; then echo "SHREK_T0: PASS gate=vault-landlock-denied"; else echo "SHREK_T0: FAIL gate=vault-landlock-denied out=$v"; fi
e=$(/usr/bin/mount -t tmpfs none /mnt 2>&1); rc=$?
if [ $rc -ne 0 ] && echo "$e" | grep -qiE "not permitted|permission denied"; then echo "SHREK_T0: PASS gate=seccomp-mount-eperm rc=$rc"; else echo "SHREK_T0: FAIL gate=seccomp-mount rc=$rc out=$e"; fi
if [ -x /usr/bin/bash ]; then
  n=$(/usr/bin/bash -c "exec 3<>/dev/tcp/1.1.1.1/53" 2>&1); nrc=$?
  [ $nrc -ne 0 ] && echo "SHREK_T0: PASS gate=netns-no-egress rc=$nrc" || echo "SHREK_T0: FAIL gate=netns-no-egress connected"
else
  echo "SHREK_T0: SKIP gate=netns-no-egress (no bash)"
fi
'

fails=0
pass() { echo "PASS [$1]"; }
fail() { echo "FAIL [$1]"; fails=$((fails+1)); }

echo
echo "=== G1: T-first/C-ro-nosec ⇒ GENUINE T0 (Landlock+seccomp+ns+cgroup) ==="
OUT=$(/run/gk-t0.sh sandbox --tier T0 --trust T-first --caps C-ro-nosec --profile C-ro-nosec \
        --id t0a --anchor /srv --grant project -- /usr/bin/sh -c "$PROBE" 2>&1)
echo "$OUT" | sed 's/^/    /'
echo "$OUT" | grep -q "construct-at=T0 effective=T0" && pass "decision=construct-at-T0" || fail "decision=construct-at-T0"
for g in grant-readable vault-landlock-denied seccomp-mount-eperm; do
  printf '%s\n' "$OUT" | grep -q "^SHREK_T0: PASS gate=$g" && pass "$g" || fail "$g"
done
# netns gate: pass if PASS, tolerate SKIP (no bash), fail only on explicit FAIL.
if printf '%s\n' "$OUT" | grep -q "^SHREK_T0: FAIL gate=netns-no-egress"; then fail "netns-no-egress"; else pass "netns-no-egress(or-skip)"; fi

echo
echo "=== G2: T-pinned/C-proj-rw ⇒ T1 nspawn (non-T0 cell unchanged) ==="
OUT2=$(/gatekeeperd sandbox --tier T1 --trust T-pinned --caps C-proj-rw --profile C-proj-rw \
        --id t1b --anchor /srv --grant project -- /usr/bin/sh -c 'echo "SHREK_T0: PASS gate=t1-ran"' 2>&1)
echo "$OUT2" | sed 's/^/    /'
echo "$OUT2" | grep -q "construct-at=T1 effective=T1" && pass "decision=construct-at-T1" || fail "decision=construct-at-T1"
printf '%s\n' "$OUT2" | grep -q "^SHREK_T0: PASS gate=t1-ran" && pass "t1-constructed" || fail "t1-constructed"

echo
echo "=== G3: ≥T2 requirement still FAILS CLOSED (no weaker path opened) ==="
OUT3=$(/gatekeeperd sandbox --tier T2 --trust T-untrust --caps C-ro-nosec --profile C-ro-nosec \
        --id t2c --anchor /srv --grant project -- /usr/bin/sh -c 'echo LEAKED' 2>&1); rc3=$?
echo "$OUT3" | sed 's/^/    /'
{ [ $rc3 -eq 12 ] && echo "$OUT3" | grep -q "no-constructor"; } && pass "t2-failed-closed rc=$rc3" || fail "t2-failed-closed rc=$rc3"
printf '%s\n' "$OUT3" | grep -q "LEAKED" && fail "t2-workload-ran (LEAK!)" || pass "t2-workload-never-ran"

echo
if [ $fails -eq 0 ]; then echo "T0-CONSTRUCT-PROOF: ALL PASS ✅"; exit 0; else echo "T0-CONSTRUCT-PROOF: $fails FAIL ❌"; exit 1; fi
