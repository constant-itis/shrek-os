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
# hermetic pattern as scripts/boot-vm.sh — the build host stays untouched.
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

# --- STALENESS GUARD (ADR-008 S5 lesson, 2026-09-04) -------------------------------------------------
# The store + layer DDIs are REUSED across runs. A slice that edits a layer's SOURCE (e.g. shrek-connect,
# menu.jsonc, shell.qml under layers/shrek-desktop/) but forgets to rebuild that layer's DDI makes this
# dogfood boot the OLD artifact and pass GREEN on code that is not in the image — a stale desktop DDI
# (pre-ADR-008 shrek-connect) cost a full S5 cycle before this guard existed. Refuse to PROVE a stale
# artifact: fail fast if any in-store layer's overlay source is newer than its built DDI, or a rebuilt DDI
# is newer than the assembled store. Override with STALE_OK=1 only when the source delta is knowingly moot.
if [ "${STALE_OK:-0}" != 1 ]; then
  stale=""
  for name in shrek-desktop shrek-dev shrek-browser shrek-apps shrek-bench shrek-ai shrek-installer; do
    grep -qa "$name.raw" "$STORE" || continue          # only layers actually staged into THIS store
    ddi="out/layers/$name.raw"
    src="layers/$name"
    [ -f "$ddi" ] || { echo "store references $name.raw but $ddi is missing — rebuild scripts/build-${name#shrek-}-layer.sh" >&2; exit 1; }
    if [ -d "$src" ] && [ -n "$(find "$src" -type f -newer "$ddi" -print -quit 2>/dev/null)" ]; then
      stale="$stale
  - $name: source under $src/ is NEWER than $ddi -> rebuild scripts/build-${name#shrek-}-layer.sh, then reassemble the store"
    elif [ "$ddi" -nt "$STORE" ]; then
      stale="$stale
  - $name: $ddi is NEWER than $STORE -> reassemble: INCLUDE_DEV=1 [INCLUDE_BROWSER=1] scripts/build-layers.sh desktop"
    fi
  done
  if [ -n "$stale" ]; then
    echo "STALE LAYER(S) — this dogfood would prove an OUTDATED artifact, not your current source:$stale" >&2
    echo "Fix the above (or set STALE_OK=1 if you KNOW the delta is irrelevant) before trusting the result." >&2
    exit 1
  fi
fi

# FRESH disposable data disk each run: boot1 must see an EMPTY /home so the probe writes the marker and
# reboots; a stale marker would short-circuit the persistence proof. (The daily domain uses a persistent
# out/shrek-data.raw instead — see scripts/dogfood-libvirt.sh.)
DATA="out/dogfood-data.raw"
# ADR-006 slice-6: when SHREK_AI_GGUF is set, DELIVER the model-as-data GGUF to the fresh /home before boot
# (ADR-006 §3 — the multi-GB model never rides the sealed Onion; it is verified against the baked digest at
# boot). Seeded into /home/.shrek/ai/model via mkfs -d (the non-privileged dogfood container can't
# loopback-mount). Bigger disk to hold the ~2-3GB GGUF + /home. Hardlink (same fs) to avoid a host copy.
DATA_SIZE=4G
if [ -n "${SHREK_AI_GGUF:-}" ]; then
  [ -f "$SHREK_AI_GGUF" ] || { echo "SHREK_AI_GGUF=$SHREK_AI_GGUF not found" >&2; exit 1; }
  SEEDROOT=$(mktemp -d out/ai-seed.XXXXXX)
  mkdir -p "$SEEDROOT/.shrek/ai/model"
  gname=$(basename "$SHREK_AI_GGUF")
  ln -f "$SHREK_AI_GGUF" "$SEEDROOT/.shrek/ai/model/$gname" 2>/dev/null \
    || cp "$SHREK_AI_GGUF" "$SEEDROOT/.shrek/ai/model/$gname"
  chmod 0644 "$SEEDROOT/.shrek/ai/model/$gname"
  # CRITICAL: strip host-inherited POSIX ACLs from the seed tree before mkfs -d stamps it as the /home ROOT
  # inode. out/ carries a DEFAULT ACL (user:libvirt-qemu:rwx, uid 64055) so mktemp -d inherits it, and
  # mktemp's 0700 mode collapses MASK+OTHER to ---. mkfs.ext4 -d then writes that ACL onto /home's root inode,
  # giving OTHER=--- → NO non-root user (dev uid1000, shrek-ai uid702) can traverse /home in the guest. That
  # is the "ACL poison" that failed the AI legs — a pure host-harness artifact, not an OS bug. -Rbk drops
  # access+default ACLs; the chmod re-asserts plain 0755 dirs so /home root is world-traversable, GGUF 0644.
  setfacl -Rbk "$SEEDROOT"
  chmod -R u=rwX,go=rX "$SEEDROOT"
  chmod 0644 "$SEEDROOT/.shrek/ai/model/$gname"
  export DATA_SEED_DIR="$SEEDROOT"
  DATA_SIZE=8G
  echo "=== slice-6: delivering $gname to VM /home/.shrek/ai/model (fresh data disk ${DATA_SIZE}) ==="
fi
FRESH=1 scripts/dogfood-data-disk.sh "$DATA" "$DATA_SIZE"
[ -n "${DATA_SEED_DIR:-}" ] && rm -rf "$DATA_SEED_DIR" && unset DATA_SEED_DIR

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
    apt-get install -y --no-install-recommends -qq qemu-system-x86 ovmf socat netpbm e2fsprogs >/dev/null
    tmp=$(mktemp -d); mon="$tmp/mon.sock"
    cp /usr/share/OVMF/OVMF_VARS_4M.fd "$tmp/vars.fd"   # SETUP-MODE vars → first boot auto-enrolls the Shrek key

    # Sprint S4: a spare "USB" disk for the udisks2 mount proof — a whole-disk ext4 labelled SHREKUSB with a
    # marker file seeded in. The probe finds it by label, mounts it via udisks (attach→mount→open), reads
    # the marker. snapshot=on so the guest cannot dirty the source across re-runs.
    usbdir=$(mktemp -d); printf "shrek s4 udisks mount proof\n" > "$usbdir/shrek-usb-marker"
    truncate -s 64M "$tmp/usb.raw"
    mkfs.ext4 -q -L SHREKUSB -d "$usbdir" "$tmp/usb.raw"

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
      -drive file="$tmp/usb.raw",format=raw,if=virtio,snapshot=on \
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
# bluetooth.service is Type=dbus (BusName=org.bluez), D-Bus-activated ON DEMAND. In the adapterless headless
# oracle nothing requests org.bluez within the probe window, so it validly rests INACTIVE (bluetoothd runs
# fine once triggered — the probe modprobes bluetooth + polls). Before the slice-6 ACL fix a ~50s boot stall
# (mycelium crash-loop) happened to let a desktop component activate it; the clean fast boot removed that
# incidental trigger. active OR inactive pass; failed/unknown do not — same posture as upower below.
svc bluetooth        '^(active|inactive)'
svc dbus-broker      '^active'
# HW-enablement batch (#2909) — new base services. timesyncd must be active (NTP on a 2012-RTC sealed OS).
# mbpfan is applesmc-condition-guarded so in the VM it is INACTIVE, NOT failed — this asserts the
# universal-image guard works (it would be active on a real Mac). upower is D-Bus-activated: present and
# not failed (active or inactive both pass; failed/unknown do not).
svc systemd-timesyncd '^active'
svc mbpfan           '^inactive'
svc upower           '^(active|inactive)'
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

# --- Sprint S4: Storage — an attached disk mounts through udisks and its contents are readable ----------
# udisks2 is a base daemon; S4 adds the file-manager capability: the active seat0 user is authorized to
# mount a "system" disk (filesystem-mount-system, granted by 49-shrek-udisks.rules since the sealed session
# has no graphical polkit agent), and the attached SHREKUSB virtio disk actually mounts + reads back.
grep -qa 'SHREK-DOGFOOD S4 udisks-busname=\[owned\]' "$LOG" \
  && ok "udisksd owns org.freedesktop.UDisks2 on the system bus" \
  || bad "udisks bus name unowned ($(grep -a 'SHREK-DOGFOOD S4 udisks-busname=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD S4 authz mount-system=yes' "$LOG" \
  && ok "polkit grants udisks filesystem-mount-system to the active seat0 session" \
  || bad "polkit did NOT grant udisks filesystem-mount-system ($(grep -a 'SHREK-DOGFOOD S4 authz mount-system=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD S4 mount-open=ok' "$LOG" \
  && ok "attached disk mounts via udisks and its contents are readable (attach->mount->open)" \
  || bad "attached disk did not mount/open ($(grep -a 'SHREK-DOGFOOD S4 mount-open=' "$LOG" | tail -1 | tr -d '\r'))"

# --- Sprint S3: Lock + idle — the DMS lock screen can authenticate dev via PAM ------------------------
# The lock GUI is not driven headlessly; instead prove the load-bearing auth path the sprint flagged as
# the knot. quickshell must link PAM; the /etc/pam.d/login service DMS uses (lockPamExternallyManaged)
# must exist; and PAM must accept dev's real password yet reject a wrong one (dev's hash is UNLOCKED as
# of S3). Idle->lock + lock-on-suspend are DMS-internal, enabled by the seeded lock settings.
grep -qa 'SHREK-DOGFOOD S3 quickshell-pam=\[linked\]' "$LOG" \
  && ok "quickshell is built with PAM (the lock screen can authenticate)" \
  || bad "quickshell has no PAM ($(grep -a 'SHREK-DOGFOOD S3 quickshell-pam=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD S3 pam-login-service=\[present\]' "$LOG" \
  && ok "/etc/pam.d/login present (the lock PAM service)" \
  || bad "/etc/pam.d/login missing ($(grep -a 'SHREK-DOGFOOD S3 pam-login-service=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD S3 pam-auth-correct=ok' "$LOG" \
  && ok "PAM authenticates dev with the correct password (the lock screen unlocks)" \
  || bad "PAM did NOT authenticate dev ($(grep -a 'SHREK-DOGFOOD S3 pam-auth-correct=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD S3 pam-auth-wrong-rejected=ok' "$LOG" \
  && ok "PAM rejects a wrong password (the lock is a real gate)" \
  || bad "PAM accepted a wrong password ($(grep -a 'SHREK-DOGFOOD S3 pam-auth-wrong-rejected=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD S3 lock-settings=\[externally-managed' "$LOG" \
  && ok "DMS lock configured: external PAM + idle-lock timeout + lock-on-suspend ($(grep -a 'SHREK-DOGFOOD S3 lock-settings=' "$LOG" | tail -1 | sed 's/.*lock-settings=//'))" \
  || bad "DMS lock settings not seeded ($(grep -a 'SHREK-DOGFOOD S3 lock-settings=' "$LOG" | tail -1 | tr -d '\r'))"

# --- Owner provisioning (#2939): the first-boot wizard replaced the public `shrek` credential ----------
# DOGFOOD always provisions (non-interactive baked seed), so the OWNER lines must be present. Because the
# probe runs these at boot>=2, a green OWNER stage also proves the credential SURVIVED the reboot. The
# acceptance is the two auth legs: the old public `shrek` is rejected and the new owner passphrase unlocks.
if grep -qa 'SHREK-DOGFOOD OWNER provisioned=\[yes\]' "$LOG"; then
  ok "owner provisioning ran on first boot (#2939)"
  ownerleg() { # $1 = OWNER key regex, $2 = human label
    if grep -qa "SHREK-DOGFOOD OWNER $1" "$LOG"; then ok "$2"
    else bad "$2 ($(grep -a 'SHREK-DOGFOOD OWNER ' "$LOG" | grep -a "$(echo "$1" | cut -d= -f1)" | tail -1 | tr -d '\r'))"; fi
  }
  ownerleg 'store-present=ok'               "owner shadow store seeded on the persistent /home"
  ownerleg 'store-dir=\[ok 700 root:root\]' "store dir is root:root 0700 (NEVER uid-1000 — the anti-escalation invariant, must-fix 1)"
  ownerleg 'store-file=\[ok 640 root:shadow\]' "store shadow file is root:shadow 0640"
  ownerleg 'shadow-bind=ok'                 "the store is bind-mounted over /etc/shadow (must-fix 2)"
  ownerleg 'dev-hash=\[ok'                  "the live dev: credential is a \$6\$ (SHA-512) crypt"
  ownerleg 'old-shrek-rejected=ok'          "the old public \`shrek\` password NO LONGER unlocks (the lock now means something)"
  ownerleg 'new-pass-unlocks=ok'            "the new owner passphrase unlocks the DMS lock (survived the reboot)"
  ownerleg 'dev-uid=\[ok 1000'              "dev is still uid 1000 — /etc/passwd sealed, authority plane intact (zero regression, must-fix 7)"
  ownerleg 'display-name=\[ok'              "the owner display name is recorded outside /etc/passwd"
  for u in root shrek swamp polkitd; do
    ownerleg "preserved-$u=ok" "the $u shadow line was preserved byte-for-byte across the splice (must-fix 5)"
  done
else
  bad "owner provisioning did NOT run (#2939) — no 'OWNER provisioned=[yes]' ($(grep -a 'SHREK-DOGFOOD OWNER provisioned=' "$LOG" | tail -1 | tr -d '\r'))"
fi

# --- ADR-005 provisioning plane (installer M1 §11): the AUTHORITATIVE runtime proof of provisioning items
# #1-4. The DOGFOOD image bakes a test-manifest (locale=C.UTF-8, keymap=de, owner_display_name="Swamp Lord")
# whose values DIFFER from the §5a baked defaults (en_US.UTF-8 / us / UTC), so a green PROVISION stage proves
# the manifest actually DELIVERED (not a default coincidence) and — running at boot>=2 — that the store
# SURVIVED the reboot. DOGFOOD always provisions, so the stage must be present (a missing stage is a real
# regression, scored like OWNER above).
if grep -qa 'SHREK-DOGFOOD PROVISION active=\[yes\]' "$LOG"; then
  ok "provisioning plane active (ADR-005 gate + appliers ran)"
  provleg() { # $1 = PROVISION key regex, $2 = human label
    if grep -qa "SHREK-DOGFOOD PROVISION $1" "$LOG"; then ok "$2"
    else bad "$2 ($(grep -a 'SHREK-DOGFOOD PROVISION ' "$LOG" | grep -a "$(echo "$1" | cut -d= -f1)" | tail -1 | tr -d '\r'))"; fi
  }
  provleg 'manifest-persist=\[ok'    "the provisioning manifest survived the reboot on the persistent /home (item #5)"
  provleg 'gate-sentinel=\[ok\]'     "the target-side re-validation gate ran to completion (.gate-complete)"
  # (#1) locale
  provleg 'locale-state=\[ok'        "locale seeded into the store (state/locale.conf materialized)"
  provleg 'locale-stamp=\[ok\]'      "locale seed-once stamp written (.applied/locale)"
  provleg 'locale-delivered=\[ok'    "manifest locale C.UTF-8 bind-delivered over the baked en_US /etc/locale.conf (#1)"
  # (#2) keymap + §5b VT closure
  provleg 'keymap-state=\[ok'        "keymap seeded into the store (state/vconsole.conf, XKBLAYOUT=de)"
  provleg 'keymap-stamp=\[ok\]'      "keymap seed-once stamp written (.applied/keymap)"
  provleg 'keymap-delivered=\[ok'    "manifest keymap de bind-delivered over the baked us /etc/vconsole.conf (#2)"
  provleg 'vt-keymap-live=\[ok'      "the LIVE kernel VT keymap is de (non-us test char kc21->z) — what the first-run wizard reads (#2)"
  provleg 'console-font=\[ok'        "the console FONT is left to console-setup (no FONT clobber) — font survives (#2)"
  provleg 'reassert-restores=\[ok'   "after a console-setup.service re-trigger the credential-boundary re-assert restores de (§5b closure)"
  provleg 'compositor-xkb=\[ok'      "the compositor sees XKB_DEFAULT_LAYOUT=de off the live sway process (#3)"
  # (#4) timezone — NOT a provisioned domain in M1 (no seed unit, no tz key); baked UTC stands (date +%Z)
  provleg 'tz-baked-default=\[ok'    "timezone (not wired in M1) leaves the baked UTC /etc/localtime authoritative, unbound (date +%Z) (#4)"
  # negative legs — the structural "never emergency mode" (§6), proven at runtime via the test seam
  provleg 'corrupt-defaults=\[ok'    "a CORRUPT manifest is whole-rejected: gate rc=0 + sentinel + no per-key files (defaults, not emergency)"
  provleg 'corrupt-applier-default=\[ok' "the applier over a rejected manifest takes the terminal default (rc=0, no emergency)"
  provleg 'gate-crash-retry=\[ok'    "a MISSING gate sentinel makes the applier neither seed nor stamp (retry next boot), rc=0"
  provleg 'no-emergency=\[ok'        "no provisioning unit entered failed/emergency (the non-secret plane never cascades)"
  # (#4b/#4c) owner display-name pre-fill + PS1 injection cleanup
  provleg 'owner-prefill=\[ok'       "the owner display name was pre-filled from the manifest's validated owner_display_name (§5)"
  provleg 'ps1-file-gone=\[ok'       "the PS1 display-name injection file was removed (#4c injection-vector cleanup)"
  provleg 'ps1-no-name=\[ok'         "the owner name never appears in the dev PS1 — Quickshell-only"
  echo "  INFO $(grep -a 'SHREK-DOGFOOD PROVISION locale-session=' "$LOG" | tail -1 | sed 's/.*PROVISION //' | tr -d '\r') (profile.d default also lands on C.UTF-8; locale-delivered is the discriminating proof)"
  echo "  INFO $(grep -a 'SHREK-DOGFOOD PROVISION tz-date=' "$LOG" | tail -1 | sed 's/.*PROVISION //' | tr -d '\r')"
  echo "  INFO clobber: $(grep -a 'SHREK-DOGFOOD PROVISION clobber-confirmed=' "$LOG" | tail -1 | sed 's/.*PROVISION //' | tr -d '\r')"
else
  bad "provisioning plane did NOT run (ADR-005 §11) — no 'PROVISION active=[yes]' ($(grep -a 'SHREK-DOGFOOD PROVISION active=' "$LOG" | tail -1 | tr -d '\r'))"
fi

# --- Sprint S6: Brightness + power-profiles — sealed-image PACKAGING + AUTHORIZATION (the hardware-
# independent half; the live brightness-set / profile-switch is a no-op in the VM and is INFO-only). --------
grep -qa 'SHREK-DOGFOOD S6 brightnessctl-present=\[yes\]' "$LOG" \
  && ok "brightnessctl shipped (the DMS brightness keys have a backend on real HW)" \
  || bad "brightnessctl missing ($(grep -a 'SHREK-DOGFOOD S6 brightnessctl-present=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD S6 dev-in-video-grp=\[yes\]' "$LOG" \
  && ok "dev is in the video group (brightnessctl can write the backlight node)" \
  || bad "dev not in the video group ($(grep -a 'SHREK-DOGFOOD S6 dev-in-video-grp=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD S6 ppd-present=\[yes\]' "$LOG" \
  && ok "power-profiles-daemon shipped (the DMS power-profile widget has a backend)" \
  || bad "power-profiles-daemon missing ($(grep -a 'SHREK-DOGFOOD S6 ppd-present=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD S6 authz switch-profile=yes' "$LOG" \
  && ok "polkit grants the active seat0 session power-profile switching" \
  || bad "polkit did NOT grant PowerProfiles.switch-profile ($(grep -a 'SHREK-DOGFOOD S6 authz switch-profile=' "$LOG" | tail -1 | tr -d '\r'))"
echo "  INFO $(grep -a 'SHREK-DOGFOOD S6 ppd-profiles=' "$LOG" | tail -1 | sed 's/.*SHREK-DOGFOOD //' | tr -d '\r') — live profiles are hardware-gated; empty in the VM is expected"

# --- Sprint S7: Weather tab disabled (owner chose no shell egress; a named-egress broker is a future slice).
grep -qa 'SHREK-DOGFOOD S7 weather-tab=disabled-in-seed' "$LOG" \
  && ok "weather tab seeded OFF — no undeclared shell egress (S7: disable, not broker)" \
  || bad "weather tab not disabled in the DMS seed ($(grep -a 'SHREK-DOGFOOD S7 weather-tab=' "$LOG" | tail -1 | tr -d '\r'))"

# --- Sprint S5: Dynamic theming — the vendored matugen binary is present, runs under the image's glibc,
# and turns the shipped wallpaper into a Material palette (the wallpaper->palette loop DMS drives via
# `dms matugen queue` when the theme is `dynamic`, the seeded default). matugen has no Debian package, so
# it is staged into /usr/bin from the pinned upstream release (third_party/matugen). The visual recolor is
# proven by screenshot, not by the serial oracle.
grep -qa 'SHREK-DOGFOOD S5 matugen-binary=\[present\]' "$LOG" \
  && ok "matugen binary staged in /usr/bin (the dynamic-theming engine is present)" \
  || bad "matugen binary missing ($(grep -a 'SHREK-DOGFOOD S5 matugen-binary=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD S5 matugen-runs=\[ok' "$LOG" \
  && ok "matugen runs under the image glibc ($(grep -a 'SHREK-DOGFOOD S5 matugen-runs=' "$LOG" | tail -1 | sed 's/.*matugen-runs=//'))" \
  || bad "matugen does not run ($(grep -a 'SHREK-DOGFOOD S5 matugen-runs=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD S5 matugen-palette=ok' "$LOG" \
  && ok "matugen extracts a Material palette from the wallpaper ($(grep -a 'SHREK-DOGFOOD S5 matugen-palette=ok' "$LOG" | tail -1 | sed 's/.*matugen-palette=ok //'))" \
  || bad "matugen did not extract a palette ($(grep -a 'SHREK-DOGFOOD S5 matugen-palette=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD S5 theme-settings=\[dynamic' "$LOG" \
  && ok "DMS seeded to the dynamic wallpaper-derived theme ($(grep -a 'SHREK-DOGFOOD S5 theme-settings=' "$LOG" | tail -1 | sed 's/.*theme-settings=//'))" \
  || bad "DMS not seeded to the dynamic theme ($(grep -a 'SHREK-DOGFOOD S5 theme-settings=' "$LOG" | tail -1 | tr -d '\r'))"

# --- Sprint S5 (foot sub-task): the terminal follows the dynamic palette. matugen writes a plain
# [colors] file (Material roles -> foot's 16 slots) that foot.ini includes; DMS's own [colors-dark]
# foot template is disabled (trixie foot 1.21 rejects that section); shrek-foot-osc retints open windows.
# The load-bearing check is render+check: matugen renders the template from the real wallpaper into a
# file `foot --check-config` accepts.
grep -qa 'SHREK-DOGFOOD S5foot include-wired=yes' "$LOG" \
  && ok "foot.ini includes the matugen-written palette" \
  || bad "foot.ini does not include the palette ($(grep -a 'SHREK-DOGFOOD S5foot include-wired=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD S5foot osc-helper=present' "$LOG" \
  && ok "shrek-foot-osc live-retint helper staged" \
  || bad "shrek-foot-osc missing ($(grep -a 'SHREK-DOGFOOD S5foot osc-helper=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD S5foot wiring-files=present' "$LOG" \
  && ok "foot matugen template + user config + seed present" \
  || bad "foot theming wiring files missing ($(grep -a 'SHREK-DOGFOOD S5foot wiring-files=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD S5foot dms-foot-template=disabled' "$LOG" \
  && ok "DMS's incompatible built-in foot template is disabled" \
  || bad "DMS foot template still on ($(grep -a 'SHREK-DOGFOOD S5foot dms-foot-template=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD S5foot render+check=ok' "$LOG" \
  && ok "matugen renders a foot palette from the wallpaper that foot accepts ($(grep -a 'SHREK-DOGFOOD S5foot render+check=ok' "$LOG" | tail -1 | sed 's/.*render+check=ok //'))" \
  || bad "matugen->foot render/check failed ($(grep -a 'SHREK-DOGFOOD S5foot render+check=' "$LOG" | tail -1 | tr -d '\r'))"
# The render+check above runs matugen DIRECTLY (parses TOML, ignores comments) so it MISSED the real
# regression: DMS's `dms matugen queue` slices config.toml by a naive substring search — a bracketed
# [config]/[templates] token in a COMMENT makes it capture comment text -> matugen "invalid table
# header" -> shell AND foot theming die silently. These two gate that real path.
grep -qa 'SHREK-DOGFOOD S5foot config-merge-safe=ok' "$LOG" \
  && ok "matugen config.toml has no bracketed section token in a comment (safe for the DMS substring merge)" \
  || bad "config.toml comment holds a [config]/[templates] token that breaks the DMS merge ($(grep -a 'SHREK-DOGFOOD S5foot config-merge-safe=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD S5foot dms-worker=ok' "$LOG" \
  && ok "the REAL dms matugen worker themes shell+foot end-to-end ($(grep -a 'SHREK-DOGFOOD S5foot dms-worker=ok' "$LOG" | tail -1 | sed 's/.*dms-worker=ok //'))" \
  || bad "dms matugen worker failed to theme shell/foot ($(grep -a 'SHREK-DOGFOOD S5foot dms-worker=' "$LOG" | tail -1 | tr -d '\r'))"

# --- Sprint S5 (wallpaper set sub-task): the switcher has a browsable first-party gallery. Staged as JPG
# (no Qt webp plugin in the image) and pointed at via wallpaperCyclingFolderPath in the default session.
grep -qa 'SHREK-DOGFOOD S5wall gallery=ok' "$LOG" \
  && ok "wallpaper gallery staged, all valid JPEG ($(grep -a 'SHREK-DOGFOOD S5wall gallery=ok' "$LOG" | tail -1 | sed 's/.*gallery=ok //'))" \
  || bad "wallpaper gallery missing/invalid ($(grep -a 'SHREK-DOGFOOD S5wall gallery=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD S5wall picker-folder=seeded' "$LOG" \
  && ok "DMS wallpaper switcher pointed at the gallery" \
  || bad "wallpaper switcher folder not seeded ($(grep -a 'SHREK-DOGFOOD S5wall picker-folder=' "$LOG" | tail -1 | tr -d '\r'))"

# --- Agent-launch L4/L5 (docs/agent-launch.md): the shrek-agent dispatcher structural asserts ---------
grep -qa 'SHREK-DOGFOOD Sagent binaries=present' "$LOG" \
  && ok "agent-launch dispatcher + set-default present and executable" \
  || bad "agent-launch binaries missing ($(grep -a 'SHREK-DOGFOOD Sagent binaries=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD Sagent map=ok' "$LOG" \
  && ok "L4->L0 provider map emits the sealed egress names ($(grep -a 'SHREK-DOGFOOD Sagent map=ok' "$LOG" | tail -1 | sed 's/.*map=ok //'))" \
  || bad "agent provider map wrong ($(grep -a 'SHREK-DOGFOOD Sagent map=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD Sagent fail-closed=ok' "$LOG" \
  && ok "agent dispatcher fail-closes on an unknown provider id" \
  || bad "agent dispatcher did not fail-closed on unknown id ($(grep -a 'SHREK-DOGFOOD Sagent fail-closed=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD Sagent set-default=ok' "$LOG" \
  && ok "L5 default round-trips (set-default -> shrek-agent resolves the right profile)" \
  || bad "agent set-default did not round-trip ($(grep -a 'SHREK-DOGFOOD Sagent set-default=' "$LOG" | tail -1 | tr -d '\r'))"

# --- Agent-launch name-resolution layer (docs/agent-launch.md §7): shrek-connect hook-up + hosts seed ---
grep -qa 'SHREK-DOGFOOD Sconnect binary=present' "$LOG" \
  && ok "hook-up tool (shrek-connect) present and executable" \
  || bad "shrek-connect missing ($(grep -a 'SHREK-DOGFOOD Sconnect binary=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD Sconnect map=ok' "$LOG" \
  && ok "provider -> sealed host-name map matches the L0 policy ($(grep -a 'SHREK-DOGFOOD Sconnect map=ok' "$LOG" | tail -1 | sed 's/.*map=ok //'))" \
  || bad "shrek-connect host map wrong ($(grep -a 'SHREK-DOGFOOD Sconnect map=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD Sconnect bind=ok' "$LOG" \
  && ok "hook-up bind writes the correct hosts line (bind/list round-trip)" \
  || bad "shrek-connect bind round-trip failed ($(grep -a 'SHREK-DOGFOOD Sconnect bind=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD Sconnect unbound-guard=ok' "$LOG" \
  && ok "shrek-agent fail-closes with a hook-up hint when the provider is unbound" \
  || bad "unbound-provider guard did not fire ($(grep -a 'SHREK-DOGFOOD Sconnect unbound-guard=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD Sconnect etc-hosts-wired=ok' "$LOG" \
  && ok "/etc/hosts is the baked symlink to the root-owned /run/shrek/hosts projection (ADR-008)" \
  || bad "/etc/hosts symlink not wired ($(grep -a 'SHREK-DOGFOOD Sconnect etc-hosts-wired=' "$LOG" | tail -1 | tr -d '\r'))"
grep -qa 'SHREK-DOGFOOD Sconnect compose-service=active' "$LOG" \
  && ok "the boot-time hosts-compose oneshot ran (localhost resolvable on a fresh box)" \
  || bad "shrek-hosts-compose.service not active ($(grep -a 'SHREK-DOGFOOD Sconnect compose-service=' "$LOG" | tail -1 | tr -d '\r'))"

# --- ADR-003 Part 1: baseline-application MERGE proof (scored only when the Part-1 Onions are merged).
# The browser (shrek-browser) and app set (shrek-apps) are independent layers, so score each on its own
# gate. A store without a given layer emits an ABSENT line and is skipped cleanly (no new FAIL). Whether
# firefox/GTK actually RENDER is a separate proof (scripts/apps-render-proof.sh — screendump lies, #2923).
if grep -qa 'SHREK-DOGFOOD APPS browser=ok' "$LOG"; then
  ok "browser — firefox-esr merged onto /usr with its .desktop entry (shrek-browser Onion)"
elif grep -qa 'SHREK-DOGFOOD APPS browser=FAIL' "$LOG"; then
  bad "browser — firefox-esr merge incomplete [$(grep -a 'SHREK-DOGFOOD APPS browser=FAIL' "$LOG" | tail -1 | sed 's/.*APPS //')]"
elif grep -qa 'SHREK-DOGFOOD APPS browser=ABSENT' "$LOG"; then
  echo "  (browser proof skipped — shrek-browser sysext not in this store)"
fi
if grep -qa 'SHREK-DOGFOOD APPS filemanager=ok\|SHREK-DOGFOOD APPS filemanager=FAIL' "$LOG"; then
  for pair in \
    "filemanager=ok|file manager (nautilus) merged with its .desktop entry" \
    "viewers=ok|image/PDF/media viewers (loupe/papers/mpv) present" \
    "editor=ok|GUI text editor (gnome-text-editor) present" \
    "archive=ok|archive manager (file-roller) + unzip/zip/7z/xz backends present" \
    "fonts=ok|the real font set landed — emoji + CJK (the thin-font fix)" \
    "cli=ok|everyday CLI utils promoted onto the installed disk (curl/git/jq/rg/fd/…)"; do
    tok=${pair%%|*}; desc=${pair#*|}
    if grep -qa "SHREK-DOGFOOD APPS ${tok}" "$LOG"; then ok "apps — ${desc}"
    else bad "apps — ${desc} [$(grep -a "SHREK-DOGFOOD APPS ${tok%%=*}=" "$LOG" | tail -1 | sed 's/.*APPS //')]"; fi
  done
elif grep -qa 'SHREK-DOGFOOD APPS filemanager=ABSENT' "$LOG"; then
  echo "  (apps proof skipped — shrek-apps sysext not in this store)"
fi

# --- Bench-0 (ADR-003 Part 2): rootless-container Bench runtime proof (scored only when shrek-bench merged).
# Fable's smallest-next-proof, phase B (sealed-boot userns/AppArmor posture). Skipped cleanly on stores
# without the shrek-bench sysext so pre-Bench dogfood runs do not newly FAIL.
if grep -qa 'SHREK-DOGFOOD BENCH ' "$LOG" && ! grep -qa 'BENCH podman=MISSING' "$LOG"; then
  echo "  bench pool: $(grep -a 'SHREK-DOGFOOD BENCH pool=' "$LOG" | tail -1 | sed 's/.*BENCH pool=//')"
  for pair in \
    "pool-service=ok|Bench pool stood up at boot by shrek-bench-pool.service (noexec sub-mount on shrek-data /home, step 3)" \
    "prjquota=ok|per-Bench PROJECT quota enforced on the growable /home — 4MiB write past a 1MiB cap -> EDQUOT (rule 1)" \
    "userns=ok|sealed unprivileged userns available (unshare -U on the real image)" \
    "overlay-native=ok|NATIVE rootless overlay driver (not vfs/fuse)" \
    "graphroot-on-pool=ok|graphroot on the persistent /home bench pool (baked storage.conf)" \
    "uidmap=ok|subuid maps the full 65536 range (setuid newuidmap survived the merge)" \
    "run-exit42=ok|container execs a real ELF off the noexec pool -> rc 42 (rule-2 proof)" \
    "seed-ffmpeg=ok|the offline seed is a real media base — ffmpeg present + runs (step-8 north-star substrate)" \
    "noexec-negctl=ok|direct exec from the noexec pool is BLOCKED (pool really is noexec)"; do
    tok=${pair%%|*}; desc=${pair#*|}
    if grep -qa "SHREK-DOGFOOD BENCH ${tok}" "$LOG"; then ok "bench — ${desc}"
    else bad "bench — ${desc} [$(grep -a "SHREK-DOGFOOD BENCH ${tok%%=*}=" "$LOG" | tail -1 | sed 's/.*BENCH //')]"; fi
  done
  echo "  bench cgroup/info: $(grep -a 'SHREK-DOGFOOD BENCH cgroup' "$LOG" | tail -1 | sed 's/.*BENCH //')"
  # Bench record-forgery anchor (mycelium #2982 hole 2): the durable records live under the root-owned
  # /home/.shrek, NOT the dev-owned 0700 pool, so dev cannot swap the records dir to inject forged records.
  if grep -qa 'SHREK-DOGFOOD BENCH-SEC ' "$LOG"; then
    for pair in \
      "anchor-root-owned=ok|the /home/.shrek records anchor is root:root 0755 (dev owns neither it nor its entries)" \
      "forge-blocked=ok|dev cannot plant or rename the records dir aside — record forgery is structurally blocked"; do
      tok=${pair%%|*}; desc=${pair#*|}
      if grep -qa "SHREK-DOGFOOD BENCH-SEC ${tok}" "$LOG"; then ok "bench-sec — ${desc}"
      else bad "bench-sec — ${desc} [$(grep -a "SHREK-DOGFOOD BENCH-SEC ${tok%%=*}=" "$LOG" | tail -1 | sed 's/.*BENCH-SEC //')]"; fi
    done
  fi
  # Bench supervisor (ADR-003 Part 2 step 4): score the `gatekeeperd bench` lifecycle if it ran.
  if grep -qa 'SHREK-DOGFOOD BENCH-SUP ' "$LOG" && ! grep -qa 'BENCH-SUP gatekeeperd=MISSING' "$LOG"; then
    echo "  bench-sup create: $(grep -a 'SHREK-DOGFOOD BENCH-SUP create=' "$LOG" | tail -1 | sed 's/.*BENCH-SUP //')"
    for pair in \
      "create=ok|supervisor creates a Bench with a project quota + durable record" \
      "seed-autoload=ok|ensure_seed re-loads the offline seed from the sysext archive when absent (product loader)" \
      "run-exit42=ok|gatekeeperd bench run executes the seed via rootless podman (rc 42)" \
      "state-stopped=ok|the record returns to state=stopped after the run" \
      "quota-enforce=ok|the supervisor's per-Bench cap is EDQUOT-enforced (non-root writer)" \
      "destroy=ok|destroy removes the record + the data dir + the /run bench dir"; do
      tok=${pair%%|*}; desc=${pair#*|}
      if grep -qa "SHREK-DOGFOOD BENCH-SUP ${tok}" "$LOG"; then ok "bench-sup — ${desc}"
      else bad "bench-sup — ${desc} [$(grep -a "SHREK-DOGFOOD BENCH-SUP ${tok%%=*}=" "$LOG" | tail -1 | sed 's/.*BENCH-SUP //')]"; fi
    done
  fi
  # Bench GRANTS (ADR-003 Part 2 step 5): score the FS-grant + egress-policy plane on the sealed image
  # (the live container round-trip + live egress inject are the host oracle's job — VM is endpoint-free).
  if grep -qa 'SHREK-DOGFOOD BENCH-GRANT ' "$LOG"; then
    for pair in \
      "materialized-noexec=ok|an FS grant relocates into the host ns as a noexec bind on the sealed /home" \
      "grantdir-root0710=ok|the per-Bench grants dir is root:dev 0710 — dev traverses but cannot write it (redirect-safe, #2982 hole 3)" \
      "redirect-blocked=ok|dev cannot plant a symlink leaf nor rename the grants dir to redirect the root bind onto a system target" \
      "recorded=ok|the FS grant is recorded durably (survives reboot)" \
      "reissue-rematerialize=ok|boot reissue re-pins + re-materializes the FS grant after /run is wiped" \
      "egress-recorded=ok|the network verb records a sealed egress profile" \
      "egress-deny-unknown=ok|the network verb refuses a non-sealed profile (default-deny)"; do
      tok=${pair%%|*}; desc=${pair#*|}
      if grep -qa "SHREK-DOGFOOD BENCH-GRANT ${tok}" "$LOG"; then ok "bench-grant — ${desc}"
      else bad "bench-grant — ${desc} [$(grep -a "SHREK-DOGFOOD BENCH-GRANT ${tok%%=*}=" "$LOG" | tail -1 | sed 's/.*BENCH-GRANT //')]"; fi
    done
  fi
  # Bench EXPORT (ADR-003 Part 2 step 7): the constrained .desktop launcher plane on the sealed image.
  if grep -qa 'SHREK-DOGFOOD BENCH-EXPORT ' "$LOG"; then
    for pair in \
      "wrapper-baked=ok|the fixed shrek-bench-run launcher wrapper is baked at /usr/bin (the .desktop Exec target)" \
      "desktop-as-dev=ok|export writes the .desktop AS DEV, not root (no symlink-redirect root-write gadget)" \
      "fixed-exec=ok|the .desktop Exec is the fixed wrapper + exactly two tokens (no command)" \
      "no-command=ok|the .desktop carries NO command and NO field codes (fixed-baked-key discipline)" \
      "recorded=ok|the key->workload map is in the root-owned record (untamperable by the .desktop)" \
      "run-export=ok|run-export resolves the key server-side + runs it in the Bench (rc 42)" \
      "forged-key-refused=ok|an unregistered launcher key is refused (a forged .desktop injects nothing)" \
      "destroy-sweep=ok|destroy sweeps the bench's exported .desktop files"; do
      tok=${pair%%|*}; desc=${pair#*|}
      if grep -qa "SHREK-DOGFOOD BENCH-EXPORT ${tok}" "$LOG"; then ok "bench-export — ${desc}"
      else bad "bench-export — ${desc} [$(grep -a "SHREK-DOGFOOD BENCH-EXPORT ${tok%%=*}=" "$LOG" | tail -1 | sed 's/.*BENCH-EXPORT //')]"; fi
    done
  fi
  # Bench MEDIA (ADR-003 Part 2 step 8, the north-star): a real OFFLINE video transcode inside a Bench with
  # the shipped seed's ffmpeg, host sealed, output round-tripping to a granted dest, destroy keeping it.
  if grep -qa 'SHREK-DOGFOOD BENCH-MEDIA ' "$LOG"; then
    echo "  bench-media transcode: $(grep -a 'SHREK-DOGFOOD BENCH-MEDIA transcode=' "$LOG" | tail -1 | sed 's/.*BENCH-MEDIA //')"
    for pair in \
      "input-fixture=ok|a real input video is fabricated offline with the shipped seed's ffmpeg" \
      "transcode=ok|THE north-star: a real offline ffmpeg transcode runs INSIDE the bench (rc 0)" \
      "output-owned-by-dev=ok|the transcoded output round-trips to the granted dest owned by dev" \
      "seed-autoload=ok|the media run's ensure_seed re-loaded the offline seed from the sysext archive" \
      "output-is-video=ok|the output is a real decodable VP8/webm video (ffprobe), not an empty file" \
      "ro-input-sealed=ok|the bench cannot write the read-only input grant (host stays sealed)" \
      "destroy-keeps-output=ok|destroy removes the bench (record+data+/run) but the delivered output persists"; do
      tok=${pair%%|*}; desc=${pair#*|}
      if grep -qa "SHREK-DOGFOOD BENCH-MEDIA ${tok}" "$LOG"; then ok "bench-media — ${desc}"
      else bad "bench-media — ${desc} [$(grep -a "SHREK-DOGFOOD BENCH-MEDIA ${tok%%=*}=" "$LOG" | tail -1 | sed 's/.*BENCH-MEDIA //')]"; fi
    done
  fi
  # Bench WORKSHOP (apt seed): the Debian workshop seed bakes + loads + per-bench selection, OFFLINE (the live
  # apt-over-egress fetch is the host oracle's job). Scored only if the debian.tar was baked into the sysext.
  if grep -qa 'SHREK-DOGFOOD BENCH-WORKSHOP seed-recorded' "$LOG"; then
    for pair in \
      "seed-recorded=ok|create --seed debian records the per-bench seed (sealed-catalog selection)" \
      "unknown-seed-refused=ok|an unknown seed name is refused up front (fail-closed, like an egress profile)" \
      "seed-loaded=ok|ensure_seed loads localhost/debian from the baked debian.tar on a plain run" \
      "seed-apt-ready=ok|the shipped seed's runtime apt sources are https deb.debian.org only (one CDN host)" \
      "python-baked=ok|the seed bakes python3 + pip + the venv/ensurepip wheels (offline-functional)" \
      "net-set-repeatable=ok|the repeatable network verb records the composed debian-apt + pypi-https set"; do
      tok=${pair%%|*}; desc=${pair#*|}
      if grep -qa "SHREK-DOGFOOD BENCH-WORKSHOP ${tok}" "$LOG"; then ok "bench-workshop — ${desc}"
      else bad "bench-workshop — ${desc} [$(grep -a "SHREK-DOGFOOD BENCH-WORKSHOP ${tok%%=*}=" "$LOG" | tail -1 | sed 's/.*BENCH-WORKSHOP //')]"; fi
    done
  elif grep -qa 'SHREK-DOGFOOD BENCH-WORKSHOP seed=absent' "$LOG"; then
    echo "  (bench-workshop skipped — debian.tar not baked into this store)"
  fi
elif grep -qa 'BENCH podman=MISSING' "$LOG"; then
  echo "  (bench proof skipped — shrek-bench sysext not in this store)"
fi

# --- Bench-authz step 3: console consent ceremony (BENCH-CONSENT) — scored ONLY if the marker-gated
# consent stage ran. A plain dogfood run seeds no /home/.shrek-consent-dogfood marker, so the persist probe
# never invokes dogfood-consent-probe and NO BENCH-CONSENT lines appear -> this whole block is skipped with
# zero new FAIL. The full driven proof (real SAK chord + typed answers + scanout OCR) is
# scripts/bench-consent-vm-proof.sh; this is only a courtesy score if the lines happen to be present.
if grep -qa 'SHREK-DOGFOOD BENCH-CONSENT begin' "$LOG"; then
  for pair in \
    "approve=ok|read-only grant: real SAK chord + typed y APPLIES" \
    "code-approve=ok|read-write grant: the 6-digit confirmation code APPLIES" \
    "deny-n=ok|a non-affirmative answer DENIES with no record change" \
    "swap-refused=ok|a same-name diff-inode swap after render is refused at apply" \
    "vt-restored=ok|the previously-active VT is restored after the ceremony" \
    "compositor-alive=ok|the compositor + seat0 session survived the ceremony"; do
    tok=${pair%%|*}; desc=${pair#*|}
    if grep -qa "SHREK-DOGFOOD BENCH-CONSENT ${tok}" "$LOG"; then ok "consent — ${desc}"
    elif grep -qa "SHREK-DOGFOOD BENCH-CONSENT ${tok%%=*}=SKIP" "$LOG"; then echo "  (consent ${tok%%=*} skipped — SAK delivery failed; see bench-consent-vm-proof.sh)"
    else bad "consent — ${desc} [$(grep -a "SHREK-DOGFOOD BENCH-CONSENT ${tok%%=*}=" "$LOG" | tail -1 | sed 's/.*BENCH-CONSENT //')]"; fi
  done
fi

# --- ADR-006 slice-6: optional on-device AI layer — boot-first proof. Scored ONLY if the shrek-ai Onion
# merged (a non-AI build emits "AI onion-merged=[no]" and this whole block skips with zero new FAIL). The
# full §9 legs (reboot persistence, egress counters, injection payload, shell no-subprocess) land in a
# follow-up; this is the "the whole layer RUNS for real" boot-first milestone. ------------------------------
if grep -qa 'SHREK-DOGFOOD AI onion-merged=.ok' "$LOG"; then
  for pair in \
    "model-verify|model-as-data GGUF on /home matches the sealed baked digest (READY)" \
    "model-health|lazy inference server started + healthy on loopback 127.0.0.1:8198" \
    "model-answers|the model ANSWERS on loopback with thinking OFF (Granite normal mode)" \
    "loopback-only|every AI listener (8198/8199) binds 127.0.0.1 ONLY (ADR-006 §7)" \
    "model-acl-safe|starting+restarting the model server leaves /home ACL intact (dev still traverses /home/dev)" \
    "recall-selfknowledge|on-box Shrek Memory API /recall returns real seed self-knowledge" \
    "brain-persists-reboot|the dev-owned runtime brain memory survives a reboot (seed re-ingest never clobbers it)" \
    "egress-zero|every AI listener enforces kernel egress-deny (IPAddressDeny=any active) — ADR-006 §7" \
    "injection-no-host-effect|a seeded instruction-shaped memory is inert — recalled as data, no host effect" \
    "shell-no-subprocess|the shipped merged mycolink-shell carries no host-exec/process-spawn primitive" \
    "reasoning-default|the sealed shell defaults to the ratified NORMAL mode (enable_thinking=False) — ADR-006 §9c" \
    "ai-cli-dispatch|the Rust \`shrek\` CLI routes \`ai\` to the on-device front door (shrek ai --help, rc 0)"; do
    tag=${pair%%|*}; desc=${pair#*|}
    line=$(grep -a "SHREK-DOGFOOD AI ${tag}=" "$LOG" | tail -1)
    val=$(printf '%s' "$line" | sed "s/.*AI ${tag}=//" | tr -d '\r')
    if printf '%s' "$val" | grep -q '^\[ok'; then ok "ai — ${desc} ${val}"
    else bad "ai — ${desc} ${val:-<no line — probe did not reach the AI stage>}"; fi
  done
elif grep -qa 'SHREK-DOGFOOD AI onion-merged=.no' "$LOG"; then
  echo "  (AI proof skipped — shrek-ai Onion not in this store/build)"
fi

echo "--- Dogfood tally: PASS=$pass FAIL=$fail ---"
[ "$fail" -eq 0 ] && echo "=== Dogfood-0 M1+M2+M3: GREEN ===" || { echo "=== Dogfood-0: NOT GREEN — inspect $LOG ==="; exit 1; }
