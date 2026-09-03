#!/usr/bin/env bash
# Stage the Quickshell graphical installer (ui-installer/) into the shrek-installer overlay before the
# sysext build (must-fix #4, slice 3a). ui-installer/ is the single source of truth — the same tree the
# render harness (scripts/installer-preview.sh) loads and the CI proofs reference — so we COPY it into the
# overlay at build time rather than committing a duplicate. The staged path is gitignored (build input,
# not source), mirroring scripts/stage-refind.sh.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

SRC="ui-installer"
DEST="layers/shrek-installer/overlay/usr/share/shrek/installer/ui"

[ -f "$SRC/shell.qml" ] || { echo "missing $SRC/shell.qml — nothing to stage" >&2; exit 1; }

rm -rf "$DEST"
mkdir -p "$DEST"
# -a preserves the tree layout so shell.qml's relative imports (state/, theme/, ui/) and assets resolve
# identically to the source checkout the preview validates.
cp -a "$SRC"/. "$DEST"/

echo "staged $SRC -> $DEST ($(find "$DEST" -type f | wc -l) files)"
