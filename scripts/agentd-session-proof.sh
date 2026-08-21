#!/usr/bin/env bash
# Shrek OS Phase-8 slice-1 — HOST oracle for the Agent Session (docs/phase8-slice1-agent-session.md).
# Proves the decision → effective-authority-view → read-only-status flow + the confirmation pass
# (C1–C3) deterministically, WITHOUT privilege or a VM (env overrides SHREK_SESSION_DIR /
# SHREK_GATEKEEPERD, mirroring the authority-record / net-binding host oracles). The REAL T2
# construction that writes the view inline is proven in the sealed-VM gate (Pn-agentd-session); this
# oracle proves the logic — agentd's decision, the record writer/reader, and C1–C3 — fast.
#
#   G-refuse   caps ⊄ profile ⇒ agentd refuses (exit 11), NOTHING constructed
#   G-argv     agentd owns the tier + attaches the subject; grants/egress pass through to gatekeeperd
#   G-view     the view renders effective.* == the written decision (C2: matches construction, not argv)
#   G-mode     the record is 0640 (C1 structural basis: a non-owner cannot write/forge it)
#   G-teardown --rm removes the record ⇒ `shrek session status` fails closed (C3)
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

echo "=== building agentd + gatekeeperd + shrek (release) ==="
CARGO_NET_OFFLINE=true cargo build --release -p agentd -p gatekeeperd -p shrek >/dev/null 2>&1 || \
  CARGO_NET_OFFLINE=true cargo build --release -p agentd -p gatekeeperd -p shrek
AGENTD=target/release/agentd
GK=target/release/gatekeeperd
SHREK=target/release/shrek

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
SDIR="$WORK/session"; mkdir -p "$SDIR"
PROJ="$WORK/proj"; mkdir -p "$PROJ"    # a real dir so the view's canonical grant resolves

PASS=0; FAIL=0
gate() { if [ "$1" = ok ]; then echo "SHREK_GATE: PASS $2"; PASS=$((PASS+1)); else echo "SHREK_GATE: FAIL $2 — $3"; FAIL=$((FAIL+1)); fi; }

# --- G-refuse: caps exceed the granted profile ⇒ agentd refuses before anything is constructed ---
set +e
OUT="$("$AGENTD" session --trust T-untrust --caps C-net --profile C-proj-rw --subject dev1 \
        --anchor "$WORK" --rw-grant proj -- coder 2>&1)"; RC=$?
set -e
if [ "$RC" = 11 ] && echo "$OUT" | grep -q "caps-exceed-profile"; then gate ok G-refuse
else gate no G-refuse "rc=$RC out=$OUT"; fi

# --- G-argv: agentd computes the tier (T-untrust,C-net ⇒ T2), attaches the subject, and passes the
#     grants/egress through. A stub gatekeeperd captures the argv agentd would exec. ---
STUB="$WORK/gk-stub.sh"
cat > "$STUB" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" > "$GK_ARGV_OUT"
EOF
chmod +x "$STUB"
GK_ARGV_OUT="$WORK/argv.txt" SHREK_GATEKEEPERD="$STUB" \
  "$AGENTD" session --trust T-untrust --caps C-net --profile C-net --subject dev1 \
    --anchor "$WORK" --rw-grant proj --egress-profile model-anthropic -- coder --provider anthropic >/dev/null 2>&1 || true
ARGV="$(cat "$WORK/argv.txt" 2>/dev/null || echo)"
if echo "$ARGV" | grep -q "sandbox --tier T2 " \
   && echo "$ARGV" | grep -q -- "--subject dev1" \
   && echo "$ARGV" | grep -q -- "--egress-profile model-anthropic" \
   && echo "$ARGV" | grep -q -- "--rw-grant proj" \
   && echo "$ARGV" | grep -q -- "-- coder --provider anthropic"; then gate ok G-argv
else gate no G-argv "argv=$ARGV"; fi

# --- G-view + G-mode: write the effective-authority record (the projection gatekeeperd writes inline
#     at construct) and read it back through `shrek session status`; effective.* must match the written
#     decision, and the record must be 0640 (the non-forgeable/non-wideable structural basis). ---
"$GK" session-view --dir "$SDIR" --session s0 --subject dev1 \
  --tier T2 --trust T-untrust --caps cnet --profile cnet --grant "$PROJ" \
  --egress-profile model-anthropic --egress-dst shrek-model-proxy:8200 \
  --workload-arg coder --workload-arg --provider --workload-arg anthropic \
  --provider anthropic --mode deterministic --semantic-available --semantic-tier fts+semantic >/dev/null

STATUS="$(SHREK_SESSION_DIR="$SDIR" "$SHREK" session status s0)"
CANON="$(readlink -f "$PROJ")"
if echo "$STATUS" | grep -q "tier=T2" \
   && echo "$STATUS" | grep -q "subject=dev1" \
   && echo "$STATUS" | grep -q "egress=model-anthropic -> shrek-model-proxy:8200" \
   && echo "$STATUS" | grep -q "model=anthropic/deterministic" \
   && echo "$STATUS" | grep -qF "$CANON"; then gate ok G-view
else gate no G-view "status=$STATUS"; fi

MODE="$(stat -c '%a' "$SDIR/s0.json")"
if [ "$MODE" = 640 ]; then gate ok G-mode; else gate no G-mode "mode=$MODE (want 640)"; fi

# --- G-teardown (C3): remove the record ⇒ status fails closed (no residual, no such session) ---
"$GK" session-view --dir "$SDIR" --session s0 --rm >/dev/null
set +e
TD="$(SHREK_SESSION_DIR="$SDIR" "$SHREK" session status s0 2>&1)"; TRC=$?
set -e
if [ ! -e "$SDIR/s0.json" ] && [ "$TRC" = 4 ] && echo "$TD" | grep -q "no such session"; then gate ok G-teardown
else gate no G-teardown "rc=$TRC out=$TD residual=$( [ -e "$SDIR/s0.json" ] && echo yes || echo no )"; fi

echo "=================== agentd-session oracle ==================="
echo "PASS=$PASS FAIL=$FAIL"
[ "$FAIL" = 0 ]
