#!/usr/bin/env bash
# coder-audit.sh — a SECOND, decorrelated pass over a draft produced by the local coder tier
# (askcoder / Qwen3-Coder-Next). The coder is smart-but-junior and makes RECURRING classes of
# mistakes; this pass audits specifically for those classes instead of asking "is this good?"
# (a same-model "is this good" pass inherits the same blind spots — mycelium #2073).
#
# The KNOWN-DEFECTS list below is the "brain": the encoded memory of where the junior slips.
# APPEND to it whenever a new recurring class is found — that is what makes the audit compound.
#
# Usage:  scripts/coder-audit.sh <file> [kind-hint]     (kind-hint: mkosi|bash|systemd|config)
#         cat draft | scripts/coder-audit.sh - bash
set -euo pipefail

SRC="${1:?usage: coder-audit.sh <file|-> [kind-hint]}"
KIND="${2:-auto}"
CONTENT="$( [ "$SRC" = "-" ] && cat - || cat "$SRC" )"

read -r -d '' DEFECTS <<'EOF' || true
Audit the draft ONLY for these recurring defect classes. For EACH class output exactly one line:
"<CLASS>: PASS" or "<CLASS>: DEFECT L<line> — <what> → <fix>". Do not rewrite the file. Be terse.

1. CONFIG-FIELD FORMAT: a config value invented in the wrong shape (e.g. an /etc/shadow field-mash
   in mkosi RootPassword; a bool given as 'off' where the tool wants 'no'; wrong section for a key).
2. UNVERIFIED EXTERNAL NAMES: package names, config keys, CLI subcommands/flags asserted as real
   without evidence (e.g. a systemd sub-binary treated as its own apt package). Each such token must
   either be known-correct or carry a # VERIFY. Flag any bare assertion.
3. SHELL FRAGILITY: bind-mount host paths used before `mkdir -p`; nested ro-over-rw mounts; output
   dirs docker would create root-owned; missing `set -euo pipefail`; unquoted expansions.
4. MISSING SETUP / ORDERING (OMISSIONS — the hardest): a step the draft NEEDS but left out
   (dir not created, ownership not fixed, a produced file never consumed, wrong operation order).
   Look for what is ABSENT, not only what is wrong.
5. SILENT-FAILURE: a command whose failure is swallowed; a claim the script does X but no line does.
EOF

PROMPT="You are a strict reviewer auditing a draft (kind: ${KIND}) written by a junior coding model.
${DEFECTS}

--- DRAFT ---
${CONTENT}
--- END DRAFT ---

Output only the 5 audit lines, most severe first. End with 'VERDICT: CLEAN' or 'VERDICT: FIX'."

echo ">>> coder-audit on ${SRC} (kind=${KIND})"
askcoder -t 1024 "$PROMPT"
