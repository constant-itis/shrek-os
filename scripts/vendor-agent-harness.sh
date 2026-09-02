#!/usr/bin/env bash
# vendor-agent-harness.sh — reproduce the ShrekOS AI front-door shell and stage it into the shrek-ai Onion
# overlay (ADR-006 §3, docs/adr-006-slice5-front-door.md). Run on the HOST by scripts/build-ai-layer.sh
# before mkosi, so the populated (git-ignored) overlay path is picked up by ExtraTrees=overlay.
#
#   pinned source (layers/shrek-ai/agent-harness.pin)
#     -> harden   (scripts/harden-mycolink-shell.py: REMOVE host-exec/escalation/dispatch/process-spawn)
#     -> adapt    (scripts/shrek-shell-adapters.py: ADD Shrek Memory API recall + file-backed system prompt)
#     -> gate     (scripts/shrek-ai-noexec-check.sh: FAIL if any exec primitive survives)
#     -> digest   (verify the hardened tree matches the pinned HARDENED_TREE_SHA256)
#     -> stage    (into layers/shrek-ai/overlay/usr/lib/shrek/ai/mycolink-shell/)
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

PIN="layers/shrek-ai/agent-harness.pin"
SRC_REPO_PATH="${AGENT_HARNESS_SRC:-$HOME/projects/agent-harness}"
DEST="layers/shrek-ai/overlay/usr/lib/shrek/ai/mycolink-shell/agent_harness"

SOURCE_COMMIT="$(grep '^SOURCE_COMMIT=' "$PIN" | cut -d= -f2)"
WANT_DIGEST="$(grep '^HARDENED_TREE_SHA256=' "$PIN" | cut -d= -f2)"
[ -n "$SOURCE_COMMIT" ] && [ -n "$WANT_DIGEST" ] || { echo "vendor: bad pin file $PIN" >&2; exit 1; }
[ -d "$SRC_REPO_PATH/.git" ] || { echo "vendor: agent-harness checkout not at $SRC_REPO_PATH (set AGENT_HARNESS_SRC)" >&2; exit 1; }

echo "=== vendoring mycolink-shell @ ${SOURCE_COMMIT} ==="
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
# Export the PINNED commit's agent_harness/ (not HEAD) — a sealed image cannot float on the sibling's HEAD.
git -C "$SRC_REPO_PATH" archive "$SOURCE_COMMIT" agent_harness | tar -x -C "$TMP"

rm -rf "$DEST"; mkdir -p "$(dirname "$DEST")"
python3 scripts/harden-mycolink-shell.py "$TMP/agent_harness" "$DEST"
python3 scripts/shrek-shell-adapters.py "$DEST"

# Structural §6 gate: no host-exec/process-spawn may survive in the shipped tree.
scripts/shrek-ai-noexec-check.sh "$DEST"

# Reproducibility: the hardened tree must match the pinned digest exactly.
GOT="$(cd "$DEST" && find . -type f -name '*.py' | LC_ALL=C sort | while read -r f; do
  printf '%s:%s\n' "$f" "$(sha256sum "$f" | cut -d' ' -f1)"; done | sha256sum | cut -d' ' -f1)"
if [ "$GOT" != "$WANT_DIGEST" ]; then
  echo "vendor: HARDENED TREE DIGEST MISMATCH" >&2
  echo "  got  $GOT" >&2
  echo "  want $WANT_DIGEST  (update layers/shrek-ai/agent-harness.pin if the patchset changed on purpose)" >&2
  exit 1
fi
echo "=== staged hardened mycolink-shell -> $DEST (digest verified: $GOT) ==="
