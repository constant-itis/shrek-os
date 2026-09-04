#!/usr/bin/env bash
# desktop-egress-s6-vm-proof.sh — sealed-VM acceptance for the ADR-007 desktop-egress plane (S6b, §11.6).
# Sibling of scripts/bench-consent-vm-proof.sh; reuses its docker+qemu boot block, its OVMF Secure-Boot
# flags, its serial CONSENT-CUE cue-loop (chord / stop-chord / shot / answer + the SAK-chord repeater), and
# its PASS/FAIL tally shape. This is the ONE proof the netns host-oracles (S2e) and the S5 NTP proof cannot
# give: the LIVE browser-cgroup matcher and the SAK/VT raw-destination ceremony driven END TO END on a
# booted, sealed, Secure-Boot image under a real compositor — plus the cold-boot NTP recovery live.
#
# It reuses the DOGFOOD persist probe's two-boot cycle (so boot2 has a live desktop) and marker-gates the
# S6 stage exactly like bench-consent: it seeds
#   1. /home/.shrek-egress-dogfood        — the marker that makes the persist probe invoke the S6 probe;
#   2. /home/.shrek-egress-probe          — the CURRENT image/overlay/.../dogfood-egress-probe (fast
#                                            iteration: probe edits need no full image rebuild).
# both via debugfs into the FRESH DATA disk, then drives the SAK ceremony over the serial console.
#
# S6 gates scored below (each a `SHREK-DOGFOOD S6 <check>=<value>` line the tally greps):
#   ntp                      NTP synced off the SEALED literal IPs from a FORWARD-skewed RTC, name-free (MF-4)
#   map-eq                   the /run pin map == the live @weather_pinned nft set (§11.6 #1)
#   weather-reach            uid-1000 reaches pinned open-meteo via --resolve, TLS verifies the sealed name (#2)
#   unblessed-drop           an unblessed dest from uid 1000 DROPs (#2)
#   cgroup-path              the LIVE shrekbrowser.slice cgroup path == the baked constant (§11.6 #3)
#   inslice-accept           a process INSIDE shrekbrowser.slice reaches the DNS stub (browser accept-pair) (#3)
#   nonbrowser-drop          the SAME probe OUTSIDE the slice is DROPped by rule-0 (#3)
#   double-launch            a fresh scope in the SAME static slice still ACCEPTs (MF-5)
#   revoke-drop              confirmed-unbless tears down the accept-pair; the slice now DROPs (MF-5)
#   repin                    a DoT re-pin reconciles @weather_pinned by element, no flush (§11.6 #5)
#   daemon-death-failclosed  kill -9 egressd: unblessed still drops, blessed weather still reaches (MF-5)
#   sak-raw                  a raw host:proto:port added through the console SAK ceremony pins @raw_pinned
#                            (owner steer). Emits DEFERRED (a NOTE, not a FAIL) if the live ceremony wiring
#                            is uncertain on the first run — see the probe's NEEDS-LIVE-TUNING comments.
#
# MF-4 qemu deltas vs bench-consent: a FORWARD-skewed RTC (`-rtc base=2030-...`) so the NTP cold-boot
# recovery is real, and an EXPLICIT user NIC (`-nic user,model=virtio-net-pci`) — S6 dials the network, so
# it must not lean on qemu's implicit default NIC the way bench-consent does.
#
# HONEST SCOPE: this proves the cgroup matcher + the raw ceremony ALONGSIDE A HEALTHY compositor and a real
# supervisor; it is NOT an adversarial-boundary claim for the cgroup (a uid-1000 process can join the slice
# by design — §7 Q7 "accident/UX containment, not attacker containment"). The stub reachability tool is a
# shell /dev/tcp probe; the exact connect tool may need live tuning (base image tool inventory unknown).
#
# Prereqs (same as bench-consent/dogfood-vm.sh): scripts/build-desktop-layer.sh ; DOGFOOD=1
# scripts/build-in-container.sh 1 ; INCLUDE_DEV=1 scripts/build-layers.sh desktop. The store MUST carry the
# BROWSER layer (shrek-browser) so shrekbrowser.slice + the browser plumbing exist. Needs /dev/kvm + docker.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"; mkdir -p out

RAW="${RAW:-$(ls -t out/shrek_*_x86-64.raw 2>/dev/null | head -1)}"
[ -n "$RAW" ] && [ -f "$RAW" ] || { echo "no out/shrek_*_x86-64.raw — run DOGFOOD=1 scripts/build-in-container.sh 1 first" >&2; exit 1; }
STORE="${STORE:-out/layer-store.raw}"
[ -f "$STORE" ] || { echo "no $STORE — run scripts/build-layers.sh desktop first" >&2; exit 1; }
grep -qa 'shrek-dev.raw' "$STORE" || { echo "$STORE has no shrek-dev toolchain — rebuild: INCLUDE_DEV=1 scripts/build-layers.sh desktop" >&2; exit 1; }
# S6 needs the BROWSER layer: shrekbrowser.slice + the browser plumbing must be present, or cgroup-path,
# inslice-accept, double-launch and revoke-drop cannot run. ext4 records the staged filename in a dir block,
# so a raw grep is a cheap mount-free probe — fail fast with the exact fix.
grep -qa 'shrek-browser' "$STORE" || { echo "$STORE has no shrek-browser layer — S6 cgroup legs need it. Rebuild with the browser layer staged into the store." >&2; exit 1; }

# FRESH disposable data disk each run (same as bench-consent/dogfood-vm.sh) so boot1 sees an empty /home,
# writes its marker + reboots, and the S6 stage starts from a clean /home each run. We inject the S6 marker
# + the override probe into it inside the container (debugfs), so they land on the persistent /home for boot2.
DATA="${DATA:-out/dogfood-data.raw}"
FRESH=1 scripts/dogfood-data-disk.sh "$DATA" 4G

# S6 adds an NTP settle (~up to 60s from a skewed RTC), the cgroup legs, a DoT re-pin, and one interactive
# SAK ceremony on top of the two-boot persist cycle + desktop bring-up — so a bigger budget than the plain
# dogfood, in the bench-consent ballpark.
BUDGET="${BUDGET:-700}"
SHOT="${SHOT:-consent-desktop}"                # basename for the settle screendump (parity with bench-consent)
SAK_CHORD="${SAK_CHORD:-ctrl-alt-shift-esc}"   # the SecureAttentionKey chord logind detects (50-shrek-sak.conf)
# MF-4: skew the RTC FORWARD (past-skew hits systemd's behind-epoch clamp => vacuous). +8h is unambiguously
# wrong so timesyncd MUST correct it off the sealed IPs, yet stays inside TLS cert validity so the DoT
# re-pin of weather (which needs a sane clock) still succeeds afterward — a far-future skew (2030) instead
# EXPIRED every cert and broke the whole clock->DoT->weather chain the proof is trying to show.
RTC_BASE="${RTC_BASE:-$(date -u -d '+8 hours' '+%Y-%m-%dT%H:%M:%S' 2>/dev/null || echo 2026-09-05T00:00:00)}"
LOG=out/egress-s6-console.log; : > "$LOG"
rm -f out/s6-sak-*.ppm out/s6-sak-*.png out/s6-sak-*.txt out/egress-s6-desktop.png

echo "=== desktop-egress S6 VM proof: booting $RAW (+store $STORE +data $DATA) budget ${BUDGET}s chord=[$SAK_CHORD] ==="
docker run --rm --device /dev/kvm \
  -v "${REPO_ROOT}:/work" -w /work \
  -e RAW="$RAW" -e STORE="$STORE" -e DATA="$DATA" -e BUDGET="$BUDGET" -e SAK_CHORD="$SAK_CHORD" -e LOG="$LOG" -e RTC_BASE="$RTC_BASE" \
  debian:trixie \
  bash -euo pipefail -c '
    apt-get update -qq >/dev/null
    apt-get install -y --no-install-recommends -qq qemu-system-x86 ovmf socat netpbm e2fsprogs tesseract-ocr >/dev/null
    tmp=$(mktemp -d); monsock="$tmp/mon.sock"
    cp /usr/share/OVMF/OVMF_VARS_4M.fd "$tmp/vars.fd"

    # Seed the S6 marker at the ROOT of the data fs (mounts as /home) => /home/.shrek-egress-dogfood, and
    # the CURRENT probe => /home/.shrek-egress-probe so probe edits skip a full image rebuild. Mirror
    # bench-consent-vm-proof.sh: TWO debugfs writes, best-effort with a legible NOTE on failure.
    printf "s6 desktop-egress dogfood marker\n" > "$tmp/emarker"
    debugfs -w -R "write $tmp/emarker /.shrek-egress-dogfood" "/work/$DATA" >/dev/null 2>&1 \
      && echo "seeded egress marker into $DATA" || echo "NOTE: debugfs marker seed FAILED (S6 stage will not run)"
    debugfs -w -R "write /work/image/overlay/usr/lib/shrek/dogfood-egress-probe /.shrek-egress-probe" "/work/$DATA" >/dev/null 2>&1 \
      && echo "seeded override probe into $DATA" || echo "NOTE: override probe seed FAILED (baked probe will run)"

    # Spare labelled USB disk so the persist probe S4 leg still has its target (kept identical to dogfood-vm.sh).
    usbdir=$(mktemp -d); printf "shrek s4 udisks mount proof\n" > "$usbdir/shrek-usb-marker"
    truncate -s 64M "$tmp/usb.raw"; mkfs.ext4 -q -L SHREKUSB -d "$usbdir" "$tmp/usb.raw"

    mon() { printf "%s\n" "$1" | socat - "UNIX-CONNECT:$monsock" >/dev/null 2>&1 || true; }

    # MF-4 deltas vs bench-consent: a FORWARD-skewed RTC so timesyncd must correct off the sealed IPs live,
    # and an EXPLICIT user NIC (bench-consent leans on qemu default; S6 dials the network, so make it real).
    qemu-system-x86_64 \
      -machine q35,smm=on -accel kvm -cpu host -m 4096 -smp 4 \
      -rtc base=$RTC_BASE \
      -global driver=cfi.pflash01,property=secure,value=on \
      -drive if=pflash,format=raw,unit=0,file=/usr/share/OVMF/OVMF_CODE_4M.secboot.fd,readonly=on \
      -drive if=pflash,format=raw,unit=1,file="$tmp/vars.fd" \
      -drive file="/work/$RAW",format=raw,if=virtio,snapshot=on \
      -drive file="/work/$STORE",format=raw,if=virtio,snapshot=on \
      -drive file="/work/$DATA",format=raw,if=virtio \
      -drive file="$tmp/usb.raw",format=raw,if=virtio,snapshot=on \
      -device virtio-vga -device virtio-keyboard-pci -device virtio-tablet-pci -device virtio-rng-pci \
      -nic user,model=virtio-net-pci \
      -display none -serial file:/work/'"$LOG"' \
      -monitor "unix:$monsock,server,nowait" &
    QPID=$!

    # ---- serial cue-loop: VERBATIM from bench-consent-vm-proof.sh, plus S6-done as a stop signal ---------
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
          # S6 stop signal (the egress probe finishes with `S6 done`); keep the consent `done` too.
          case "$line" in *"SHREK-DOGFOOD S6 done"*) DONE=1 ;; *"BENCH-CONSENT done"*) DONE=1 ;; esac
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
    sleep 2; mon "screendump /work/out/egress-s6-desktop.ppm"; sleep 0.6
    [ -s /work/out/egress-s6-desktop.ppm ] && pnmtopng /work/out/egress-s6-desktop.ppm > /work/out/egress-s6-desktop.png 2>/dev/null && rm -f /work/out/egress-s6-desktop.ppm
    for p in /work/out/s6-sak-*.png; do
      [ -e "$p" ] || continue
      b="${p%.png}"
      tesseract "$p" "$b" --psm 6 >/dev/null 2>&1 || true   # writes ${b}.txt
    done

    mon "quit"; kill "$QPID" 2>/dev/null; wait "$QPID" 2>/dev/null
  '

echo "=== serial log: $LOG ($(wc -l < "$LOG" 2>/dev/null || echo 0) lines) ==="
ls -l out/s6-sak-*.png out/egress-s6-desktop.png 2>/dev/null || echo "no screendumps captured"

# ---- verdict: score the SHREK-DOGFOOD S6 lines --------------------------------------------------------
# set +e: the verdict is explicit pass/fail accounting — a grep that finds nothing (a MISSING gate) must
# count as a FAIL below, not abort the whole verdict under set -e/pipefail.
set +e
echo
echo "=== desktop-egress S6 acceptance verdict ==="
pass=0; fail=0; note=0
ok()  { echo "  PASS $*"; pass=$((pass + 1)); }
bad() { echo "  FAIL $*"; fail=$((fail + 1)); }
nte() { echo "  NOTE $*"; note=$((note + 1)); }

line_for() { grep -a "SHREK-DOGFOOD S6 $1=" "$LOG" | tail -1 | tr -d '\r'; }

# A gate passes iff its value is the ok-token for that gate. `gate KEY OK-TOKEN "label"`.
gate() {
  l=$(line_for "$1"); v=${l#*"$1="}; v=${v%% *}
  case "$v" in
    "$2") ok  "$1 — $3 [$v]" ;;
    "")   bad "$1 — $3 [no S6 line — probe did not reach this gate]" ;;
    *)    bad "$1 — $3 [$l]" ;;
  esac
}

# Did the probe run at all?
if ! grep -qa 'SHREK-DOGFOOD S6 begin' "$LOG"; then
  echo "  the S6 egress probe never started — no 'S6 begin' on serial."
  echo "  (marker not seeded? persist probe never reached boot2? browser layer absent? inspect $LOG)"
  echo "=== S6 desktop-egress VM: PASS=0 FAIL=1 ==="; exit 1
fi

# ntp: ok-token is `synced` (name-free bootstrap is a separate token on the same line, checked below).
gate ntp synced "NTP synced off the sealed literal IPs from a forward-skewed RTC (MF-4)"
# name-free assertion rides the ntp line's noname=<yes|no> field.
if grep -qa 'SHREK-DOGFOOD S6 ntp=.*noname=yes' "$LOG"; then ok "ntp-noname — timesyncd used NO ServerName (name-free bootstrap, §5 [R2-MF-C])"
else bad "ntp-noname — a ServerName was present ($(line_for ntp))"; fi

gate map-eq        yes   "the /run pin map == the live @weather_pinned nft set (§11.6 #1)"
gate weather-reach ok    "uid-1000 reaches pinned open-meteo via --resolve, TLS verifies the sealed name (#2)"
gate unblessed-drop ok   "an unblessed dest from uid 1000 is dropped (#2)"
gate cgroup-path   match "the LIVE shrekbrowser.slice cgroup path == the baked constant (§11.6 #3)"
gate inslice-accept ok   "a process INSIDE shrekbrowser.slice reaches the DNS stub — browser accept-pair (#3)"
gate nonbrowser-drop ok  "the SAME probe OUTSIDE the slice is dropped by rule-0 (#3)"
gate double-launch ok    "a fresh scope in the SAME static slice still ACCEPTs (MF-5)"
gate revoke-drop   ok    "confirmed-unbless tears down the accept-pair; the slice now DROPs (MF-5)"
gate repin         ok    "a DoT re-pin reconciles @weather_pinned by element, no flush (§11.6 #5)"
gate daemon-death-failclosed ok "kill -9 egressd: unblessed still drops, blessed weather still reaches (MF-5)"

# sak-raw: ok passes; DEFERRED is a NOTE (not a FAIL) so a first run can be green on everything else while
# the live ceremony wiring is tuned; anything else (incl. missing) is a FAIL.
sr=$(line_for sak-raw); srv=${sr#*sak-raw=}; srv=${srv%% *}
case "$srv" in
  ok)       ok  "sak-raw — a raw host:proto:port added through the console SAK ceremony pins @raw_pinned (§11.6 owner steer)" ;;
  DEFERRED) nte "sak-raw — DEFERRED (live ceremony wiring unverified; not a FAIL) [$sr]" ;;
  "")       bad "sak-raw — no S6 line [probe did not reach the SAK ceremony leg]" ;;
  *)        bad "sak-raw — the raw ceremony did not pin @raw_pinned [$sr]" ;;
esac

# OCR the cued mid-ceremony screendump as a second evidence channel (the consent screen must have SCANNED
# OUT, not just landed in the tty8 buffer). INFO-only: the primary sak-raw verdict is the nft-element proof.
for t in out/s6-sak-raw.txt; do
  [ -f "$t" ] || continue
  if grep -qiE 'AUTHORITY|CHANGE REQUEST|EGRESS|203\.0\.113' "$t"; then
    echo "  INFO scanout OCR — the raw-ceremony screen is legible in the live capture (real DRM handoff)"
  else
    echo "  INFO scanout OCR — screendump captured but the ceremony text is not legible [$(tr '\n' ' ' <"$t" | cut -c1-80)]"
  fi
done

echo "--- S6 tally: PASS=$pass FAIL=$fail NOTE=$note ---"
echo "=== S6 desktop-egress VM: PASS=$pass FAIL=$fail ==="
[ "$fail" -eq 0 ] || exit 1
if [ "$note" -gt 0 ]; then
  echo "=== S6: GREEN on scored gates; $note gate(s) DEFERRED for live wiring (sak-raw) ==="
else
  echo "=== S6: GREEN — the desktop-egress plane is VM-proven end to end ==="
fi
exit 0
