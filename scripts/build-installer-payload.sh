#!/usr/bin/env bash
# Build a read-only payload disk for the live installer VM.
#
# The live installer runs from the normal Shrek base + shrek-installer sysext. This payload disk carries
# the exact sealed install image plus the installed-system layer store that must be copied to the target
# disk. Do not use the live installer store here; installed systems should not carry the installer layer.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

BASE="${BASE:-out/shrek_1_x86-64.raw}"
LAYERS="${LAYERS:-out/layer-store-desktop.raw}"
OUT="${OUT:-out/shrek-install-payload.raw}"
[ -f "$BASE" ] || { echo "missing BASE=$BASE" >&2; exit 1; }
[ -f "$LAYERS" ] || { echo "missing LAYERS=$LAYERS" >&2; exit 1; }

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
