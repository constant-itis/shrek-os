#!/usr/bin/env bash
# INSTALL-0 disk-picker proof: exercise the Shrek target-disk filter
# (shrek-list-disks) headlessly against a synthesized live-VM block topology.
#
# The live installer boots with four virtio disks: the sealed live medium (vda),
# the layer store (vdb), the read-only install payload (vdc), and a blank target
# (vdd). Only the blank target must be offered for erase/install.
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"
LIST_DISKS="$REPO_ROOT/layers/shrek-installer/overlay/usr/libexec/shrek/shrek-list-disks"
[ -x "$LIST_DISKS" ] || chmod +x "$LIST_DISKS" 2>/dev/null || true

pass=0; fail=0
ok()  { echo "  PASS $*"; pass=$((pass + 1)); }
bad() { echo "  FAIL $*"; fail=$((fail + 1)); }

tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
bin="$tmp/bin"; mkdir -p "$bin"

# Fixture: mimic `lsblk -P` output for the live installer VM.
#  vda  = sealed live medium (esp + shrek_1 + verity partlabels, verity ro)
#  vdb  = layer store, whole-disk fs mounted into the live system
#  vdc  = install payload, whole-disk fs LABEL=shrek-payload (not yet mounted)
#  vdd  = blank target  <- the only eligible disk
write_fixture() {
    cat >"$bin/lsblk_p_rows" <<'ROWS'
NAME="vda" PKNAME="" TYPE="disk" RO="0" MOUNTPOINT="" PARTLABEL="" LABEL="" SIZE="8589934592" MODEL="QEMU HARDDISK"
NAME="vda1" PKNAME="vda" TYPE="part" RO="0" MOUNTPOINT="" PARTLABEL="esp" LABEL="ESP" SIZE="536870912" MODEL=""
NAME="vda2" PKNAME="vda" TYPE="part" RO="1" MOUNTPOINT="" PARTLABEL="shrek_1" LABEL="" SIZE="4294967296" MODEL=""
NAME="vda3" PKNAME="vda" TYPE="part" RO="1" MOUNTPOINT="" PARTLABEL="shrek_1_verity" LABEL="" SIZE="268435456" MODEL=""
NAME="vdb" PKNAME="" TYPE="disk" RO="0" MOUNTPOINT="/run/shrek/store" PARTLABEL="" LABEL="" SIZE="2147483648" MODEL="QEMU HARDDISK"
NAME="vdc" PKNAME="" TYPE="disk" RO="0" MOUNTPOINT="" PARTLABEL="" LABEL="shrek-payload" SIZE="3221225472" MODEL="QEMU HARDDISK"
NAME="vdd" PKNAME="" TYPE="disk" RO="0" MOUNTPOINT="" PARTLABEL="" LABEL="" SIZE="19327352832" MODEL="QEMU HARDDISK"
ROWS
}

cat >"$bin/lsblk" <<EOF
#!/bin/sh
for a in "\$@"; do
    [ "\$a" = "-P" ] && { cat "$bin/lsblk_p_rows"; exit 0; }
done
for a in "\$@"; do
    [ "\$a" = "-ndo" ] && { echo "vda2"; exit 0; }
done
echo "diag-lsblk"
EOF
chmod +x "$bin/lsblk"

cat >"$bin/findmnt" <<'EOF'
#!/bin/sh
echo "/dev/mapper/root"
EOF
chmod +x "$bin/findmnt"

run_list() {
    SHREK_LSBLK="$bin/lsblk" SHREK_FINDMNT="$bin/findmnt" "$LIST_DISKS"
}

echo "=== scenario: live VM with one blank target ==="
write_fixture
out="$(run_list)"
echo "--- eligible disks ---"; printf '%s\n' "$out"; echo "----------------------"

paths="$(printf '%s\n' "$out" | cut -f1 | sort | tr '\n' ' ')"
[ "$paths" = "/dev/vdd " ] && ok "only /dev/vdd offered (got: $paths)" || bad "expected only /dev/vdd, got: $paths"
printf '%s\n' "$out" | grep -q "/dev/vda" && bad "live medium /dev/vda leaked" || ok "live medium excluded"
printf '%s\n' "$out" | grep -q "/dev/vdb" && bad "layer store /dev/vdb leaked" || ok "layer store excluded"
printf '%s\n' "$out" | grep -q "/dev/vdc" && bad "payload /dev/vdc leaked" || ok "payload excluded"
printf '%s\n' "$out" | grep -q "18G" && ok "target size rendered human-readable (18G)" || bad "target size not rendered as 18G"

echo "=== scenario: no blank disk attached ==="
# Drop the blank target from the fixture -> nothing eligible.
grep -v 'NAME="vdd"' "$bin/lsblk_p_rows" > "$bin/rows.tmp" && mv "$bin/rows.tmp" "$bin/lsblk_p_rows"
out_none="$(run_list || true)"
[ -z "$out_none" ] && ok "no disks offered when only live media present" || bad "unexpected disks offered: $out_none"

# --- reinstall regression (metal 2026-09-03) -------------------------------------------------------
# A disk that ALREADY has Shrek installed (esp + shrek_1 + shrek-layers + shrek-data labels, RO=0, not
# mounted) MUST be offered for erase — otherwise the picker shows "no eligible disk" on every machine that
# already runs Shrek and you can never reinstall from the GUI. The live medium (shrek-payload) stays out.
echo "=== scenario: reinstall over an existing Shrek disk ==="
cat >"$bin/lsblk_p_rows" <<'ROWS'
NAME="vda" PKNAME="" TYPE="disk" RO="0" MOUNTPOINT="" PARTLABEL="" LABEL="" SIZE="8589934592" MODEL="LIVE USB"
NAME="vda2" PKNAME="vda" TYPE="part" RO="1" MOUNTPOINT="" PARTLABEL="shrek_1" LABEL="" SIZE="4294967296" MODEL=""
NAME="vda8" PKNAME="vda" TYPE="part" RO="0" MOUNTPOINT="" PARTLABEL="" LABEL="shrek-payload" SIZE="3221225472" MODEL=""
NAME="vde" PKNAME="" TYPE="disk" RO="0" MOUNTPOINT="" PARTLABEL="" LABEL="" SIZE="480103981056" MODEL="APPLE SSD"
NAME="vde1" PKNAME="vde" TYPE="part" RO="0" MOUNTPOINT="" PARTLABEL="esp" LABEL="ESP" SIZE="1073741824" MODEL=""
NAME="vde2" PKNAME="vde" TYPE="part" RO="0" MOUNTPOINT="" PARTLABEL="shrek_1" LABEL="shrek_1" SIZE="2147483648" MODEL=""
NAME="vde7" PKNAME="vde" TYPE="part" RO="0" MOUNTPOINT="" PARTLABEL="shrek-layers" LABEL="shrek-layers" SIZE="4187593728" MODEL=""
NAME="vde8" PKNAME="vde" TYPE="part" RO="0" MOUNTPOINT="" PARTLABEL="shrek-data" LABEL="shrek-data" SIZE="470000000000" MODEL=""
ROWS
out_re="$(run_list)"
paths_re="$(printf '%s\n' "$out_re" | cut -f1 | sort | tr '\n' ' ')"
[ "$paths_re" = "/dev/vde " ] && ok "existing-Shrek target offered for reinstall (got: $paths_re)" || bad "expected only /dev/vde, got: $paths_re"
printf '%s\n' "$out_re" | grep -q "/dev/vda" && bad "live medium (shrek-payload) leaked" || ok "live medium still excluded via shrek-payload"

# --- courtesy vs system mount ----------------------------------------------------------------------
# A target whose partition is auto-mounted under /media (udisks/file-manager courtesy) stays selectable;
# a disk mounted at a system path does not.
echo "=== scenario: courtesy /media mount stays eligible; system mount excluded ==="
sed 's#NAME="vde8"\(.*\)MOUNTPOINT=""#NAME="vde8"\1MOUNTPOINT="/media/nhac/shrek-data"#' "$bin/lsblk_p_rows" > "$bin/rows.tmp" && mv "$bin/rows.tmp" "$bin/lsblk_p_rows"
out_courtesy="$(run_list)"
printf '%s\n' "$out_courtesy" | grep -q "/dev/vde" && ok "target with a /media courtesy mount still offered" || bad "target wrongly excluded by a /media mount"
sed 's#MOUNTPOINT="/media/nhac/shrek-data"#MOUNTPOINT="/mnt/data"#' "$bin/lsblk_p_rows" > "$bin/rows.tmp" && mv "$bin/rows.tmp" "$bin/lsblk_p_rows"
out_sys="$(run_list || true)"
printf '%s\n' "$out_sys" | grep -q "/dev/vde" && bad "disk mounted at a system path (/mnt) wrongly offered" || ok "disk mounted at a system path excluded"

echo "--- INSTALL-0 disk-picker tally: PASS=$pass FAIL=$fail ---"
[ "$fail" -eq 0 ] && echo "=== INSTALL-0 disk-picker proof GREEN ===" || { echo "=== INSTALL-0 disk-picker proof NOT GREEN ==="; exit 1; }
