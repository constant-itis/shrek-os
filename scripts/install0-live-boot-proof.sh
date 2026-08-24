#!/usr/bin/env bash
# INSTALL-0 live boot proof: boot the live installer image with the installer store, payload disk,
# and a blank target disk attached. This validates the live-media composition and captures a screenshot
# for the Calamares autostart surface; the destructive writer path is covered by install0-writer-proof.sh.
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"; mkdir -p out

RAW="${RAW:-$(ls -t out/shrek_*_x86-64.raw 2>/dev/null | head -1)}"
[ -n "$RAW" ] && [ -f "$RAW" ] || { echo "no out/shrek_*_x86-64.raw - run LIVE_INSTALLER=1 scripts/build-in-container.sh 1 first" >&2; exit 1; }

STORE="${STORE:-out/layer-store-installer.raw}"
[ -f "$STORE" ] || { echo "missing STORE=$STORE - run scripts/build-layers.sh installer first" >&2; exit 1; }

PAYLOAD="${PAYLOAD:-out/shrek-install-payload.raw}"
[ -f "$PAYLOAD" ] || { echo "missing PAYLOAD=$PAYLOAD - run scripts/build-installer-payload.sh first" >&2; exit 1; }

STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
TARGET="${TARGET:-out/install0-live-target-$STAMP.raw}"
LOG="${LOG:-out/install0-live-boot-$STAMP.log}"
SHOT="${SHOT:-out/install0-live-boot-$STAMP.png}"
BUDGET="${BUDGET:-190}"
SHOT_AT="${SHOT_AT:-155}"

[ ! -e "$TARGET" ] || { echo "refusing to overwrite TARGET=$TARGET" >&2; exit 1; }
: > "$LOG"
truncate -s "${TARGET_SIZE:-18G}" "$TARGET"

echo "=== INSTALL-0 live boot: raw=$RAW store=$STORE payload=$PAYLOAD target=$TARGET budget=${BUDGET}s ==="
docker run --rm --device /dev/kvm \
  -v "${REPO_ROOT}:/work" -w /work \
  -e RAW="$RAW" -e STORE="$STORE" -e PAYLOAD="$PAYLOAD" -e TARGET="$TARGET" \
  -e LOG="$LOG" -e SHOT="$SHOT" -e BUDGET="$BUDGET" -e SHOT_AT="$SHOT_AT" \
  debian:trixie \
  bash -euo pipefail -c '
    apt-get update -qq >/dev/null
    apt-get install -y --no-install-recommends -qq qemu-system-x86 ovmf socat netpbm >/dev/null
    tmp=$(mktemp -d); mon="$tmp/mon.sock"
    cp /usr/share/OVMF/OVMF_VARS_4M.fd "$tmp/vars.fd"

    qemu-system-x86_64 \
      -machine q35,smm=on -accel kvm -cpu host -m 4096 -smp 4 \
      -global driver=cfi.pflash01,property=secure,value=on \
      -drive if=pflash,format=raw,unit=0,file=/usr/share/OVMF/OVMF_CODE_4M.secboot.fd,readonly=on \
      -drive if=pflash,format=raw,unit=1,file="$tmp/vars.fd" \
      -drive file="/work/$RAW",format=raw,if=virtio,snapshot=on \
      -drive file="/work/$STORE",format=raw,if=virtio,snapshot=on \
      -drive file="/work/$PAYLOAD",format=raw,if=virtio,snapshot=on \
      -drive file="/work/$TARGET",format=raw,if=virtio \
      -device virtio-vga -device virtio-keyboard-pci -device virtio-tablet-pci -device virtio-rng-pci \
      -display none -serial file:/work/$LOG \
      -monitor "unix:$mon,server,nowait" &
    qpid=$!

    sleep "$SHOT_AT"
    printf "screendump /work/%s.ppm\n" "${SHOT%.png}" | socat - "UNIX-CONNECT:$mon" >/dev/null 2>&1 || true
    [ -s "/work/${SHOT%.png}.ppm" ] && pnmtopng "/work/${SHOT%.png}.ppm" > "/work/$SHOT" 2>/dev/null && rm -f "/work/${SHOT%.png}.ppm" || true

    remain=$((BUDGET - SHOT_AT))
    [ "$remain" -gt 0 ] && sleep "$remain"
    printf "quit\n" | socat - "UNIX-CONNECT:$mon" >/dev/null 2>&1 || true
    kill "$qpid" 2>/dev/null || true
    wait "$qpid" 2>/dev/null || true
  '

echo "=== serial log: $LOG ($(wc -l < "$LOG" 2>/dev/null || echo 0) lines) ==="
[ -s "$SHOT" ] && echo "=== screenshot: $SHOT ===" || echo "=== screenshot missing: $SHOT ==="

pass=0; fail=0
ok() { echo "  PASS $*"; pass=$((pass + 1)); }
bad() { echo "  FAIL $*"; fail=$((fail + 1)); }

grep -qa "shrek-installer" "$LOG" && ok "installer sysext mentioned on serial" || bad "installer sysext not mentioned on serial"
grep -qa "graphical.target" "$LOG" && ok "graphical target reached" || bad "graphical target not reached"
grep -qa 'session_open.*acct="dev".*exe="/usr/bin/login"' "$LOG" && ok "dev autologin opened a session" || bad "dev autologin session not observed"
# INSTALL-0 now presents the Shrek target-disk picker before Calamares, so a
# headless (un-clicked) boot reaches the picker but not Calamares itself. Assert
# the installer flow reached target enumeration (and, ideally, found a target).
grep -qaE "enumerating install targets|eligible target" "$LOG" && ok "Shrek target picker reached on serial" || bad "Shrek target picker not reached on serial"
grep -qaE "[1-9][0-9]* eligible target" "$LOG" && ok "at least one eligible target enumerated" || bad "no eligible target enumerated (picker would be empty)"
if grep -qaE "traps: xdg-desktop-por|comm=\"xdg-desktop-por\".*sig=5" "$LOG"; then
  bad "xdg-desktop-portal crashed with SIGTRAP"
else
  ok "no xdg-desktop-portal SIGTRAP observed"
fi
[ -s "$SHOT" ] && ok "graphical screenshot captured" || bad "graphical screenshot was not captured"

echo "--- INSTALL-0 live boot tally: PASS=$pass FAIL=$fail ---"
[ "$fail" -eq 0 ] && echo "=== INSTALL-0 live boot proof GREEN ===" || { echo "=== INSTALL-0 live boot proof NOT GREEN - inspect $LOG and $SHOT ==="; exit 1; }
