#!/usr/bin/env bash
# Shrek OS Phase-1 — S6 (the core acceptance gate): boot the signed + dm-verity-sealed image in a
# throwaway KVM VM under OVMF Secure Boot, headless, serial captured to out/vm-console.log.
#
# Runs qemu INSIDE a debian:trixie container (modern qemu + ovmf, /dev/kvm passthrough) so the build host
# stays untouched — same hermetic pattern as scripts/build-in-container.sh.
#
# BOOT CYCLE: first boot, OVMF is in SETUP MODE (blank OVMF_VARS) → systemd-boot auto-enrolls the
# Shrek key (staged in the ESP by SecureBootAutoEnroll=yes) and reboots → second boot runs under
# ENFORCING Secure Boot: firmware verifies systemd-boot → systemd-boot verifies the signed UKI →
# the UKI's sealed cmdline (roothash=…) drives systemd-veritysetup to mount the dm-verity root.
# One qemu invocation covers both boots (guest reboot = qemu reset, no -no-reboot).
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"
# S7: images are versioned (out/shrek_<v>_x86-64.raw). Default to the newest built raw; override with
# RAW=out/shrek_2_x86-64.raw scripts/boot-vm.sh to boot a specific version (e.g. the A/B-updated disk).
RAW="${RAW:-$(ls -t out/shrek_*_x86-64.raw 2>/dev/null | head -1)}"
[ -n "$RAW" ] && [ -f "$RAW" ] || { echo "no out/shrek_*_x86-64.raw — run scripts/build-in-container.sh first" >&2; exit 1; }
echo "=== booting $RAW ==="

# Phase-2 (Onion): optional second drive = the layer store (STORE=out/layer-store.raw). Attached as a
# virtio disk; shrek-onion.service mounts it by fs-label `shrek-layers` and merges the signed layers.
# snapshot=on so the store artifact is never mutated by the guest.
STORE="${STORE:-}"
STORE_DRIVE=""
if [ -n "$STORE" ]; then
  [ -f "$STORE" ] || { echo "STORE=$STORE not found" >&2; exit 1; }
  STORE_DRIVE="-drive file=/work/${STORE},format=raw,if=virtio,snapshot=on"
  echo "=== attaching layer store: $STORE ==="
fi
QEMU_SECONDS="${1:-150}"     # wall-clock budget for the two-boot cycle
LOG=out/vm-console.log
: > "$LOG"

echo "=== S6: booting $RAW under OVMF Secure Boot (headless, ${QEMU_SECONDS}s budget) ==="
docker run --rm --device /dev/kvm \
  -v "${REPO_ROOT}:/work" -w /work \
  debian:trixie \
  bash -euo pipefail -c '
    apt-get update -qq >/dev/null
    apt-get install -y --no-install-recommends -qq qemu-system-x86 ovmf >/dev/null
    tmp=$(mktemp -d)
    # Writable copy of the SETUP-MODE (blank) OVMF vars so systemd-boot can enroll into them.
    cp /usr/share/OVMF/OVMF_VARS_4M.fd "$tmp/vars.fd"
    set +e
    # snapshot=on: guest writes go to a discarded temp overlay — the built raw artifact is never
    # mutated, and we avoid a qemu-img/qemu-utils dependency.
    timeout '"$QEMU_SECONDS"' qemu-system-x86_64 \
      -machine q35,smm=on -accel kvm -cpu host -m 2048 -smp 2 \
      -global driver=cfi.pflash01,property=secure,value=on \
      -drive if=pflash,format=raw,unit=0,file=/usr/share/OVMF/OVMF_CODE_4M.secboot.fd,readonly=on \
      -drive if=pflash,format=raw,unit=1,file="$tmp/vars.fd" \
      -drive file="/work/'"$RAW"'",format=raw,if=virtio,snapshot=on \
      '"$STORE_DRIVE"' \
      -device virtio-rng-pci \
      -display none -serial file:/work/'"$LOG"'
    rc=$?
    # 124 = timeout reached (VM ran the full budget — expected for a system that boots and stays up).
    echo "qemu exited rc=$rc"
  '
echo "=== serial log: $LOG ($(wc -l < "$LOG") lines) ==="
