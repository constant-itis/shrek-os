#!/usr/bin/env bash
# Dogfood-0 (M1) — create the writable data disk that carries persistent /home (docs/dogfood-0.md).
#
# The sealed image keeps /usr read-only (dm-verity) and /var volatile (systemd.volatile=state); the
# ONLY durable state is /home, which lives here — a dedicated ext4 filesystem (label `shrek-data`) that
# the baked home.mount unit mounts by label. This disk is NEVER part of the sealed image and survives
# both reboots and A/B image updates.
#
# Usage:
#   scripts/dogfood-data-disk.sh [PATH] [SIZE]      create if absent (the DAILY persistent disk)
#   FRESH=1 scripts/dogfood-data-disk.sh [PATH]     always recreate (the disposable ORACLE disk)
#
# mkfs.ext4 on a plain file needs no root and never mounts anything, so the host stays untouched. The
# file is sparse (truncate), so a 16G disk costs only the bytes actually written.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"; mkdir -p out

DISK="${1:-out/shrek-data.raw}"
SIZE="${2:-16G}"
LABEL="shrek-data"

if [ "${FRESH:-0}" = "1" ]; then
  rm -f "$DISK"
  echo "FRESH=1 → recreating disposable data disk $DISK"
elif [ -f "$DISK" ]; then
  echo "keeping existing persistent data disk: $DISK ($(du -h "$DISK" | cut -f1) on disk)"
  exit 0
fi

command -v mkfs.ext4 >/dev/null || { echo "mkfs.ext4 not found — install e2fsprogs" >&2; exit 1; }
truncate -s "$SIZE" "$DISK"
# -F: operate on a regular file; -q: quiet. Label lets home.mount find it as /dev/disk/by-label/shrek-data.
mkfs.ext4 -F -q -L "$LABEL" "$DISK"
echo "created $DISK — ext4 label=$LABEL size=$SIZE (sparse: $(du -h "$DISK" | cut -f1) used)"
