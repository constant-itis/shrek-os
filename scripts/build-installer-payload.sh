#!/usr/bin/env bash
# Build a read-only payload disk for the live installer VM.
#
# The live installer runs from the normal Shrek base + shrek-installer sysext. This payload disk carries
# the exact sealed install image plus the installed-system layer store that must be copied to the target
# disk. Do not use the live installer store here; installed systems should not carry the installer layer.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

BASE="${BASE:-out/shrek-install-base.raw}"
LAYERS="${LAYERS:-out/layer-store-desktop.raw}"
OUT="${OUT:-out/shrek-install-payload.raw}"
[ -f "$BASE" ] || { echo "missing BASE=$BASE" >&2; exit 1; }
[ -f "$LAYERS" ] || { echo "missing LAYERS=$LAYERS" >&2; exit 1; }

# --- GATE: the base packaged for the target disk MUST be a clean INSTALLABLE product base ------------
# Every variant emits the same out/shrek_1_x86-64.raw filename; the distinct artifacts exist only via a
# manual cp, and this script packages whatever is named $BASE with no other check. Assert the security
# property on the SEALED ROOT itself (not staging state): the product must NOT carry the dev NOPASSWD
# placeholder (bench authz slice step 4), and must be the INSTALLABLE build (home.mount enabled, swamp NOT
# masked) — the mirror of build-installer-usb.sh's GATE A. Fail closed so a mislabeled LIVE_INSTALLER base
# (which DOES carry dev NOPASSWD) can never be shipped as the product. Loop+mount need root.
echo "--- GATE: verifying $BASE is a clean INSTALLABLE product base (no dev NOPASSWD) ---"
command -v losetup >/dev/null 2>&1 || { echo "gate: losetup missing" >&2; exit 1; }
sudo sh -s "$BASE" <<'GATE'
set -eu
base="$1"
lo=$(losetup --show -fP "$base")
mnt=$(mktemp -d)
cleanup() { mountpoint -q "$mnt" 2>/dev/null && umount "$mnt" 2>/dev/null || true; rmdir "$mnt" 2>/dev/null || true; losetup -d "$lo" 2>/dev/null || true; }
trap cleanup EXIT
udevadm settle 2>/dev/null || true
i=0; while [ ! -b "${lo}p2" ] && [ "$i" -lt 30 ]; do sleep 0.2; i=$((i+1)); done
mount -o ro "${lo}p2" "$mnt"
fail=""
[ -e "$mnt/etc/sudoers.d/dev-nopasswd" ] && fail="$fail dev-nopasswd-PRESENT"
[ "$(readlink "$mnt/etc/systemd/system/var-lib-swamp.mount" 2>/dev/null || true)" = "/dev/null" ] && fail="$fail swamp-masked(live-installer-base?)"
[ -e "$mnt/usr/lib/systemd/system/local-fs.target.wants/home.mount" ] || fail="$fail home.mount-not-enabled(not-installable?)"
# Owner-provisioning (#2939): the product MUST ship the first-boot owner wizard enabled (getty@tty1
# Requires it, via the drop-in) in INTERACTIVE mode, and MUST NOT carry the DOGFOOD baked test seed — else
# a real box would either boot on the public `shrek` credential or provision itself from a public passphrase.
[ -e "$mnt/etc/systemd/system/getty@tty1.service.d/50-owner-provision.conf" ] || fail="$fail owner-provision-not-enabled(not-installable?)"
grep -q '^SHREK_PROVISION_MODE=interactive' "$mnt/etc/shrek/owner-provision.env" 2>/dev/null || fail="$fail owner-provision-not-interactive"
[ -e "$mnt/etc/shrek/owner-seed" ] && fail="$fail dogfood-owner-seed-PRESENT(dogfood-base?)"
if [ -n "$fail" ]; then
  echo "!!! $base is NOT a clean INSTALLABLE product base:$fail" >&2
  echo "!!! Rebuild with: INSTALLABLE=1 scripts/build-in-container.sh 1  then cp out/shrek_1_x86-64.raw $base" >&2
  exit 1
fi
GATE
echo "--- GATE: $BASE confirmed INSTALLABLE + no dev NOPASSWD ---"

stage=out/install-payload-stage
rm -rf "$stage"
mkdir -p "$stage"
cp "$BASE" "$stage/shrek_1_x86-64.raw"
cp "$LAYERS" "$stage/layer-store.raw"
( cd "$stage" && sha256sum shrek_1_x86-64.raw layer-store.raw > SHA256SUMS )

mb=$(( $(du -sm "$stage" | cut -f1) + 512 ))
rm -f "$OUT"
mkfs.ext4 -q -L shrek-payload -d "$stage" "$OUT" "${mb}M"
rm -rf "$stage"
echo "payload ready: $OUT"
sha256sum "$OUT"
