#!/usr/bin/env bash
# INSTALL-0 writer proof: exercise the installer deployment job against a disposable target disk.
#
# This is the non-GUI gate for the dangerous part of INSTALL-0. The Quickshell installer owns the human UI; this proof
# drives the Shrek deployment payload writer directly and verifies that the target disk ends with the
# installed-system layout: sealed base image + appended shrek-layers + shrek-data partitions.
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"; mkdir -p out

PAYLOAD="${PAYLOAD:-out/shrek-install-payload.raw}"
TARGET="${TARGET:-out/install0-target-proof.raw}"
TARGET_SIZE="${TARGET_SIZE:-16G}"
LOG="${LOG:-out/install0-writer-proof.log}"

[ -f "$PAYLOAD" ] || {
  echo "missing PAYLOAD=$PAYLOAD — build it with scripts/build-installer-payload.sh first" >&2
  exit 1
}

[ ! -e "$TARGET" ] || {
  echo "refusing to overwrite existing TARGET=$TARGET; pass TARGET=out/name.raw for a fresh proof disk" >&2
  exit 1
}
truncate -s "$TARGET_SIZE" "$TARGET"
: > "$LOG"

echo "=== INSTALL-0 writer proof: payload=$PAYLOAD target=$TARGET size=$TARGET_SIZE ===" | tee -a "$LOG"
docker run --rm --privileged \
  -v "${REPO_ROOT}:/work" -w /work \
  -e PAYLOAD="$PAYLOAD" -e TARGET="$TARGET" \
  debian:trixie \
  bash -euo pipefail -c '
    apt-get update -qq >/dev/null
    apt-get install -y --no-install-recommends -qq coreutils util-linux mount udev gdisk e2fsprogs >/dev/null

    for i in $(seq 0 31); do
      [ -b "/dev/loop$i" ] || mknod "/dev/loop$i" b 7 "$i"
    done

    payload_loop=$(losetup --find --show "/work/$PAYLOAD")
    target_loop=$(losetup --find --show -P "/work/$TARGET")
    cleanup() {
      umount /run/shrek-payload 2>/dev/null || true
      losetup -d "$target_loop" 2>/dev/null || true
      losetup -d "$payload_loop" 2>/dev/null || true
    }
    trap cleanup EXIT

    mkdir -p /dev/disk/by-label
    ln -sf "$payload_loop" /dev/disk/by-label/shrek-payload

    # ADR-005 §6.5: stage a provisioning manifest as the live installer would, so the writer transplants it
    # onto the target /home. Non-default values (keymap=de) prove it is the transplanted file, not a default.
    mkdir -p /run/shrek/provisioning
    printf "schema_version=1\nkeymap=de\nlocale=C.UTF-8\n" > /run/shrek/provisioning/manifest
    chmod 0600 /run/shrek/provisioning/manifest

    SHREK_INSTALL_ALLOW_LOOP=1 /work/layers/shrek-installer/overlay/usr/libexec/shrek/shrek-install-target \
      --target-disk "$target_loop" \
      --provisioning-manifest /run/shrek/provisioning/manifest

    old_loop="$target_loop"
    losetup -d "$target_loop"
    rm -f "${old_loop}p"*
    target_loop=$(losetup --find --show -P "/work/$TARGET")
    partprobe "$target_loop" 2>/dev/null || true
    partx -u "$target_loop" 2>/dev/null || true
    sleep 1

    layers_part="${target_loop}p7"
    data_part="${target_loop}p8"
    echo "verifier target partition view:"
    lsblk -lnpo NAME,SIZE,TYPE,LABEL,MAJ:MIN "$target_loop" || true
    ensure_node() {
      path="$1"
      [ -b "$path" ] && return 0
      majmin=$(lsblk -nrpo NAME,MAJ:MIN "$target_loop" 2>/dev/null | awk -v p="$path" '\''$1 == p { print $2 }'\'' || true)
      [ -n "$majmin" ] || return 0
      mknod "$path" b "${majmin%:*}" "${majmin#*:}" 2>/dev/null || true
    }
    ensure_node "$layers_part"
    ensure_node "$data_part"
    [ -b "$layers_part" ] || { echo "FAIL missing $layers_part" >&2; exit 2; }
    [ -b "$data_part" ] || { echo "FAIL missing $data_part" >&2; exit 2; }

    layers_label=$(e2label "$layers_part" 2>/dev/null || true)
    data_label=$(e2label "$data_part" 2>/dev/null || true)
    [ "$layers_label" = shrek-layers ] || { echo "FAIL p7 label=$layers_label" >&2; exit 2; }
    [ "$data_label" = shrek-data ] || { echo "FAIL p8 label=$data_label" >&2; exit 2; }

    mkdir -p /mnt/shrek-proof-layers
    mount -o ro "$layers_part" /mnt/shrek-proof-layers
    [ -f /mnt/shrek-proof-layers/extensions/shrek-desktop.raw ] || { echo "FAIL installed store missing shrek-desktop.raw" >&2; exit 2; }
    [ ! -f /mnt/shrek-proof-layers/extensions/shrek-installer.raw ] || { echo "FAIL installed store still contains shrek-installer.raw" >&2; exit 2; }
    umount /mnt/shrek-proof-layers

    # ADR-005 §6.5: the staged manifest must have crossed onto the target /home (p8), root:root 0600, intact.
    mkdir -p /mnt/shrek-proof-data
    mount "$data_part" /mnt/shrek-proof-data
    dm=/mnt/shrek-proof-data/.shrek-system/provisioning/manifest
    [ -f "$dm" ] || { echo "FAIL transplanted provisioning manifest missing on target /home" >&2; exit 2; }
    [ "$(stat -c %a "$dm")" = 600 ] || { echo "FAIL manifest mode=$(stat -c %a "$dm") (want 600)" >&2; exit 2; }
    [ "$(stat -c %u "$dm")" = 0 ]   || { echo "FAIL manifest not root-owned (uid=$(stat -c %u "$dm"))" >&2; exit 2; }
    grep -qx "keymap=de" "$dm" || { echo "FAIL transplanted manifest content missing keymap=de" >&2; exit 2; }
    [ ! -e /mnt/shrek-proof-data/.shrek-system/provisioning/manifest.tmp ] || { echo "FAIL manifest.tmp left on target (non-atomic)" >&2; exit 2; }
    umount /mnt/shrek-proof-data

    echo "PASS target layout contains shrek-layers p7 and shrek-data p8"
    echo "PASS installed layer store omits installer layer"
    echo "PASS provisioning manifest transplanted to target /home (root:root 0600, keymap=de intact)"
  ' 2>&1 | tee -a "$LOG"

echo "=== INSTALL-0 writer proof GREEN: $TARGET ===" | tee -a "$LOG"
