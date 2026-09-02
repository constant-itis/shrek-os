#!/usr/bin/env bash
# shrek-ai-noexec-check.sh — the ADR-006 §6 structural gate: assert a source tree carries NO host-exec /
# process-spawn primitive. Run over the HARDENED mycolink-shell snapshot at build time; a non-empty match
# FAILS the build. This is the "the shipped shell cannot spawn a host subprocess" invariant, mechanized.
#
#   usage: shrek-ai-noexec-check.sh <tree-dir>
set -euo pipefail
TREE="${1:?usage: shrek-ai-noexec-check.sh <tree-dir>}"
# Actual host-exec / process-spawn CODE sites that must be ABSENT from the sealed artifact — import
# statements and call sites, not prose. (A comment mentioning "subprocess" is not an exec surface.)
PAT='^[[:space:]]*(import|from)[[:space:]]+(subprocess|pty)\b|subprocess\.(run|Popen|call|check_output|check_call|getoutput|getstatusoutput)|os\.(system|popen|exec[lv]e?p?|spawn[lv])|\bpty\.(spawn|fork|openpty)|shutil\.which[[:space:]]*\('
hits="$(grep -rnE "$PAT" "$TREE" --include='*.py' 2>/dev/null || true)"
if [ -n "$hits" ]; then
  echo "SHREK NO-EXEC GATE: FAIL — host-exec/process-spawn primitives present in the shipped tree:" >&2
  echo "$hits" >&2
  exit 1
fi
echo "SHREK NO-EXEC GATE: PASS — no host-exec/process-spawn primitive in $TREE"
