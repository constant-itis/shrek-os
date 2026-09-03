#!/usr/bin/env bash
# Assemble a single bootable live-installer USB image for Shrek OS.
#
# The live installer has only ever run as a 4-virtio-disk VM (base + shrek-layers store +
# shrek-payload + blank target). On real hardware you have ONE stick, so this collapses the
# three *source* disks onto one GPT image. The 4th disk (the blank target) is the machine's
# real internal disk at install time.
#
# Discovery is byte-for-byte identical to the VM: nothing is found by device path.
#   - gatekeeperd mounts the store by  /dev/disk/by-label/shrek-layers  (main.rs:431)
#   - shrek-install-target reads        /dev/disk/by-label/shrek-payload (shrek-install-target:42)
# So the store and payload just become two extra fs-labelled partitions on the stick. The stick
# itself is excluded as an install target because it backs the live root AND carries shrek labels
# (shrek-list-disks filters both).
#
# Layout produced (mirrors shrek-install-target's append convention):
#   p1..p6   the live base image, copied verbatim (esp / root+verity / A-B slots / swamp)
#   p7       shrek-layers   <- STORE   (desktop + installer sysexts: gparted, Quickshell installer, writer)
#   p8       shrek-payload  <- PAYLOAD (the sealed installed image written to the target)
#
# With STAGE_REFIND=1 (default) the ESP is fitted with rEFInd + a loose kernel/initrd so 2012-era
# Apple firmware boots it with no post-dd ESP surgery (docs/hardware-boot.md §4). The UKI is also
# run through the mandatory pre-flash USB-stack gate (scripts/initrd-usb-check.py) either way.
set -euo pipefail

# Absolute self path FIRST (survives the sudo re-exec + any cwd change; do not use git rev-parse,
# which dies on "dubious ownership" in a bare root shell with no SUDO_UID — fable finding 5).
SELF="$(readlink -f "${BASH_SOURCE[0]}")"
REPO_ROOT="$(cd "$(dirname "$SELF")/.." && pwd)"
cd "$REPO_ROOT"

# --- inputs (override via env) ---------------------------------------------------------------
BASE="${BASE:-out/shrek-installer-base.raw}"        # LIVE_INSTALLER=1 base (boots the installer)
STORE="${STORE:-out/layer-store-installer.raw}"     # desktop+installer sysext store (label shrek-layers)
PAYLOAD="${PAYLOAD:-out/shrek-install-payload.raw}" # installed base + desktop store (label shrek-payload)
OUT="${OUT:-out/shrek-installer-usb.img}"
SLACK_MB="${SLACK_MB:-256}"                          # free space padding
STAGE_REFIND="${STAGE_REFIND:-1}"                    # fit rEFInd + loose kernel/initrd on the ESP

# --- must run as root (loop + partition + mount ops) -----------------------------------------
if [ "$(id -u)" -ne 0 ]; then
  echo "=== build-installer-usb: re-exec under sudo (loop/partition ops need root) ==="
  exec sudo -E BASE="$BASE" STORE="$STORE" PAYLOAD="$PAYLOAD" OUT="$OUT" \
       SLACK_MB="$SLACK_MB" STAGE_REFIND="$STAGE_REFIND" "$SELF" "$@"
fi

for f in "$BASE" "$STORE" "$PAYLOAD"; do
  [ -f "$f" ] || { echo "missing input: $f" >&2; exit 1; }
done
for t in sgdisk losetup partprobe udevadm dd e2label blkid; do
  command -v "$t" >/dev/null 2>&1 || { echo "missing tool: $t" >&2; exit 1; }
done

# Sanity: the store/payload really carry the labels discovery relies on.
[ "$(blkid -o value -s LABEL "$STORE")"   = "shrek-layers" ]  || { echo "STORE label != shrek-layers"   >&2; exit 1; }
[ "$(blkid -o value -s LABEL "$PAYLOAD")" = "shrek-payload" ] || { echo "PAYLOAD label != shrek-payload" >&2; exit 1; }

base_bytes=$(stat -c%s "$BASE")
store_bytes=$(stat -c%s "$STORE")
payload_bytes=$(stat -c%s "$PAYLOAD")
total_mb=$(( (base_bytes + store_bytes + payload_bytes) / 1048576 + SLACK_MB + 16 ))

echo "=== build-installer-usb ==="
echo "  base    $BASE    ($(( base_bytes    / 1048576 )) MB)"
echo "  store   $STORE   ($(( store_bytes   / 1048576 )) MB)  -> p7 shrek-layers"
echo "  payload $PAYLOAD ($(( payload_bytes / 1048576 )) MB)  -> p8 shrek-payload"
echo "  out     $OUT     (~${total_mb} MB)"

# --- foundation: the live base, grown to hold the two appended partitions ---------------------
rm -f "$OUT"
cp --reflink=auto "$BASE" "$OUT"
truncate -s "${total_mb}M" "$OUT"

lo=$(losetup --show -fP "$OUT")
mnt=""
dl=""
cleanup() {
  [ -n "$mnt" ] && mountpoint -q "$mnt" && umount "$mnt" 2>/dev/null || true
  [ -n "$mnt" ] && rmdir "$mnt" 2>/dev/null || true
  [ -n "$dl" ] && rm -rf "$dl" 2>/dev/null || true
  losetup -d "$lo" 2>/dev/null || true
}
trap cleanup EXIT
echo "  loop    $lo"

# --- GATE A: verify BASE is really a LIVE_INSTALLER build (fable finding 1) --------------------
# The default filename is two chars from the INSTALLABLE base and every build mode emits the same
# name, so verify the mode from the sealed root itself. LIVE_INSTALLER uniquely masks
# var-lib-swamp.mount -> /dev/null and ships NO home.mount enable; an INSTALLABLE/DOGFOOD image
# would wait 90s on the absent shrek-data label -> emergency mode; a plain-CI image self-powers-off.
mnt=$(mktemp -d)
mount -o ro "${lo}p2" "$mnt"
swamp_link=$(readlink "$mnt/etc/systemd/system/var-lib-swamp.mount" 2>/dev/null || true)
home_enabled=""; [ -e "$mnt/usr/lib/systemd/system/local-fs.target.wants/home.mount" ] && home_enabled=1
# home.mount is by-label (LABEL=shrek-data) and login/session machinery pulls it even when it is not in
# local-fs.target.wants, so on the live medium it MUST be masked or it will mount a previously-installed
# target disk's /home into the live session (metal 2026-09-03). Not-wanted is necessary but not sufficient.
home_mask=$(readlink "$mnt/etc/systemd/system/home.mount" 2>/dev/null || true)
umount "$mnt"; rmdir "$mnt"; mnt=""
if [ "$swamp_link" != "/dev/null" ] || [ -n "$home_enabled" ] || [ "$home_mask" != "/dev/null" ]; then
  echo "!!! BASE is NOT a LIVE_INSTALLER build (var-lib-swamp mask='$swamp_link', home.mount enabled='${home_enabled:-no}', home.mount mask='${home_mask:-UNMASKED}')" >&2
  echo "!!! Rebuild with: LIVE_INSTALLER=1 scripts/build-in-container.sh 1  then cp out/shrek_1_x86-64.raw $BASE" >&2
  exit 1
fi
echo "--- GATE A: BASE confirmed LIVE_INSTALLER (swamp masked, home.mount masked + not enabled) ---"

# --- GATE B: mandatory pre-flash USB-stack check on the UKI (fable finding 3) ------------------
# image/mkosi.conf.d/40-usb-boot.conf makes this a hard gate: a UKI whose appended modules initrd
# lacks the USB stack boots fine in the VM but times out on veritysetup@root off a real stick.
mnt=$(mktemp -d)
mount "${lo}p1" "$mnt"                                   # ESP; kept mounted for rEFInd staging below
uki=$(ls "$mnt"/EFI/Linux/shrek_*_x86-64.efi 2>/dev/null | head -1 || true)
[ -n "$uki" ] || { echo "no UKI at ESP EFI/Linux — cannot gate or stage" >&2; exit 1; }
echo "--- GATE B: initrd USB-stack check on $(basename "$uki") ---"
python3 scripts/initrd-usb-check.py "$uki" || {
  echo "!!! initrd USB-stack gate FAILED — this stick would drop to emergency mode on real hardware." >&2
  echo "!!! (needs python3-zstandard + binutils; if a dep is missing, install it and re-run.)" >&2
  exit 1
}

# --- rEFInd + loose kernel/initrd (default; ESP already mounted at $mnt) -----------------------
if [ "$STAGE_REFIND" = "1" ]; then
  echo "--- staging rEFInd + loose kernel/initrd on the ESP (docs/hardware-boot.md §4) ---"
  command -v objcopy >/dev/null 2>&1 || { echo "!!! objcopy (binutils) required for STAGE_REFIND=1" >&2; exit 1; }
  refind_efi=""
  if [ -f /usr/share/refind/refind/refind_x64.efi ]; then
    refind_efi=/usr/share/refind/refind/refind_x64.efi
  else
    dl=$(mktemp -d); ( cd "$dl" && apt-get download refind >/dev/null 2>&1 ) || true
    deb=$(ls "$dl"/refind_*.deb 2>/dev/null | head -1 || true)
    [ -n "$deb" ] && { dpkg-deb -x "$deb" "$dl/x"; refind_efi="$dl/x/usr/share/refind/refind/refind_x64.efi"; }
  fi
  [ -n "$refind_efi" ] && [ -f "$refind_efi" ] || {
    echo "!!! could not obtain refind_x64.efi (install the 'refind' package or set STAGE_REFIND=0" >&2
    echo "!!!  and stage it by hand per docs/hardware-boot.md §4). Refusing to ship a Mac-unbootable stick." >&2
    exit 1
  }
  # Stage rEFInd via the shared helper — SINGLE source of truth for the recipe, also used by
  # shrek-install-target for the installed disk (keeps the two from drifting; fable's concern).
  layers/shrek-installer/overlay/usr/libexec/shrek/shrek-stage-refind "$mnt" "$refind_efi" "Shrek OS Installer"
fi
umount "$mnt"; rmdir "$mnt"; mnt=""

# --- append the two state partitions (mirror shrek-install-target:90-137) ---------------------
store_mb=$(( store_bytes / 1048576 + 1 ))
echo "--- extending GPT + appending shrek-layers (p7) and shrek-payload (p8) ---"
sgdisk -e "$lo"                                             # relocate backup GPT header to new end
sgdisk -n 7:0:+"${store_mb}"M -t 7:8300 -c 7:shrek-layers  "$lo"
sgdisk -n 8:0:0               -t 8:8300 -c 8:shrek-payload "$lo"
partprobe "$lo"; udevadm settle
partx -u "$lo" 2>/dev/null || partx -a "$lo" 2>/dev/null || true
udevadm settle

for _ in $(seq 1 25); do
  [ -b "${lo}p7" ] && [ -b "${lo}p8" ] && break
  sleep 0.2; partprobe "$lo" 2>/dev/null || true; udevadm settle
done
[ -b "${lo}p7" ] && [ -b "${lo}p8" ] || { echo "appended partitions never appeared" >&2; lsblk "$lo" >&2; exit 1; }

echo "--- writing store -> ${lo}p7 ---"
dd if="$STORE"   of="${lo}p7" bs=16M conv=fsync status=progress
echo "--- writing payload -> ${lo}p8 ---"
dd if="$PAYLOAD" of="${lo}p8" bs=16M conv=fsync status=progress
# labels are baked into the copied fs images; re-assert defensively (harmless if already set)
e2label "${lo}p7" shrek-layers  2>/dev/null || true
e2label "${lo}p8" shrek-payload 2>/dev/null || true
sync

losetup -d "$lo"; trap - EXIT
sync
img_mib=$(( $(stat -c%s "$OUT") / 1048576 ))
echo "=== live-installer USB image ready: $OUT (${img_mib} MiB) ==="
echo "Target stick must be >= ${img_mib} MiB. Flash it (verify RM=1, NOT an internal disk):"
echo "  lsblk -o NAME,SIZE,RM,MODEL"
echo "  sudo dd if=$OUT of=/dev/sdX bs=4M status=progress conv=fsync"
echo "Then read-back verify per docs/hardware-boot.md §3 before trusting the stick."
echo "After installing to the internal disk, REMOVE the stick before first boot (avoids a"
echo "duplicate shrek-layers label race with the installed disk's own store)."
