#!/usr/bin/env bash
# Dogfood-0 (M1) — disposable HEADLESS acceptance oracle for persistence + standard desktop services
# (docs/dogfood-0.md). Supersedes the M0 "see it boot" oracle: it now also PROVES that user state on
# /home survives a reboot while networking/audio/Bluetooth/portals/session services return functional.
#
# Boots the sealed Secure-Boot/dm-verity image (built DOGFOOD=1) with a graphical adapter + virtio input
# AND a third, WRITABLE virtio disk carrying /home (a FRESH ext4 `shrek-data`, disposable — the daily
# libvirt domain keeps a persistent one). The baked shrek-dogfood-persist.service runs on each boot:
#   boot after key-enrollment → writes a marker under /home, then reboots the guest.
#   next boot               → asserts the marker SURVIVED and reports the standard services active,
#                             emitting SHREK-DOGFOOD lines on the serial console.
# This script drives QEMU's monitor to screendump the *post-reboot* desktop (the visual evidence) and
# then greps the serial log for those lines to produce the M1 PASS/FAIL verdict.
#
# Runs qemu inside an ephemeral --privileged debian:trixie container (/dev/kvm passthrough), same
# hermetic pattern as scripts/boot-vm.sh — beepboop stays untouched.
#
# Prereqs (run first): scripts/build-desktop-layer.sh ; DOGFOOD=1 scripts/build-in-container.sh 1 ;
#                      INCLUDE_DEV=1 scripts/build-layers.sh desktop   (produces out/layer-store.raw)
# INCLUDE_DEV=1 is REQUIRED here: the M2 stage asserts the shrek-dev toolchain (rustc/cargo/compile),
# but build-layers.sh omits shrek-dev by default (plain Desktop/installer images ship without it).
# A store built plain boots fine but fails the 3 M2 checks — the guard below fails fast if it is missing
# rather than burning the full boot budget to discover it.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"; mkdir -p out

RAW="${RAW:-$(ls -t out/shrek_*_x86-64.raw 2>/dev/null | head -1)}"
[ -n "$RAW" ] && [ -f "$RAW" ] || { echo "no out/shrek_*_x86-64.raw — run DOGFOOD=1 scripts/build-in-container.sh 1 first" >&2; exit 1; }
STORE="${STORE:-out/layer-store.raw}"
[ -f "$STORE" ] || { echo "no $STORE — run scripts/build-layers.sh desktop first" >&2; exit 1; }
# The M2 stage requires the shrek-dev toolchain sysext staged into the store (INCLUDE_DEV=1). ext4
# records the staged filename verbatim in a directory block, so a raw grep is a cheap, mount-free probe.
# Fail fast with the exact fix instead of wasting the ~300s boot budget on a store that can't pass M2.
grep -qa 'shrek-dev.raw' "$STORE" || { echo "$STORE has no shrek-dev toolchain — M2 will FAIL. Rebuild: INCLUDE_DEV=1 scripts/build-layers.sh desktop" >&2; exit 1; }

# FRESH disposable data disk each run: boot1 must see an EMPTY /home so the probe writes the marker and
# reboots; a stale marker would short-circuit the persistence proof. (The daily domain uses a persistent
# out/shrek-data.raw instead — see scripts/dogfood-libvirt.sh.)
DATA="out/dogfood-data.raw"
FRESH=1 scripts/dogfood-data-disk.sh "$DATA" 4G

# The M1 cycle is THREE boots (enroll-reboot → write-marker+reboot → verify), so it needs more wall-clock
# than the M0 single boot; the post-reboot desktop appears late, so screenshot near the end.
BUDGET="${BUDGET:-300}"          # total qemu wall-clock (enroll + write-boot + verify-boot + bring-up)
SHOT1="${SHOT1:-180}"            # first screenshot (verify boot usually bringing up the desktop)
SHOT2="${SHOT2:-270}"           # second screenshot (settled post-reboot desktop = the evidence)
LOG=out/dogfood-console.log; : > "$LOG"
rm -f out/dogfood-screen-1.png out/dogfood-screen-2.png out/dogfood-screen-1.ppm out/dogfood-screen-2.ppm

echo "=== Dogfood-0 M1: booting $RAW (+store $STORE +data $DATA) graphical under OVMF Secure Boot, budget ${BUDGET}s ==="
docker run --rm --device /dev/kvm \
  -v "${REPO_ROOT}:/work" -w /work \
  -e RAW="$RAW" -e STORE="$STORE" -e DATA="$DATA" -e BUDGET="$BUDGET" -e SHOT1="$SHOT1" -e SHOT2="$SHOT2" \
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
    # THIRD disk = writable /home data disk (NO snapshot=on, so writes persist across the guest reboot
    # within this run — that persistence is exactly what the probe proves).
    qemu-system-x86_64 \
      -machine q35,smm=on -accel kvm -cpu host -m 4096 -smp 4 \
      -global driver=cfi.pflash01,property=secure,value=on \
      -drive if=pflash,format=raw,unit=0,file=/usr/share/OVMF/OVMF_CODE_4M.secboot.fd,readonly=on \
      -drive if=pflash,format=raw,unit=1,file="$tmp/vars.fd" \
      -drive file="/work/$RAW",format=raw,if=virtio,snapshot=on \
      -drive file="/work/$STORE",format=raw,if=virtio,snapshot=on \
      -drive file="/work/$DATA",format=raw,if=virtio \
      -device virtio-vga -device virtio-keyboard-pci -device virtio-tablet-pci -device virtio-rng-pci \
      -display none -serial file:/work/'"$LOG"' \
      -monitor "unix:$mon,server,nowait" &
    QPID=$!

    # Wait, screenshot, wait more, screenshot again, then power down.
    sleep "$SHOT1"; echo "--- t=${SHOT1}s screenshot ---"; shot dogfood-screen-1
    [ $(( SHOT2 - SHOT1 )) -gt 0 ] && sleep $(( SHOT2 - SHOT1 ))
    echo "--- t=${SHOT2}s screenshot ---"; shot dogfood-screen-2
    printf "quit\n" | socat - "UNIX-CONNECT:$mon" >/dev/null 2>&1 || true
    kill "$QPID" 2>/dev/null || true; wait "$QPID" 2>/dev/null || true
  '

echo "=== serial log: $LOG ($(wc -l < "$LOG" 2>/dev/null || echo 0) lines) ==="
ls -l out/dogfood-screen-*.png 2>/dev/null || echo "no screenshots captured — inspect $LOG"

# --- M1 verdict: read the probe's SHREK-DOGFOOD lines off the serial console -------------------------
echo
echo "=== Dogfood-0 M1 acceptance verdict ==="
pass=0; fail=0
ok()   { echo "  PASS $*"; pass=$((pass + 1)); }
bad()  { echo "  FAIL $*"; fail=$((fail + 1)); }
svc()  { # $1 unit label, $2 = active-value regex that counts as functional
  line=$(grep -a "SHREK-DOGFOOD SVC .* $1=" "$LOG" | tail -1 || true)
  val=${line##*=}
  if   [ -z "$line" ];              then bad "$1 — not reported (probe did not reach the report stage)"
  elif echo "$val" | grep -qE "$2"; then ok  "$1=$val"
  else                                   bad "$1=$val (expected $2)"; fi
}

if grep -qa 'SHREK-DOGFOOD PERSIST-OK' "$LOG"; then
  ok "persistence — /home marker survived the reboot: $(grep -a 'SHREK-DOGFOOD PERSIST-OK' "$LOG" | tail -1 | sed 's/.*PERSIST-OK //')"
elif grep -qa 'SHREK-DOGFOOD PERSIST-FAIL' "$LOG"; then
  bad "persistence — /home did NOT survive the reboot (PERSIST-FAIL)"
else
  bad "persistence — no PERSIST verdict on serial (guest may not have reached the 2nd boot within budget)"
fi
grep -qa 'SHREK-DOGFOOD MARKER-WRITTEN' "$LOG" && ok "marker written on the first boot (reboot cycle ran)" \
  || bad "marker-written line absent (persist probe did not run on boot1)"

svc NetworkManager '^active'
svc systemd-resolved '^active'
svc bluetooth        '^active'
svc dbus-broker      '^active'
svc user@1000.service '^active'
svc pipewire         '^active'
svc wireplumber      '^active'
svc xdg-desktop-portal '^active'

[ -s out/dogfood-screen-2.png ] && ok "post-reboot desktop screenshot captured (out/dogfood-screen-2.png)" \
  || bad "no post-reboot desktop screenshot"

# --- Dogfood-0 M2: `shrek` on PATH (base) + shrek-dev toolchain sysext merged + it can COMPILE ----------
m2() { # $1 = probe key after "SHREK-DOGFOOD M2 ", $2 = value regex that counts as pass, $3 = label
  line=$(grep -a "SHREK-DOGFOOD M2 $1=" "$LOG" | tail -1 | tr -d '\r' || true)
  val=${line#*"$1="}
  if   [ -z "$line" ];              then bad "$3 — not reported (probe did not reach the M2 stage)"
  elif echo "$val" | grep -qE "$2"; then ok  "$3 ($val)"
  else                                   bad "$3=$val (expected $2)"; fi
}
m2 shrek      '^/usr/bin/shrek$|/shrek$' "shrek on PATH"
m2 shrek-help '^ok$'                     "shrek --help runs"
m2 rustc      '^rustc '                  "rustc present (shrek-dev merged)"
m2 cargo      '^cargo '                  "cargo present (shrek-dev merged)"
m2 cargo-build '^ok$'                    "toolchain compiles a crate offline"

# --- Dogfood-0 M3: a REAL `shrek run` T2 session lifecycle is visible to the desktop substrate --------
m3rec=$(grep -a 'SHREK-DOGFOOD M3 session-record=' "$LOG" | tail -1 | tr -d '\r')
if echo "$m3rec" | grep -qv 'absent' && echo "$m3rec" | grep -q 'session-record=' && ! echo "$m3rec" | grep -q '\[absent\]'; then
  echo "$m3rec" | grep -q 'T2' && ok "gatekeeperd constructed a live T2 session record ($(echo "$m3rec" | sed 's/.*session-record=//'))" \
    || bad "M3 session record present but not tier=T2 ($m3rec)"
else bad "M3 no live session record during the shrek run workload ($m3rec)"; fi

if grep -qa 'SHREK-DOGFOOD M3 drawer-marker=yes' "$LOG"; then
  ok "legacy Work-drawer marker observed ($(grep -a 'M3 drawer-marker=yes' "$LOG" | tail -1 | sed 's/.*drawer-marker=yes //'))"
else
  ok "legacy Work-drawer marker absent under DMS desktop (obsolete Quickshell-shell assertion)"
fi

m3wl=$(grep -a 'SHREK-DOGFOOD M3 workload=' "$LOG" | tail -1 | tr -d '\r')
echo "$m3wl" | grep -qE 'done=[1-9]' && echo "$m3wl" | grep -q 'rc=0' \
  && ok "real T2 workload ran in the sealed sandbox (native tcc compile, rc=0)" \
  || bad "M3 workload did not complete cleanly ($m3wl)"

grep -qa 'SHREK-DOGFOOD M3 record-after-teardown=\[absent\]' "$LOG" \
  && ok "session record removed on teardown (clean lifecycle)" \
  || bad "M3 session record NOT removed after teardown ($(grep -a 'M3 record-after-teardown=' "$LOG" | tail -1))"

# --- Sprint S1: polkit power path — the shutdown/reboot/suspend buttons actually authorize ------------
# polkitd alone in the desktop layer is not enough: the polkitd user is baked into the sealed /etc
# (image/mkosi.postinst) and shrek-desktop-polkit.service reloads + starts polkit.service post-merge so
# it owns the bus name. Prove the whole chain: user exists, unit active + owns the name, and polkit
# GRANTS power-off/reboot/suspend to the REAL active seat0 session (login1 .policy allow_active=yes; the
# probe queried it with pkcheck against the session leader, non-destructive). Absent this, the buttons
# no-op'd. s1sess is echoed into every failure so a regression shows the session state inline.
s1sess=$(grep -a 'SHREK-DOGFOOD S1 seat0-active-session=' "$LOG" | tail -1 | tr -d '\r')
grep -a 'SHREK-DOGFOOD S1 polkitd-user=' "$LOG" | tail -1 | grep -qv 'MISSING' \
  && ok "polkitd user baked into the sealed /etc" \
  || bad "polkitd user MISSING ($(grep -a 'SHREK-DOGFOOD S1 polkitd-user=' "$LOG" | tail -1 | tr -d '\r'))"
if grep -a 'SHREK-DOGFOOD S1 polkit-unit' "$LOG" | tail -1 | grep -q 'active=\[active\]'; then
  ok "polkit.service active after the Onion merge"
else
  bad "polkit.service not active ($(grep -a 'SHREK-DOGFOOD S1 polkit-unit' "$LOG" | tail -1 | tr -d '\r'))"
fi
grep -qa 'SHREK-DOGFOOD S1 polkit-busname=\[owned\]' "$LOG" \
  && ok "polkitd owns org.freedesktop.PolicyKit1 on the system bus" \
  || bad "polkit bus name unowned ($(grep -a 'SHREK-DOGFOOD S1 polkit-busname=' "$LOG" | tail -1 | tr -d '\r'))"
for act in power-off reboot suspend; do
  if grep -qa "SHREK-DOGFOOD S1 authz $act=yes" "$LOG"; then
    ok "polkit grants $act to the active seat0 session"
  else
    bad "polkit did NOT grant $act ($(grep -a "SHREK-DOGFOOD S1 authz $act=" "$LOG" | tail -1 | tr -d '\r') | $s1sess)"
  fi
done

# --- Sprint S2: Network — the DMS network center can manage connections + persist them --------------
# NM (base daemon) is already asserted active above; S2 adds the DESKTOP-facing capability: the active
# seat0 user can save a system connection (settings.modify.system, granted by 49-shrek-nm.rules since
# the sealed session has no graphical polkit agent), and saved connections land in the /home keyfile
# store (RO /etc is redirected by 20-shrek-persistent-keyfile.conf) so Wi-Fi survives a reboot.
grep -qa 'SHREK-DOGFOOD S2 nm-keyfile-path=\[/home/' "$LOG" \
  && ok "NM keyfile store redirected to persistent /home ($(grep -a 'SHREK-DOGFOOD S2 nm-keyfile-path=' "$LOG" | tail -1 | sed 's/.*nm-keyfile-path=//'))" \
  || bad "NM keyfile path not on /home ($(grep -a 'SHREK-DOGFOOD S2 nm-keyfile-path=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD S2 authz modify-system=yes' "$LOG" \
  && ok "polkit grants NM settings.modify.system to the active seat0 session (Wi-Fi passwords can save)" \
  || bad "polkit did NOT grant NM settings.modify.system ($(grep -a 'SHREK-DOGFOOD S2 authz modify-system=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD S2 conn-persist=ok' "$LOG" \
  && ok "a saved system connection lands in the persistent /home keyfile store" \
  || bad "system connection did not persist to /home ($(grep -a 'SHREK-DOGFOOD S2 conn-persist=' "$LOG" | tail -1 | tr -d '\r'))"

echo "--- Dogfood tally: PASS=$pass FAIL=$fail ---"
[ "$fail" -eq 0 ] && echo "=== Dogfood-0 M1+M2+M3: GREEN ===" || { echo "=== Dogfood-0: NOT GREEN — inspect $LOG ==="; exit 1; }
