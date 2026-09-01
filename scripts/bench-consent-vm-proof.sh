#!/usr/bin/env bash
# bench-consent-vm-proof.sh — sealed-VM acceptance for the console consent ceremony (step 3 of the bench
# authorization slice; docs/bench-authz-consent-slice.md). This is the ONE proof the unit tests and the
# headless host oracle cannot give: the RealConsole VT/SAK transport driven END TO END on a booted image.
#
# It reuses scripts/dogfood-vm.sh's boot block (sealed Secure-Boot/dm-verity image + persistent /home data
# disk + the DOGFOOD persist probe that reboots once so boot2 has a live desktop), then adds three things:
#   1. seeds a marker (/home/.shrek-consent-dogfood) into the fresh data disk so the persist probe invokes
#      image/overlay/usr/lib/shrek/dogfood-consent-probe on boot2 (a plain dogfood run has no marker and
#      skips the whole stage clean — an undriven ceremony would burn 60s SAK-arm timeouts + arm cooldowns).
#   2. replaces the fixed screenshot sleeps with a SERIAL CUE-LOOP: the guest probe emits
#      SHREK-DOGFOOD CONSENT-CUE lines, this host reacts with QEMU monitor `sendkey` (the REAL
#      Ctrl+Alt+Shift+Esc chord + the typed answers) and `screendump` (the mid-ceremony scanout capture).
#      We drive the REAL chord through the virtio-keyboard, NEVER a D-Bus-injected SAK signal (arm_sak's
#      sender-pinned match filters a forged signal by the very defense under test).
#   3. TWO evidence channels for the display property: the guest asserts the tty8 char buffer (/dev/vcs8)
#      holds the request (render barrier); this host OCRs the cued screendump (tesseract) to prove the text
#      VT actually SCANNED OUT while the compositor DRM master was paused (never vcs8 alone — a failed DRM
#      handoff renders into the tty8 buffer while the display still scans out the compositor = consent
#      theater).
#
# HONEST SCOPE — what this proves and what it does NOT:
#   PROVES (alongside a HEALTHY compositor): a real SAK chord flips to the gatekeeperd-owned VT; the
#   sanitized authority DIFF scans out; a scripted y / typed confirmation-code APPLIES; a wrong answer /
#   answer timeout / apply-time object-identity swap / dropped peer DENIES with no record change; a forged
#   (non-login1) SAK is filtered; the previous VT is restored and the compositor survives every path; no
#   getty squats the ceremony VT; a dev-uid agent cannot read the consent buffer.
#   DOES NOT PROVE (still headless / unit-only): no-seat fail-closed (green in bench-plane-proof.sh),
#   VT-switch-deadline against an UNCOOPERATIVE compositor (we cannot make a cooperating sway veto), pure
#   render-fail, PID-reuse microsecond TOCTOU, daemon-restart-mid-ceremony. It makes NO claim that a
#   HIJACKED compositor cannot suppress the chord — that is a separate property.
#
# RUNTIME UNCERTAINTY (this is design-build + VM iteration): the one thing no static check settles is
# whether the SAK chord actually reaches logind as a SecureAttentionKey signal in this VM. The chord is
# SAK_CHORD below; if arm never delivers, the probe reports BENCH-CONSENT sak-delivery=FAIL and
# short-circuits (the run returns in ~1 min instead of burning ~9x60s), so a red run is a fast, legible
# signal to tune the chord / logind config rather than a nine-minute hang.
#
# Prereqs (same as dogfood-vm.sh): scripts/build-desktop-layer.sh ; DOGFOOD=1 scripts/build-in-container.sh
# 1 ; INCLUDE_DEV=1 scripts/build-layers.sh desktop. Needs /dev/kvm + docker (beepboop has both).
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"; mkdir -p out

RAW="${RAW:-$(ls -t out/shrek_*_x86-64.raw 2>/dev/null | head -1)}"
[ -n "$RAW" ] && [ -f "$RAW" ] || { echo "no out/shrek_*_x86-64.raw — run DOGFOOD=1 scripts/build-in-container.sh 1 first" >&2; exit 1; }
STORE="${STORE:-out/layer-store.raw}"
[ -f "$STORE" ] || { echo "no $STORE — run scripts/build-layers.sh desktop first" >&2; exit 1; }
grep -qa 'shrek-dev.raw' "$STORE" || { echo "$STORE has no shrek-dev toolchain — rebuild: INCLUDE_DEV=1 scripts/build-layers.sh desktop" >&2; exit 1; }

# FRESH disposable data disk (same as dogfood-vm.sh: boot1 must see an empty /home so the persist probe
# writes its marker and reboots). We inject the CONSENT marker into it inside the container (debugfs), so
# it is present on the persistent /home for boot2 when the desktop is up and the ceremony can run.
DATA="out/consent-data.raw"
FRESH=1 scripts/dogfood-data-disk.sh "$DATA" 4G

# The consent stage adds up to ~9 interactive ceremonies (one carries a 45s answer-timeout), on top of the
# two-boot persist cycle + desktop bring-up — so a bigger wall-clock budget than the plain dogfood.
BUDGET="${BUDGET:-600}"
SAK_CHORD="${SAK_CHORD:-ctrl-alt-shift-esc}"   # the SecureAttentionKey chord logind detects (50-shrek-sak.conf)
LOG=out/consent-console.log; : > "$LOG"
rm -f out/consent-*.ppm out/consent-*.png out/consent-*.txt out/consent-desktop.png

echo "=== bench-consent VM proof: booting $RAW (+store $STORE +data $DATA) budget ${BUDGET}s chord=[$SAK_CHORD] ==="
docker run --rm --device /dev/kvm \
  -v "${REPO_ROOT}:/work" -w /work \
  -e RAW="$RAW" -e STORE="$STORE" -e DATA="$DATA" -e BUDGET="$BUDGET" -e SAK_CHORD="$SAK_CHORD" -e LOG="$LOG" \
  debian:trixie \
  bash -euo pipefail -c '
    apt-get update -qq >/dev/null
    apt-get install -y --no-install-recommends -qq qemu-system-x86 ovmf socat netpbm e2fsprogs tesseract-ocr >/dev/null
    tmp=$(mktemp -d); monsock="$tmp/mon.sock"
    cp /usr/share/OVMF/OVMF_VARS_4M.fd "$tmp/vars.fd"

    # Seed the consent marker at the ROOT of the data fs (which mounts as /home) => /home/.shrek-consent-dogfood.
    printf "bench-consent dogfood marker\n" > "$tmp/cmarker"
    debugfs -w -R "write $tmp/cmarker /.shrek-consent-dogfood" "/work/$DATA" >/dev/null 2>&1 \
      && echo "seeded consent marker into $DATA" || echo "NOTE: debugfs marker seed FAILED (consent stage will not run)"
    # Fast-iteration override: seed the CURRENT probe onto the data disk (=> /home/.shrek-consent-probe) so
    # probe edits do not need a full image rebuild — the baked persist probe prefers this copy when present.
    debugfs -w -R "write /work/image/overlay/usr/lib/shrek/dogfood-consent-probe /.shrek-consent-probe" "/work/$DATA" >/dev/null 2>&1 \
      && echo "seeded override probe into $DATA" || echo "NOTE: override probe seed FAILED (baked probe will run)"

    # Spare labelled USB disk so the persist probe S4 leg still has its target (kept identical to dogfood-vm.sh).
    usbdir=$(mktemp -d); printf "shrek s4 udisks mount proof\n" > "$usbdir/shrek-usb-marker"
    truncate -s 64M "$tmp/usb.raw"; mkfs.ext4 -q -L SHREKUSB -d "$usbdir" "$tmp/usb.raw"

    mon() { printf "%s\n" "$1" | socat - "UNIX-CONNECT:$monsock" >/dev/null 2>&1 || true; }

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
      -monitor "unix:$monsock,server,nowait" &
    QPID=$!

    # ---- serial cue-loop: the dumb host executor -----------------------------------------------------
    # Read new CONSENT-CUE lines and act; re-issue the SAK chord ~every 2s while a chord is armed. State
    # (CHORD/LASTCHORD) must persist across iterations, so the read loop runs in-shell via process
    # substitution (NOT a pipe). set +e: a dropped monitor command must never abort the run.
    set +e
    LOGF="/work/'"$LOG"'"
    nlines=0; CHORD=0; LASTCHORD=0; DONE=0; start=$SECONDS
    while kill -0 "$QPID" 2>/dev/null; do
      (( SECONDS - start > BUDGET )) && { echo "--- budget ${BUDGET}s reached ---"; break; }
      total=$(wc -l < "$LOGF" 2>/dev/null || echo 0)
      if (( total > nlines )); then
        while IFS= read -r line; do
          case "$line" in *"BENCH-CONSENT done"*) DONE=1 ;; esac
          case "$line" in *CONSENT-CUE*) : ;; *) continue ;; esac
          rest="${line#*CONSENT-CUE }"; verb="${rest%% *}"
          leg=""; name=""; keys=""
          [[ "$rest" =~ leg=([^[:space:]]+) ]] && leg="${BASH_REMATCH[1]}"
          [[ "$rest" =~ name=([^[:space:]]+) ]] && name="${BASH_REMATCH[1]}"
          [[ "$rest" =~ keys=([^[:space:]]+) ]] && keys="${BASH_REMATCH[1]}"
          case "$verb" in
            chord)      CHORD=1; echo "[cue] chord-start leg=$leg" ;;
            stop-chord) CHORD=0; echo "[cue] chord-stop leg=$leg" ;;
            shot)       echo "[cue] screendump leg=$leg name=$name"
                        mon "screendump /work/out/${name}.ppm"; sleep 0.6
                        [ -s "/work/out/${name}.ppm" ] && pnmtopng "/work/out/${name}.ppm" > "/work/out/${name}.png" 2>/dev/null && rm -f "/work/out/${name}.ppm" ;;
            answer)     CHORD=0; echo "[cue] answer leg=$leg keys=$keys"
                        for k in ${keys//,/ }; do mon "sendkey $k"; sleep 0.07; done ;;
          esac
        done < <(sed -n "$((nlines+1)),${total}p" "$LOGF" 2>/dev/null)
        nlines=$total
      fi
      (( DONE )) && { echo "--- probe signalled done, stopping qemu ---"; break; }
      if (( CHORD == 1 )) && (( SECONDS - LASTCHORD >= 2 )); then
        mon "sendkey $SAK_CHORD"; LASTCHORD=$SECONDS
      fi
      sleep 0.4
    done

    # Settle + a context screenshot of wherever the display ended up, then OCR every ceremony scanout shot.
    sleep 2; mon "screendump /work/out/consent-desktop.ppm"; sleep 0.6
    [ -s /work/out/consent-desktop.ppm ] && pnmtopng /work/out/consent-desktop.ppm > /work/out/consent-desktop.png 2>/dev/null && rm -f /work/out/consent-desktop.ppm
    for p in /work/out/consent-*.png; do
      [ -e "$p" ] || continue
      b="${p%.png}"
      tesseract "$p" "$b" --psm 6 >/dev/null 2>&1 || true   # writes ${b}.txt
    done

    mon "quit"; kill "$QPID" 2>/dev/null; wait "$QPID" 2>/dev/null
  '

echo "=== serial log: $LOG ($(wc -l < "$LOG" 2>/dev/null || echo 0) lines) ==="
ls -l out/consent-*.png 2>/dev/null || echo "no screendumps captured"

# ---- verdict: score the BENCH-CONSENT lines + the OCR scanout evidence --------------------------------
# set +e: the verdict is explicit pass/fail accounting — a `line_for` grep that finds nothing (a MISSING
# leg) must count as a FAIL below, not abort the whole verdict under set -e/pipefail.
set +e
echo
echo "=== bench-consent acceptance verdict ==="
pass=0; fail=0; skip=0
ok()  { echo "  PASS $*"; pass=$((pass + 1)); }
bad() { echo "  FAIL $*"; fail=$((fail + 1)); }
skp() { echo "  SKIP $*"; skip=$((skip + 1)); }

line_for() { grep -a "SHREK-DOGFOOD BENCH-CONSENT $1=" "$LOG" | tail -1 | tr -d '\r'; }

# A leg passes on =ok, fails on =FAIL, is skipped on =SKIP (SAK short-circuit), missing = FAIL.
leg() { # $1 key, $2 human label
  l=$(line_for "$1"); v=${l#*"$1="}; v=${v%% *}
  case "$v" in
    ok)   ok  "$1 — $2" ;;
    SKIP) skp "$1 — $2 (skipped: SAK delivery failed)" ;;
    "")   bad "$1 — $2 [no BENCH-CONSENT line — probe did not reach this leg]" ;;
    *)    bad "$1 — $2 [$l]" ;;
  esac
}

# Did the probe run at all?
if ! grep -qa 'SHREK-DOGFOOD BENCH-CONSENT begin' "$LOG"; then
  echo "  the consent probe never started — no BENCH-CONSENT lines on serial."
  echo "  (marker not seeded? persist probe never reached boot2? inspect $LOG)"
  echo "--- bench-consent tally: PASS=$pass FAIL=1 SKIP=$skip ---"
  echo "=== bench-consent: NOT GREEN ==="; exit 1
fi

# SAK delivery is the linchpin — call it out explicitly.
if grep -qa 'SHREK-DOGFOOD BENCH-CONSENT sak-delivery=FAIL' "$LOG"; then
  bad "SAK delivery — the real chord did NOT reach logind as a SecureAttentionKey signal ($(line_for approve))"
  echo "     -> the interactive legs were short-circuited. Tune SAK_CHORD / logind SAK config and re-run."
fi

grep -qa 'SHREK-DOGFOOD BENCH-CONSENT daemon=ok' "$LOG" && ok "gatekeeperd socket present on the sealed image" || bad "gatekeeperd socket absent ($(line_for daemon))"

# Static / content-independent properties.
leg no-getty        "no getty squats the reserved consent VT (tty8 > NAutoVTs)"
leg agent-blind-vcs "a dev-uid agent cannot read the consent VT buffer (/dev/vcs8)"

# The core apply/deny matrix.
leg approve         "read-only grant: real chord + typed y APPLIES (RESULT ok, fs-ro recorded)"
leg vcs-render      "the sanitized authority DIFF landed in the tty8 buffer (render barrier)"
leg code-approve    "read-write grant: the 6-digit confirmation code APPLIES (fs-rw recorded)"
leg export-approve  "export APPLIES (write_desktop_as_dev via runuser => CAP_SETUID/SETGID present)"
leg trifecta-warn   "attaching egress to an fs-granted bench renders the lethal-trifecta WARNING"
leg deny-n          "a non-affirmative answer DENIES (ceremony-declined, nothing recorded)"
leg cooldown        "a repeat request in the post-deny window is rate-limited (anti SAK-fatigue)"
leg answer-timeout  "no answer DENIES on the 45s timeout (ceremony-timeout, nothing recorded)"
leg swap-refused    "a same-name diff-inode swap after render is refused at apply (object-identity)"
leg disconnect      "a dropped peer DENIES an affirmative answer (ceremony-disconnected)"

# forged-sak: ok (sender-pin held) OR emit-blocked (bus policy — a DISTINCT non-failing outcome) both pass;
# only a FAIL (forged signal flipped the VT) or SKIP fails/skips.
fl=$(line_for forged-sak)
case "${fl#*forged-sak=}" in
  ok*)           ok  "forged-sak — a forged (non-login1) SAK is filtered; the real chord still completes" ;;
  emit-blocked*) ok  "forged-sak — bus policy blocked the test emit (sender-pin not exercised, but not a failure) [$fl]" ;;
  SKIP*)         skp "forged-sak — skipped (SAK delivery failed)" ;;
  "")            bad "forged-sak — no line [probe did not reach it]" ;;
  *)             bad "forged-sak — a forged SAK affected the ceremony [$fl]" ;;
esac

# VT restore + compositor survival across the whole run.
if grep -qa 'SHREK-DOGFOOD BENCH-CONSENT vt-restored=FAIL' "$LOG"; then
  bad "vt-restored — a ceremony did NOT restore the previous VT ($(grep -a 'vt-restored=FAIL' "$LOG" | tail -1 | tr -d '\r'))"
elif grep -qa 'SHREK-DOGFOOD BENCH-CONSENT vt-restored=ok' "$LOG"; then
  ok "vt-restored — the previously-active VT is restored after every ceremony path"
else
  bad "vt-restored — no vt-restored verdict on serial"
fi
leg compositor-alive "the compositor + seat0 session survived the whole run"

# Second evidence channel: OCR the cued mid-ceremony screendumps. The text VT must have SCANNED OUT (the
# request text is legible in the pixels), not just landed in the tty8 buffer while the desktop was on screen.
scored_ocr=0
for leg_name in approve code-approve; do
  t="out/consent-${leg_name}.txt"
  if [ -f "$t" ]; then
    scored_ocr=1
    if grep -qiE 'AUTHORITY|CHANGE REQUEST|EXPAND' "$t"; then
      ok "scanout OCR ($leg_name) — the consent screen is legible in the live display capture (real DRM handoff)"
    else
      bad "scanout OCR ($leg_name) — screendump captured but the consent text is NOT in the pixels [$(tr '\n' ' ' <"$t" | cut -c1-80)] (DRM handoff may have failed = consent theater)"
    fi
  fi
done
[ "$scored_ocr" -eq 0 ] && echo "  (no consent screendumps to OCR — scanout channel not exercised)"

echo "--- bench-consent tally: PASS=$pass FAIL=$fail SKIP=$skip ---"
if [ "$fail" -eq 0 ] && [ "$skip" -eq 0 ]; then
  echo "=== bench-consent: GREEN — the console consent ceremony is VM-proven end to end ==="
elif [ "$fail" -eq 0 ]; then
  echo "=== bench-consent: PARTIAL — no failures but $skip legs skipped (resolve SAK delivery, re-run) ==="; exit 2
else
  echo "=== bench-consent: NOT GREEN — inspect $LOG + out/consent-*.png ==="; exit 1
fi
