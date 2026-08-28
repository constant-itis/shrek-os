#!/usr/bin/env bash
# Phase-6 slice-1b — BARE-METAL KVM smoke. Runs the T2 named-egress ingest session DIRECTLY on the host
# (NOT docker, NOT a VM) so `systemd-detect-virt` returns `none` and gatekeeperd's select_platform picks
# the REAL `--platform=kvm` — the one axis the docker oracle (→systrap) and the nested sealed VM
# (→systrap) cannot exercise. Proves gVisor's kvm platform + the egress plane compose on real hardware.
#
# Needs root (netns/nft/mount/loopback + fs-verity ioctl). Self-cleaning: restores /etc/hosts, tears down
# the netns/veth/nft/loopback, removes its work dir — on every exit. No apt (uses busybox nc +
# gatekeeperd's built-in `pin-verity`; a python3 listener for the hermetic server). SPIKE-ONLY.
#
# HOST FORWARD POLICY: the default run NEVER mutates the host firewall. On a host whose kernel FORWARD
# policy is `drop` (Docker/libvirt/ufw present), the guest→dst forward is blocked by the HOST — an
# external condition, not an egress-plane defect — so BM1 *reach* is classified SKIP (reach is proven by
# the p6-egress-proof.sh oracle E1 + the sealed VM); BM0/BM1-exec/BM2/BM3 still run and must pass. For a
# full bare-metal reach proof on such hosts, opt in: SHREK_BM_FORWARD_ALLOW=1 installs a narrow,
# self-cleaning FORWARD ACCEPT for the Shrek supernet. Firewall mutation is NEVER part of construction.
#
# EGRESS CLIENT: the guest probe is busybox `nc` (by pinned IP), NOT `wget` — the host busybox `wget`
# applet is a broken glibc-static binary that SIGSEGVs even outside gVisor; see the rootfs note below.
#
# Usage (from the repo root, after `cargo build --release -p gatekeeperd --features spike`):
#   sudo bash scripts/p6-egress-baremetal.sh
#
# HOST-HARDENING ACCEPTANCE (slice-6 / P6-1c): a hardened host mounts /run `noexec` (Pop!_OS/CIS
# default). gatekeeperd now stages the per-sandbox T2 rootfs on its OWN fresh exec tmpfs
# (t2_plane::construct → mount_plane::stage_tmpfs) instead of inheriting /run's flags, so the guest's
# PROT_EXEC map + gVisor's setup-root no longer depend on /run being exec. This smoke therefore does
# NOT shadow /run/shrek-t2 anymore — running it on a noexec /run is the whole point: a green BM1 here
# PROVES the product fix (previously the shadow masked the bug). BM0 still isolates the kvm-engine
# question on an exec /tmp bundle.
set -uo pipefail

[ "$(id -u)" = 0 ] || { echo "must run as root:  sudo bash scripts/p6-egress-baremetal.sh"; exit 2; }
REPO="${REPO:-$PWD}"
GK="$REPO/target/release/gatekeeperd"
INVOKER="/home/${SUDO_USER:-$USER}"
RUNSC_SRC="${RUNSC_SRC:-$INVOKER/.cache/shrek/runsc-20260810.0}"
[ -x "$GK" ] || { echo "build first (as your user):  cargo build --release -p gatekeeperd --features spike"; exit 2; }
[ -f "$RUNSC_SRC" ] || { echo "no cached runsc at $RUNSC_SRC (run scripts/p6-egress-proof.sh once to fetch it)"; exit 2; }
command -v busybox >/dev/null || { echo "need busybox"; exit 2; }
command -v python3 >/dev/null || { echo "need python3 (hermetic egress server)"; exit 2; }
[ -e /dev/kvm ] || { echo "no /dev/kvm — cannot exercise the kvm platform"; exit 2; }
V=$(systemd-detect-virt 2>/dev/null || echo unknown)
[ "$V" = none ] || echo "WARN: systemd-detect-virt=$V (want 'none'); select_platform may pick systrap → this smoke would not test kvm"

fails=0; skips=0
pass(){ echo "  PASS $*"; }; fail(){ echo "  FAIL $*"; fails=$((fails+1)); }; skip(){ echo "  SKIP $*"; skips=$((skips+1)); }

WORK=$(mktemp -d /tmp/p6-bm.XXXXXX)
MNT="$WORK/harness"; IMG="$WORK/harness.img"; ROOTFS="$WORK/rootfs"; ANCHOR="$WORK/anchor"
LOOP=""; SRV_PID=""; HOSTS_TOUCHED=0; FW_ALLOWED=0; SHREK_SUPERNET=10.66.0.0/16
# NOTE: intentionally NO /run/shrek-t2 exec-tmpfs shadow here — gatekeeperd stages its own exec tmpfs
# for the rootfs area now, so a noexec /run (this host) must be handled by the product, not the smoke.
if findmnt -no OPTIONS /run 2>/dev/null | grep -q noexec; then
  echo "note: /run is noexec on this host — exercising gatekeeperd's own exec-tmpfs rootfs staging (no shadow)"
fi
cleanup(){
  [ -n "$SRV_PID" ] && kill "$SRV_PID" 2>/dev/null
  # Belt-and-suspenders: gatekeeperd tears its own egress plumbing down, but sweep any residue by id/name.
  for tb in $(nft list tables 2>/dev/null | grep -o 'shrek_egress_[a-z0-9_]*'); do nft delete table ip "$tb" 2>/dev/null; done
  for ns in $(ip netns list 2>/dev/null | grep -oE 'shrek-bm[0-9]+'); do ip netns del "$ns" 2>/dev/null; done
  for lk in $(ip -o link show 2>/dev/null | grep -oE 'skh[0-9a-f]{4}'); do ip link del "$lk" 2>/dev/null; done
  ip netns del srv 2>/dev/null; ip link del up-egr 2>/dev/null
  umount "$MNT" 2>/dev/null; [ -n "$LOOP" ] && losetup -d "$LOOP" 2>/dev/null
  [ "$HOSTS_TOUCHED" = 1 ] && cp "$WORK/hosts.bak" /etc/hosts   # restore the real /etc/hosts exactly
  # Opt-in only: remove the narrow FORWARD allowance we installed (exact-match delete, leaves host policy intact).
  if [ "$FW_ALLOWED" = 1 ]; then
    iptables -D FORWARD -s "$SHREK_SUPERNET" -j ACCEPT 2>/dev/null
    iptables -D FORWARD -d "$SHREK_SUPERNET" -j ACCEPT 2>/dev/null
  fi
  rm -rf "$WORK" /run/shrek-t2-debug/bm1 /run/shrek-t2-debug/bm2 2>/dev/null
}
trap cleanup EXIT

# --- fs-verity harness: verity-capable ext4 on a loopback; enable+measure via gatekeeperd (no fsverity CLI) ---
dd if=/dev/zero of="$IMG" bs=1M count=256 status=none
mkfs.ext4 -q -b 4096 -O verity "$IMG" || { echo "FAIL: mkfs.ext4 -O verity unavailable"; exit 1; }
LOOP=$(losetup -f --show "$IMG") || { echo "FAIL losetup"; exit 1; }
mkdir -p "$MNT"; mount -o exec "$LOOP" "$MNT" || { echo "FAIL mount verity fs"; exit 1; }
cp "$RUNSC_SRC" "$MNT/runsc"; chmod +x "$MNT/runsc"
"$GK" pin-verity enable "$MNT/runsc" || { echo "FAIL enable-verity (kernel/fs lacks fs-verity)"; exit 1; }
DL=$("$GK" pin-verity measure "$MNT/runsc") || { echo "FAIL measure"; exit 1; }
ALGO=${DL%% *}; HEX=${DL##* }
ADMIT="$WORK/admit.ok"; printf 'shrek-t2-ingest-admit v1\n%s %s\n' "$ALGO" "$HEX" > "$ADMIT"
RUNSC="$MNT/runsc"
echo "harness fs-verity: $ALGO $HEX"

# --- minimal busybox rootfs (nc = egress client) ---
# NB: the guest egress probe is busybox `nc`, NOT `wget`: the HOST's busybox `wget` applet is a broken
# glibc-static binary on this class of host — its getaddrinfo dlopens an ABI-mismatched libnss and
# SIGSEGVs even on the bare host, outside gVisor (rc139). `nc`/`sh`/`echo`/`cat` (same binary) are fine.
# `nc` connects to the profile's PINNED IPv4 endpoint directly, exercising the netstack + per-sandbox nft
# boundary under kvm; the NAME-resolution path is covered by the oracle (p6-egress-proof.sh E1, which
# installs a working busybox). The oracle wouldn't ship a broken wget; the smoke reuses the host's.
mkdir -p "$ROOTFS/bin"; cp /usr/bin/busybox "$ROOTFS/bin/busybox"
for a in sh nc ls cat echo timeout; do ln -sf busybox "$ROOTFS/bin/$a"; done

# --- anchor grant ---
mkdir -p "$ANCHOR/project"; echo pre-existing > "$ANCHOR/project/README"

# --- hermetic "internet": a server netns holding the pinned dst on :443 ---
# Sends the marker IMMEDIATELY on accept (no recv first) so busybox `nc <ip> 443 </dev/null` reads it
# reliably over the netstack path without depending on stdin/half-close timing.
ip netns add srv
ip link add up-egr type veth peer name srv0
ip link set srv0 netns srv
ip addr add 10.20.0.1/24 dev up-egr; ip link set up-egr up
ip -n srv addr add 10.20.0.2/24 dev srv0; ip -n srv link set srv0 up; ip -n srv link set lo up
ip -n srv route add default via 10.20.0.1
ip netns exec srv python3 -c '
import socket
s=socket.socket(); s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1)
s.bind(("10.20.0.2",443)); s.listen(8)
while True:
    c,_=s.accept()
    try: c.sendall(b"HTTP/1.0 200 OK\r\nContent-Length: 15\r\n\r\nSHREK_EGRESS_OK")
    except OSError: pass
    c.close()' &
SRV_PID=$!
echo 1 > /proc/sys/net/ipv4/ip_forward
# Map the sealed github-https profile's hosts to the local server so gatekeeperd's resolver pins them.
# Back up + restore the REAL /etc/hosts so the laptop is left byte-identical.
cp /etc/hosts "$WORK/hosts.bak"; HOSTS_TOUCHED=1
printf '10.20.0.2 github.com codeload.github.com objects.githubusercontent.com  # shrek-p6-bm (temp)\n' >> /etc/hosts
sleep 0.4

# Host FORWARD policy gate. The egress plane masquerades the sandbox /30 and scopes its per-sandbox nft
# chain to that container IP — DELIBERATELY without touching the host's global forward hook. So on a host
# whose kernel FORWARD policy is `drop` (Docker/libvirt/ufw installed), the guest→dst packet is dropped by
# the HOST firewall, not by Shrek — an EXTERNAL condition, not an egress-plane defect. Default behavior:
# detect it and classify BM1 reach as SKIP (reach is proven by the oracle + sealed VM), never a FAIL, and
# NEVER mutate the host firewall. Opt-in only (SHREK_BM_FORWARD_ALLOW=1) installs a narrow, self-cleaning
# FORWARD ACCEPT for the Shrek supernet to get a full bare-metal reach proof on hardened hosts.
FWD_POLICY=$(iptables -S FORWARD 2>/dev/null | grep -m1 '^-P FORWARD' | awk '{print $3}')
FWD_POLICY=${FWD_POLICY:-UNKNOWN}
if [ "$FWD_POLICY" = DROP ] && [ "${SHREK_BM_FORWARD_ALLOW:-0}" = 1 ]; then
  iptables -I FORWARD 1 -s "$SHREK_SUPERNET" -j ACCEPT 2>/dev/null && \
  iptables -I FORWARD 1 -d "$SHREK_SUPERNET" -j ACCEPT 2>/dev/null && FW_ALLOWED=1
  [ "$FW_ALLOWED" = 1 ] && echo "note: SHREK_BM_FORWARD_ALLOW=1 — installed a narrow self-cleaning FORWARD ACCEPT for $SHREK_SUPERNET (opt-in reach proof)"
fi

run(){ # each run under its own delegated scope so the cgroup dance is clean per construction
  systemd-run --scope --quiet -p Delegate=yes \
    --setenv=SHREK_T2_RUNSC="$RUNSC" --setenv=SHREK_T2_ROOTFS="$ROOTFS" --setenv=SHREK_INGEST_ADMIT="$ADMIT" \
    --setenv=SHREK_T2_DEBUG=1 \
    "$GK" sandbox "$@" 2>&1
}
# On a boot failure, dump WHY the runsc sandbox died (SHREK_T2_DEBUG makes runsc write per-subcommand
# logs to /run/shrek-t2-debug/<id>/; the boot log carries the platform/KVM failure reason).
dump_runsc_debug(){ local id="$1"; local d="/run/shrek-t2-debug/$id"
  echo "  ---- runsc debug ($d) ----"
  if [ -d "$d" ]; then
    grep -rhiE 'kvm|platform|panic|fatal|error|fail|unsupported|not permitted|denied|eperm|enosys|mmap|ioctl|vcpu|bluepill|sentry' "$d" 2>/dev/null | tail -30 | sed 's/^/    /'
    echo "  ---- (tail of newest log) ----"; tail -20 "$(ls -t "$d"/* 2>/dev/null | head -1)" 2>/dev/null | sed 's/^/    /'
  else echo "    (no debug dir — sandbox failed before runsc wrote logs)"; fi
}

echo
echo "=== BM0: gVisor --platform=kvm boots + runs a minimal sandbox (is kvm USABLE on this bare-metal host?) ==="
# Direct runsc on a /tmp (exec) bundle — isolates the platform question from gatekeeperd's /run staging,
# so a host with a hardened noexec /run still gives a clean yes/no on the kvm engine itself.
BM0="$WORK/bm0"; mkdir -p "$BM0/rootfs/bin"; cp /usr/bin/busybox "$BM0/rootfs/bin/"; ln -sf busybox "$BM0/rootfs/bin/echo"
cat > "$BM0/config.json" <<EOF
{"ociVersion":"1.0.2","process":{"args":["/bin/echo","KVM-BOOT-OK"],"cwd":"/","user":{"uid":0,"gid":0},"capabilities":{}},"root":{"path":"$BM0/rootfs","readonly":true},"mounts":[],"linux":{"namespaces":[{"type":"pid"},{"type":"mount"},{"type":"ipc"},{"type":"uts"}]}}
EOF
b0=$("$RUNSC" --root "$BM0/st" --ignore-cgroups --network=none --platform=kvm run --bundle "$BM0" bm0 2>&1)
"$RUNSC" --root "$BM0/st" delete --force bm0 2>/dev/null
echo "$b0" | grep -q KVM-BOOT-OK && pass "BM0 ★ gVisor --platform=kvm boots + runs on bare metal → kvm is USABLE here" || fail "BM0 kvm did not boot: $(echo "$b0" | tail -1)"

echo
echo "=== BM1: authentic harness + C-net + github-https ⇒ REAL --platform=kvm, guest EXECUTES + REACHES pinned dst ==="
# Guest EXECUTES a real binary (echo) THEN nc's the pinned endpoint. The echo is the direct bare-metal
# proof of the slice-6 staging fix: a guest binary runs under kvm on this host's noexec /run (the whole
# point of P6-1c). The nc reach is the netstack+nft egress proof, gated on the host FORWARD policy below.
OUT=$(run --tier T2 --trust T-hostile --caps C-net --profile C-net --ingest-harness \
        --id bm1 --anchor "$ANCHOR" --grant project --egress-profile github-https \
        -- /bin/sh -c 'PATH=/bin; export PATH; echo GUEST-EXEC-OK; nc -w6 10.20.0.2 443 </dev/null')
echo "$OUT" | grep -oE 'SANDBOX-DECISION cleared[^\n]*|egress plane up[^\n]*|GUEST-EXEC-OK|SHREK_EGRESS_OK' | sed 's/^/    /'
echo "$OUT" | grep -q "mode=ingest-harness derived=T-untrust" && pass "BM1 derived=T-untrust" || fail "BM1 derived=T-untrust"
echo "$OUT" | grep -q "construct-at=T2 effective=T2" && pass "BM1 construct-at=T2" || fail "BM1 construct-at=T2"
if echo "$OUT" | grep -q "platform=kvm"; then pass "BM1 ★ select_platform chose REAL kvm (bare metal)"; else fail "BM1 platform!=kvm ($(echo "$OUT" | grep -oE 'platform=[a-z]+' | head -1)) — not exercising kvm"; fi
echo "$OUT" | grep -q "netstack --network=sandbox" && pass "BM1 netstack egress path" || fail "BM1 netstack path"
# ★ slice-6 staging fix: the guest binary executed under kvm on a noexec /run (via gatekeeperd's own exec tmpfs).
echo "$OUT" | grep -q "GUEST-EXEC-OK" && pass "BM1 ★ guest EXECUTED under kvm on noexec /run (self-contained exec-tmpfs staging)" || { fail "BM1 guest did not execute (staging fix regressed?)"; dump_runsc_debug bm1; }
if echo "$OUT" | grep -q "SHREK_EGRESS_OK"; then pass "BM1 ★ workload REACHED pinned dst from inside gVisor(kvm) via netstack"
elif [ "$FWD_POLICY" = DROP ] && [ "$FW_ALLOWED" != 1 ]; then
  skip "BM1 reach EXTERNALLY BLOCKED by host FORWARD=drop (Docker/libvirt/ufw) — NOT an egress-plane defect; the plane masquerades correctly but won't override the host's global forward hook. Reach is proven by scripts/p6-egress-proof.sh (E1) + the sealed VM. Re-run with SHREK_BM_FORWARD_ALLOW=1 for a full bare-metal reach proof."
else
  fail "BM1 no reach marker (host FORWARD=$FWD_POLICY, allowance=$FW_ALLOWED)"; echo "$OUT" | grep -iE 'sandbox|kvm|platform|error|EOF' | sed 's/^/    runsc: /'; dump_runsc_debug bm1
fi

echo
echo "=== BM2: same session, NON-allowed dst ⇒ NOT reached (no leak) ==="
OUT=$(run --tier T2 --trust T-hostile --caps C-net --profile C-net --ingest-harness \
        --id bm2 --anchor "$ANCHOR" --grant project --egress-profile github-https \
        -- /bin/sh -c 'PATH=/bin; export PATH; nc -w3 10.20.0.3 443 </dev/null; echo RC=$?')
echo "$OUT" | grep -oE 'RC=[0-9]+|SHREK_EGRESS_OK' | sed 's/^/    /'
if echo "$OUT" | grep -q SHREK_EGRESS_OK; then fail "BM2 reached a non-allowed dst (LEAK!)"
elif echo "$OUT" | grep -qE 'RC=[^0 ]'; then
  if [ "$FWD_POLICY" = DROP ] && [ "$FW_ALLOWED" != 1 ]; then pass "BM2 non-allowed dst not reached (host FORWARD=drop also blocks; the per-sandbox default-deny is proven by the oracle E2)"
  else pass "BM2 non-allowed dst dropped by the per-sandbox nft default-deny (nc rc!=0)"; fi
else fail "BM2 unexpected: $(echo "$OUT" | tr '\n' ' ')"; fi

echo
echo "=== BM3: teardown ⇒ no residual netns / veth / nft ==="
r_ns=$(ip netns list 2>/dev/null | grep -oE 'shrek-bm[0-9]+' | tr '\n' ,)
r_if=$(ip -o link show 2>/dev/null | grep -oE 'skh[0-9a-f]{4}' | tr '\n' ,)
r_tb=$(nft list tables 2>/dev/null | grep -o 'shrek_egress_[a-z0-9_]*' | tr '\n' ,)
{ [ -z "$r_ns" ] && [ -z "$r_if" ] && [ -z "$r_tb" ]; } && pass "BM3 no residual egress plumbing" || fail "BM3 residual ns=[$r_ns] if=[$r_if] tb=[$r_tb]"

echo
if [ $fails -eq 0 ]; then
  if [ $skips -gt 0 ]; then echo "P6-EGRESS-BAREMETAL: PASS ✅ ($skips skipped — see notes; kvm platform + exec-tmpfs staging + deny + teardown proven on real hardware)"
  else echo "P6-EGRESS-BAREMETAL: ALL PASS ✅ (kvm platform + egress reach + staging proven on real hardware)"; fi
  exit 0
else echo "P6-EGRESS-BAREMETAL: $fails FAIL ❌ ($skips skipped)"; exit 1; fi
