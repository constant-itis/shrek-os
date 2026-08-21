#!/usr/bin/env bash
# Dogfood-0 (M0) — disposable HEADLESS acceptance oracle for the interactive desktop (docs/dogfood-0.md).
#
# Boots the sealed, Secure-Boot, dm-verity image (built with DOGFOOD=1) with a GRAPHICAL adapter
# (virtio-vga) + virtio input under OVMF, the same firmware/secboot/layer-store assumptions as the
# libvirt daily-driver domain (scripts/dogfood-libvirt.sh) — but headless and reproducible. It cannot
# "look" at the screen, so instead it drives QEMU's monitor to `screendump` the guest scanout at two
# moments and converts them to PNG: the images ARE the evidence that the boot lands at the real
# Sway + Quickshell desktop (root tty1 autologin → shrek-desktop → sway → quickshell).
#
# Runs qemu inside an ephemeral --privileged debian:trixie container (/dev/kvm passthrough), same
# hermetic pattern as scripts/boot-vm.sh — beepboop stays untouched.
#
# Prereqs (run first): scripts/build-desktop-layer.sh ; DOGFOOD=1 scripts/build-in-container.sh 1 ;
#                      scripts/build-layers.sh desktop   (produces out/layer-store.raw)
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"; mkdir -p out

RAW="${RAW:-$(ls -t out/shrek_*_x86-64.raw 2>/dev/null | head -1)}"
[ -n "$RAW" ] && [ -f "$RAW" ] || { echo "no out/shrek_*_x86-64.raw — run DOGFOOD=1 scripts/build-in-container.sh 1 first" >&2; exit 1; }
STORE="${STORE:-out/layer-store.raw}"
[ -f "$STORE" ] || { echo "no $STORE — run scripts/build-layers.sh desktop first" >&2; exit 1; }

BUDGET="${BUDGET:-165}"          # total qemu wall-clock (covers enroll-reboot + 2nd boot + desktop bring-up)
SHOT1="${SHOT1:-105}"            # first screenshot (desktop usually up by now)
SHOT2="${SHOT2:-150}"           # second screenshot (settled)
LOG=out/dogfood-console.log; : > "$LOG"
rm -f out/dogfood-screen-1.png out/dogfood-screen-2.png out/dogfood-screen-1.ppm out/dogfood-screen-2.ppm

echo "=== Dogfood-0 M0: booting $RAW (+store $STORE) graphical under OVMF Secure Boot, budget ${BUDGET}s ==="
docker run --rm --device /dev/kvm \
  -v "${REPO_ROOT}:/work" -w /work \
  -e RAW="$RAW" -e STORE="$STORE" -e BUDGET="$BUDGET" -e SHOT1="$SHOT1" -e SHOT2="$SHOT2" \
  debian:trixie \
  bash -euo pipefail -c '
    apt-get update -qq >/dev/null
    apt-get install -y --no-install-recommends -qq qemu-system-x86 ovmf socat netpbm >/dev/null
    tmp=$(mktemp -d); mon="$tmp/mon.sock"
    cp /usr/share/OVMF/OVMF_VARS_4M.fd "$tmp/vars.fd"   # SETUP-MODE vars → first boot auto-enrolls the Shrek key

    shot() { # $1 = output basename (no ext)
      printf "screendump /work/out/%s.ppm\n" "$1" | socat - "UNIX-CONNECT:$mon" >/dev/null 2>&1 || echo "NOTE screendump $1 failed"
      sleep 1
      [ -s "/work/out/$1.ppm" ] && pnmtopng "/work/out/$1.ppm" > "/work/out/$1.png" 2>/dev/null && rm -f "/work/out/$1.ppm" \
        && echo "captured out/$1.png" || echo "NOTE no scanout for $1"
    }

    # virtio-vga = a real KMS/DRM device in the guest (/dev/dri/card0) whose scanout `screendump` reads;
    # virtio keyboard+tablet = the input the interactive session needs. -display none (headless host).
    qemu-system-x86_64 \
      -machine q35,smm=on -accel kvm -cpu host -m 4096 -smp 4 \
      -global driver=cfi.pflash01,property=secure,value=on \
      -drive if=pflash,format=raw,unit=0,file=/usr/share/OVMF/OVMF_CODE_4M.secboot.fd,readonly=on \
      -drive if=pflash,format=raw,unit=1,file="$tmp/vars.fd" \
      -drive file="/work/$RAW",format=raw,if=virtio,snapshot=on \
      -drive file="/work/$STORE",format=raw,if=virtio,snapshot=on \
      -device virtio-vga -device virtio-keyboard-pci -device virtio-tablet-pci -device virtio-rng-pci \
      -display none -serial file:/work/'"$LOG"' \
      -monitor "unix:$mon,server,nowait" &
    QPID=$!

    # Wait, screenshot, wait more, screenshot again, then power down.
    sleep "$SHOT1"; echo "--- t=${SHOT1}s screenshot ---"; shot dogfood-screen-1
    REM=$(( BUDGET - SHOT1 )); [ "$REM" -gt 0 ] && sleep $(( SHOT2 - SHOT1 ))
    echo "--- t=${SHOT2}s screenshot ---"; shot dogfood-screen-2
    printf "quit\n" | socat - "UNIX-CONNECT:$mon" >/dev/null 2>&1 || true
    kill "$QPID" 2>/dev/null || true; wait "$QPID" 2>/dev/null || true
  '
echo "=== serial log: $LOG ($(wc -l < "$LOG" 2>/dev/null || echo 0) lines) ==="
ls -l out/dogfood-screen-*.png 2>/dev/null || echo "no screenshots captured — inspect $LOG"
