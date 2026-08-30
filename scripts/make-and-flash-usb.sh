#!/usr/bin/env bash
# One-shot: (re)build the live-installer USB image if stale, flash it to the USB stick, and
# verity-verify the stick + a source control. Nothing to paste.
#
#   sudo scripts/make-and-flash-usb.sh            # auto-detect the single USB disk
#   sudo scripts/make-and-flash-usb.sh /dev/sdX   # or name the target explicitly
#
# Safety: refuses a non-removable device, the disk backing /, or a stick smaller than the image.
set -euo pipefail
SELF="$(readlink -f "${BASH_SOURCE[0]}")"
REPO_ROOT="$(cd "$(dirname "$SELF")/.." && pwd)"; cd "$REPO_ROOT"

IMG="${IMG:-out/shrek-installer-usb.img}"
STORE="${STORE:-out/layer-store-installer.raw}"

if [ "$(id -u)" -ne 0 ]; then
  echo "=== re-exec under sudo ==="
  exec sudo -E IMG="$IMG" STORE="$STORE" "$SELF" "$@"
fi

for t in losetup dd partprobe udevadm veritysetup objcopy blockdev lsblk findmnt; do
  command -v "$t" >/dev/null 2>&1 || { echo "missing tool: $t" >&2; exit 1; }
done

# 1. (Re)assemble the image if missing or older than the installer store.
if [ ! -f "$IMG" ] || { [ -f "$STORE" ] && [ "$STORE" -nt "$IMG" ]; }; then
  echo "=== image missing or stale -> assembling (build-installer-usb.sh) ==="
  "$REPO_ROOT/scripts/build-installer-usb.sh"
else
  echo "=== image is current: $IMG ==="
fi
[ -f "$IMG" ] || { echo "no image at $IMG" >&2; exit 1; }

# 2. Pick the target device.
DEV="${1:-${DEV:-}}"
if [ -z "$DEV" ]; then
  mapfile -t cands < <(lsblk -dnpo NAME,RM,TYPE,TRAN | awk '$2==1 && $3=="disk" && $4=="usb"{print $1}')
  if [ "${#cands[@]}" -eq 1 ]; then DEV="${cands[0]}"; echo "auto-selected USB: $DEV"
  else echo "could not auto-pick (candidates: ${cands[*]:-none}); run: sudo $SELF /dev/sdX" >&2; exit 1; fi
fi

# 3. Safety guards.
[ -b "$DEV" ] || { echo "REFUSING: $DEV is not a block device" >&2; exit 1; }
# lsblk right-aligns even with -n, so trim whitespace. Accept removable OR a USB-transport disk
# (some USB sticks report RM=0); this still rejects internal nvme/sata (TRAN!=usb, RM=0).
rm_flag="$(lsblk -dno RM "$DEV" | tr -d '[:space:]')"
tran="$(lsblk -dno TRAN "$DEV" | tr -d '[:space:]')"
[ "$rm_flag" = "1" ] || [ "$tran" = "usb" ] || {
  echo "REFUSING: $DEV is neither removable nor USB (RM=$rm_flag TRAN=$tran)" >&2; exit 1; }
rootsrc="$(findmnt -no SOURCE / || true)"
rootdisk="$(lsblk -no PKNAME "$rootsrc" 2>/dev/null | head -1 || true)"
if [ -n "$rootdisk" ] && [ "$(basename "$DEV")" = "$rootdisk" ]; then
  echo "REFUSING: $DEV backs the root filesystem" >&2; exit 1
fi
imgsz="$(stat -c%s "$IMG")"; devsz="$(blockdev --getsize64 "$DEV")"
[ "$devsz" -ge "$imgsz" ] || { echo "REFUSING: $DEV ($((devsz/1048576)) MiB) < image ($((imgsz/1048576)) MiB)" >&2; exit 1; }
echo "=== target: $DEV  [$(lsblk -dno SIZE,MODEL "$DEV" | xargs)] ; image $((imgsz/1048576)) MiB ==="

part() { case "$DEV" in *nvme*|*mmcblk*|*loop*) echo "${DEV}p$1" ;; *) echo "${DEV}$1" ;; esac; }

# 4. Derive the roothash from the image's UKI (no hardcode — always matches this image).
lo="$(losetup --show -fP "$IMG")"; m="$(mktemp -d)"
cleanup() { mountpoint -q "$m" 2>/dev/null && umount "$m" 2>/dev/null || true; rmdir "$m" 2>/dev/null || true; losetup -d "$lo" 2>/dev/null || true; }
trap cleanup EXIT
mount "${lo}p1" "$m"
uki="$(ls "$m"/EFI/Linux/shrek_*_x86-64.efi 2>/dev/null | sort -V | tail -1 || true)"
[ -n "$uki" ] || { echo "no UKI on image ESP" >&2; exit 1; }
tmpc="$(mktemp)"; objcopy -O binary --only-section=.cmdline "$uki" "$tmpc"
RH="$(tr -d '\0' < "$tmpc" | tr ' ' '\n' | sed -n 's/^roothash=//p')"; rm -f "$tmpc"
umount "$m"; losetup -d "$lo"; trap - EXIT
[ -n "$RH" ] || { echo "could not derive roothash from image UKI" >&2; exit 1; }
echo "roothash=$RH"

# 5. Flash.
echo "=== flashing (unmounting any auto-mounted partitions first) ==="
umount "${DEV}"?* 2>/dev/null || true
dd if="$IMG" of="$DEV" bs=4M status=progress conv=fsync
sync; partprobe "$DEV" 2>/dev/null || true; udevadm settle 2>/dev/null || true; sleep 1

# 6. Read-back verify: the stick, then the source image as a control.
echo "=== verify stick (reads it back — catches a bad flash) ==="
if veritysetup verify "$(part 2)" "$(part 3)" "$RH"; then stick=OK; else stick=FAIL; fi
echo "=== control (source image) ==="
lo="$(losetup --show -fP "$IMG")"
if veritysetup verify "${lo}p2" "${lo}p3" "$RH"; then ctrl=OK; else ctrl=FAIL; fi
losetup -d "$lo" 2>/dev/null || true

echo
echo "================  RESULT  ================"
echo "  STICK-$stick    CONTROL-$ctrl"
if [ "$stick" = OK ] && [ "$ctrl" = OK ]; then
  echo "  ✅ stick is good — boot the Mac: hold Option -> EFI Boot -> rEFInd -> \"Shrek OS Installer\""
elif [ "$ctrl" = OK ]; then
  echo "  ❌ bad flash/media — re-run this script (use a USB 3.0 stick if it fails twice)"
else
  echo "  ❌ CONTROL failed too — tooling/roothash issue, not the stick; tell Claude"
fi
echo "=========================================="
