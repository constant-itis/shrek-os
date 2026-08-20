#!/usr/bin/env bash
# Phase-6 slice-1b — T2 NAMED-EGRESS proof. Drives the REAL release gatekeeperd (t2_plane egress
# wiring: a PRE-created netns + veth + the sealed nft allow-list, runsc `--network=sandbox` so gVisor's
# netstack egresses over our veth) inside a privileged debian:trixie oracle, over a GENUINE fs-verity
# runsc harness. The fast gate before the ~35-min sealed-VM cycle. SPIKE-ONLY (strip before ship).
# Sibling of p6-coder-proof.sh (integrity-bound T2 ingest session) + egress-construct-proof.sh (T1 plane).
#
# What only a real run can show (unit tests cover the pure parser/lattice + config shape):
#   E1  An INTEGRITY-BOUND untrusted-ingest session (--ingest-harness ⇒ derived=T-untrust) with
#       --caps C-net + a sealed egress profile constructs at GENUINE T2 and its workload REACHES the
#       pinned destination FROM INSIDE gVisor — proving netstack programs itself from the pre-created
#       veth and egresses through the host-side nft allow-list. Audited: construct-at=T2,
#       egress=github-https, `netstack --network=sandbox`.
#   E2  the SAME session against a NON-allowed dst → DROPPED (default-deny at the veth/nft boundary).
#   E3  a no-egress T2 ingest session (C-proj-rw) → `--network=none`, loopback-only (regression: the
#       slice-1a posture is unchanged for cells that name no profile).
#   E4  unknown egress profile → REFUSED (rc 13), nothing constructed.
#   E5  C-broad ⇒ (T-untrust,C-broad)=T3 ⇒ REFUSED (no constructor) — opening C-net at T2 did NOT open
#       unrestricted egress.
#   E6  teardown → no residual netns / veth / nft table after the runs (fail-closed default = no network).
set -uo pipefail

PIN_SHA256="670bcd3cbc103f00d8bb5098edc370f32397ee4c134231436bafa659bb3c068e"
PIN_URL="https://storage.googleapis.com/gvisor/releases/release/20260810.0/x86_64/runsc"

if [ "${IN_CONTAINER:-0}" != "1" ]; then
  REPO_ROOT="$(git rev-parse --show-toplevel)"
  echo "=== building gatekeeperd --features spike (host) [pin-verity provisions the harness verity] ==="
  ( cd "$REPO_ROOT" && cargo build --release -p gatekeeperd --features spike ) || exit 3

  CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/shrek"; mkdir -p "$CACHE"
  RUNSC="$CACHE/runsc-20260810.0"
  if [ ! -f "$RUNSC" ] || [ "$(sha256sum "$RUNSC" | awk '{print $1}')" != "$PIN_SHA256" ]; then
    echo "=== fetching pinned runsc (release-20260810.0) ==="
    curl -fsSL -m 300 -o "$RUNSC" "$PIN_URL" || { echo "runsc fetch failed"; exit 3; }
  fi
  [ "$(sha256sum "$RUNSC" | awk '{print $1}')" = "$PIN_SHA256" ] || { echo "PIN MISMATCH"; exit 3; }
  chmod +x "$RUNSC"

  echo "=== launching privileged debian:trixie oracle ==="
  exec docker run --rm --privileged --cgroupns=private -e IN_CONTAINER=1 \
    -v "$REPO_ROOT/scripts/p6-egress-proof.sh:/proof.sh:ro" \
    -v "$REPO_ROOT/target/release/gatekeeperd:/gatekeeperd:ro" \
    -v "$RUNSC:/runsc-src:ro" \
    debian:trixie bash /proof.sh
fi

# ---------------- inside the container ----------------
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
# busybox-static = sandbox userland (wget applet); systemd = systemd-detect-virt (virt gate → systrap in
# docker); e2fsprogs = mkfs.ext4 -O verity; iproute2/nftables = the egress plane; ncat = hermetic server.
apt-get install -y --no-install-recommends busybox-static systemd ca-certificates e2fsprogs iproute2 nftables ncat >/dev/null 2>&1
mount --make-rshared / 2>/dev/null || true
mount -t tmpfs tmpfs /run 2>/dev/null || true
echo 1 > /proc/sys/net/ipv4/ip_forward 2>/dev/null || true
GK=/gatekeeperd

pass(){ echo "  PASS $*"; }
fail(){ echo "  FAIL $*"; fails=$((fails+1)); }
fails=0

# --- Genuine fs-verity harness: provision a verity-capable ext4 on a loopback and enable verity on the
#     runsc harness inode, then measure it into the sealed admit-list (same anchor as p6-coder-proof). ---
IMG=/tmp/harness-verity.img; MNT=/mnt/harness; mkdir -p "$MNT"
dd if=/dev/zero of="$IMG" bs=1M count=256 status=none
mkfs.ext4 -q -b 4096 -O verity "$IMG" || { echo "FAIL provision: mkfs.ext4 -O verity unavailable"; exit 2; }
for i in $(seq 0 63); do [ -e /dev/loop$i ] || mknod -m660 /dev/loop$i b 7 "$i"; done
LOOP=$(losetup -f --show "$IMG") || { echo "FAIL provision: losetup"; exit 2; }
mount -o exec "$LOOP" "$MNT" || { echo "FAIL provision: mount verity fs"; exit 2; }
cp /runsc-src "$MNT/runsc"; chmod +x "$MNT/runsc"
"$GK" pin-verity enable "$MNT/runsc" || { echo "FAIL provision: enable-verity on runsc"; exit 2; }
DL=$("$GK" pin-verity measure "$MNT/runsc") || { echo "FAIL provision: measure runsc"; exit 2; }
ALGO=${DL%% *}; HEX=${DL##* }
RUNSC="$MNT/runsc"
ADMIT_OK=/tmp/ingest-admit.ok
printf 'shrek-t2-ingest-admit v1\n# authorised T2 untrusted-ingest harness\n%s %s\n' "$ALGO" "$HEX" > "$ADMIT_OK"
echo "provisioned genuine fs-verity harness: runsc = $ALGO $HEX"

# --- Minimal pinned sandbox rootfs: busybox applets (wget = the egress client). Throwaway. ---
ROOTFS=/rootfs; rm -rf "$ROOTFS"; mkdir -p "$ROOTFS/bin"
cp /bin/busybox "$ROOTFS/bin/busybox" 2>/dev/null || cp /usr/bin/busybox "$ROOTFS/bin/busybox"
for a in sh cat ls wget nc timeout echo test; do ln -sf busybox "$ROOTFS/bin/$a"; done
# Reproduce the sealed condition (rootfs on read-only dm-verity /usr): construct copies it to /run and
# writes /etc/hosts into that writable copy — so a RO source is exactly the gofer-EROFS path we ship.
mount --bind "$ROOTFS" "$ROOTFS" && mount -o remount,ro,bind "$ROOTFS" || echo "WARN: rootfs not remounted ro (oracle less faithful)"

# --- The local "internet": a server netns holding the pinned dst on :443, reached via a root uplink.
#     The container's egress veth masquerades toward it exactly as in the T1 egress oracle. ---
ip netns add srv
ip link add up-egr type veth peer name srv0
ip link set srv0 netns srv
ip addr add 10.20.0.1/24 dev up-egr; ip link set up-egr up
ip -n srv addr add 10.20.0.2/24 dev srv0; ip -n srv link set srv0 up; ip -n srv link set lo up
ip -n srv route add default via 10.20.0.1
ip netns exec srv ncat -lk 10.20.0.2 443 --sh-exec \
  'printf "HTTP/1.0 200 OK\r\nContent-Length: 15\r\n\r\nSHREK_EGRESS_OK"' >/dev/null 2>&1 &
SRV_PID=$!
# Map the sealed github-https profile's hosts to the local server so gatekeeperd's resolver pins them.
printf '10.20.0.2 github.com codeload.github.com objects.githubusercontent.com\n' >> /etc/hosts
sleep 0.4

# --- Anchor + writable project/build grants (the ingest coding session). ---
rm -rf /srv; mkdir -p /srv/project /srv/build
echo "pre-existing-project-file" > /srv/project/README

# --- cgroup-v2 delegation for the oracle. As in p6-coder-proof: vacate root's procs into an `init` leaf
#     so root can DELEGATE controllers to its children, then enable +memory +pids. UNLIKE the single-
#     construct oracles this proof runs SEVERAL successful T2 builds back-to-back, and the gatekeeperd
#     `_daemon`-vacate is not reentrant on a shared base — so each run also gets its OWN parent
#     (shrek-run-<tag>, a child of root that inherits the delegated controllers), leaving each build as
#     clean as a per-boot VM. ---
if [ -w /sys/fs/cgroup/cgroup.subtree_control ]; then
  mkdir -p /sys/fs/cgroup/init
  for p in $(cat /sys/fs/cgroup/cgroup.procs 2>/dev/null); do echo "$p" > /sys/fs/cgroup/init/cgroup.procs 2>/dev/null || true; done
  echo "+memory +pids" > /sys/fs/cgroup/cgroup.subtree_control 2>/dev/null || true
fi
runsandbox(){ # the gatekeeperd `sandbox` args; the --id value doubles as this run's cgroup-parent tag
  local tag="run" prev=""
  for a in "$@"; do [ "$prev" = "--id" ] && { tag="$a"; break; }; prev="$a"; done
  local cgp="/sys/fs/cgroup/shrek-run-$tag"
  mkdir -p "$cgp" 2>/dev/null || true
  cat > /run/gk-run.sh <<WRAP
#!/usr/bin/env bash
echo \$\$ > $cgp/cgroup.procs 2>/dev/null || true
exec env SHREK_T2_RUNSC=$RUNSC SHREK_T2_ROOTFS=/rootfs SHREK_INGEST_ADMIT=$ADMIT_OK /gatekeeperd sandbox "\$@"
WRAP
  chmod +x /run/gk-run.sh
  /run/gk-run.sh "$@" 2>&1
}

# A full ingest coding session shape: --caps C-net (⊇ C-proj-rw, so rw+build grants are authorized too),
# authentic harness ⇒ T-untrust, requested T2 = floor. Only the egress-profile / dst / id vary per case.
INGEST=(--ingest-harness --anchor /srv --rw-grant project --build-grant build)

echo
echo "=== E1: authentic harness + C-net + sealed github-https ⇒ T2 ingest session REACHES pinned dst (netstack) ==="
OUT=$(runsandbox --tier T2 --trust T-hostile --caps C-net --profile C-net "${INGEST[@]}" \
        --egress-profile github-https --id e1 -- /bin/sh -c 'PATH=/bin; export PATH; wget -T6 -qO- http://github.com:443/')
echo "$OUT" | sed 's/^/    /'
echo "$OUT" | grep -q "mode=ingest-harness derived=T-untrust" && pass "E1 admission ⇒ derived=T-untrust" || fail "E1 admission ⇒ T-untrust"
echo "$OUT" | grep -q "construct-at=T2 effective=T2"          && pass "E1 construct-at=T2"                || fail "E1 construct-at=T2"
echo "$OUT" | grep -q "egress=github-https"                   && pass "E1 decision egress=github-https"  || fail "E1 egress name in decision"
echo "$OUT" | grep -q "netstack --network=sandbox"            && pass "E1 runsc joined the netns (netstack --network=sandbox)" || fail "E1 netstack path not taken"
echo "$OUT" | grep -q "SHREK_EGRESS_OK"                       && pass "E1 workload REACHED the pinned dst from inside gVisor" || fail "E1 no reach marker (netstack egress broken?)"

echo
echo "=== E2: same session, NON-allowed dst ⇒ DROPPED (default-deny) ==="
OUT=$(runsandbox --tier T2 --trust T-hostile --caps C-net --profile C-net "${INGEST[@]}" \
        --egress-profile github-https --id e2 -- /bin/sh -c 'PATH=/bin; export PATH; wget -T3 -qO- http://10.20.0.3:443/ 2>&1; echo RC=$?')
echo "$OUT" | grep -oE 'RC=[0-9]+|SHREK_EGRESS_OK|construct-at=[^ ]+' | sed 's/^/    /'
if echo "$OUT" | grep -q SHREK_EGRESS_OK; then fail "E2 reached a non-allowed dst (LEAK!)"
elif echo "$OUT" | grep -qE 'RC=[^0 ]'; then pass "E2 non-allowed dst dropped (wget rc!=0, no RST)"
else fail "E2 unexpected: $(echo "$OUT" | tr '\n' ' ')"; fi

echo
echo "=== E3: no-egress T2 ingest session (C-proj-rw) ⇒ --network=none, loopback-only (regression) ==="
OUT=$(runsandbox --tier T2 --trust T-hostile --caps C-proj-rw --profile C-proj-rw "${INGEST[@]}" \
        --id e3 -- /bin/sh -c 'PATH=/bin; export PATH; echo IFACES=$(ls /sys/class/net | tr "\n" ,); wget -T3 -qO- http://github.com:443/ 2>&1; echo RC=$?')
echo "$OUT" | grep -oE 'IFACES=[^ ]*|SHREK_EGRESS_OK|construct-at=[^ ]+|egress=[^ ]+' | sed 's/^/    /'
echo "$OUT" | grep -q "construct-at=T2"    && pass "E3 constructed at T2"                 || fail "E3 did not construct"
if echo "$OUT" | grep -q SHREK_EGRESS_OK; then fail "E3 no-egress cell REACHED dst (LEAK!)"
elif echo "$OUT" | grep -qE 'IFACES=(lo,?)?$'; then pass "E3 loopback-only, dst unreachable (--network=none)"
else fail "E3 unexpected ifaces: $(echo "$OUT" | grep -o 'IFACES=[^ ]*')"; fi
echo "$OUT" | grep -q "netstack --network=sandbox" && fail "E3 took the netstack path (should be --network=none)" || pass "E3 no netstack (loopback-only)"

echo
echo "=== E4: unknown egress profile ⇒ REFUSED (rc 13), nothing constructed ==="
OUT=$(runsandbox --tier T2 --trust T-hostile --caps C-net --profile C-net "${INGEST[@]}" \
        --egress-profile bogus-exfil --id e4 -- /bin/sh -c 'echo SHOULD-NOT-RUN'); rc=$?
echo "$OUT" | grep -oE 'reason=[^ ]+|construct-at=[^ ]+' | sed 's/^/    /'
{ [ "$rc" = 13 ] && echo "$OUT" | grep -q "unknown-egress-profile=bogus-exfil"; } && pass "E4 rc=$rc unknown-egress-profile" || fail "E4 rc=$rc (expected 13 + unknown-egress-profile)"
echo "$OUT" | grep -q "construct-at=" && fail "E4 constructed (should refuse!)" || pass "E4 never-constructed"
echo "$OUT" | grep -q "SHOULD-NOT-RUN" && fail "E4 workload ran" || pass "E4 workload-never-ran"

echo
echo "=== E5: C-broad ⇒ (T-untrust,C-broad)=T3 ⇒ REFUSED (no constructor); C-net at T2 did NOT open C-broad ==="
OUT=$(runsandbox --tier T3 --trust T-hostile --caps C-broad --profile C-broad "${INGEST[@]}" \
        --egress-profile github-https --id e5 -- /bin/sh -c 'echo SHOULD-NOT-RUN'); rc=$?
echo "$OUT" | grep -oE 'reason=[^ ]+|construct-at=[^ ]+' | sed 's/^/    /'
{ [ "$rc" = 12 ] && echo "$OUT" | grep -q "no-constructor-T3"; } && pass "E5 rc=$rc no-constructor-T3 (C-broad refused)" || fail "E5 rc=$rc (expected 12 + no-constructor-T3)"
echo "$OUT" | grep -q "construct-at=" && fail "E5 constructed (should refuse!)" || pass "E5 never-constructed"
echo "$OUT" | grep -q "SHOULD-NOT-RUN" && fail "E5 workload ran" || pass "E5 workload-never-ran"

echo
echo "=== E6: teardown ⇒ no residual netns / veth / nft table ==="
resid_ns=$(ip netns list 2>/dev/null | grep -oE 'shrek-e[0-9]+' | tr '\n' ',')
resid_if=$(ip -o link show 2>/dev/null | grep -oE 'skh[0-9a-f]{4}' | tr '\n' ',')
resid_tb=$(nft list tables 2>/dev/null | grep -o 'shrek_egress_[a-z0-9_]*' | tr '\n' ',')
if [ -z "$resid_ns" ] && [ -z "$resid_if" ] && [ -z "$resid_tb" ]; then pass "E6 no residual egress plumbing"
else fail "E6 residual ns=[$resid_ns] if=[$resid_if] tables=[$resid_tb]"; fi

# --- teardown ---
kill "$SRV_PID" 2>/dev/null
umount "$MNT" 2>/dev/null || true; losetup -d "$LOOP" 2>/dev/null || true

echo
if [ $fails -eq 0 ]; then echo "P6-EGRESS-PROOF: ALL PASS ✅"; exit 0; else echo "P6-EGRESS-PROOF: $fails FAIL ❌"; exit 1; fi
