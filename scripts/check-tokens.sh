#!/usr/bin/env bash
# Enforce the semantic-token contract for the Shrek shell.
#
# Every surface must express colour ONLY through the Tokens.* role names. It may NOT:
#   (1) hard-code a raw colour hex literal, or
#   (2) reach a palette SOURCE directly (Theme / Colours / Palettes).
# Only ui/themes/** is allowed to hold raw colour or touch the sources. This keeps all five theme modes
# flowing through one chokepoint (Tokens) so an individual component can never bypass a mode switch.
#
# Runs host-side in milliseconds; wired into scripts/qml-check.sh as a fail-fast pre-step. Exit 1 on any
# violation (prints file:line), 0 when clean.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"
SCOPE=(ui/surfaces ui/shell)
fail=0

# (1) raw colour hex literals inside quotes: "#RGB" "#RGBA" "#RRGGBB" "#AARRGGBB"
hexhits="$(grep -rnE "[\"']#([0-9a-fA-F]{8}|[0-9a-fA-F]{6}|[0-9a-fA-F]{3,4})[\"']" "${SCOPE[@]}" --include='*.qml' || true)"
if [ -n "$hexhits" ]; then
  fail=1
  echo "CHECK-TOKENS: raw colour hex in surfaces/shell (use a Tokens.* role instead):"
  echo "$hexhits" | sed 's/^/  /'
fi

# (2) direct references to a palette source — surfaces must go through Tokens, never Theme/Colours/Palettes
srchits="$(grep -rnE "\b(Theme|Colours|Palettes)\." "${SCOPE[@]}" --include='*.qml' || true)"
if [ -n "$srchits" ]; then
  fail=1
  echo "CHECK-TOKENS: direct palette-source reference in surfaces/shell (use a Tokens.* role instead):"
  echo "$srchits" | sed 's/^/  /'
fi

if [ "$fail" = 0 ]; then
  echo "CHECK-TOKENS: PASS (no semantic-token bypass in surfaces/shell)"
else
  echo "CHECK-TOKENS: FAIL"
  exit 1
fi
