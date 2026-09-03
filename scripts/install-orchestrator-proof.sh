#!/bin/sh
set -eu
# Fast host proof for must-fix #4 — the shrek-install-run orchestrator's progress protocol.
#
# Exercises the protocol translation WITHOUT a real disk or root: SHREK_INSTALL_TARGET points at a stub
# writer that emits @PHASE markers (and prose) and exits 0 or non-zero on demand. Asserts that
# shrek-install-run re-emits the canonical SHREK-INSTALL frames in order, keeps writer prose out of its
# stdout, reports FAIL for the in-flight phase on a crash, and mirrors the writer's exit code.

here=$(CDPATH= cd "$(dirname "$0")/.." && pwd)
RUN="$here/layers/shrek-installer/overlay/usr/libexec/shrek/shrek-install-run"
TARGET_REAL="$here/layers/shrek-installer/overlay/usr/libexec/shrek/shrek-install-target"

work=$(mktemp -d "${TMPDIR:-/tmp}/orch-proof.XXXXXX")
trap 'rm -rf "$work"' EXIT
LOG="$work/run.log"

PASS=0; FAIL=0
ok()   { PASS=$((PASS+1)); echo "  PASS $1"; }
bad()  { FAIL=$((FAIL+1)); echo "  FAIL $1"; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (want [$3] got [$2])"; fi; }

# --- stub writer: emits the four phases, honours a fault point ---------------------------------------
make_stub() {
    # $1 = phase to die AFTER emitting its `begin` (empty = never die / full success)
    cat >"$work/stub" <<STUB
#!/bin/sh
# ignores --target-disk/--provisioning-manifest; emits markers + prose like the real writer
die_after="$1"
for ph in verify write layout firstboot; do
    echo "prose: entering \$ph"        # non-frame chatter — must NOT reach run's stdout
    printf '@PHASE %s begin\n' "\$ph"
    if [ "\$ph" = "\$die_after" ]; then echo "boom" >&2; exit 7; fi
    printf '@PHASE %s done\n' "\$ph"
done
echo "INSTALL-0 complete"
STUB
    chmod +x "$work/stub"
}

echo "=== 1. full success: canonical frames in order, ends DONE, exit 0 ==="
make_stub ""
set +e
SHREK_INSTALL_TARGET="$work/stub" SHREK_INSTALL_RUN_LOG="$LOG" \
    "$RUN" --target-disk /dev/fake --provisioning-manifest "$work/manifest" >"$work/out" 2>"$work/err"
rc=$?
set -e
check "success exit code is 0" "$rc" "0"
cat >"$work/expected" <<EOF
SHREK-INSTALL BEGIN target=/dev/fake
SHREK-INSTALL STEP verify begin
SHREK-INSTALL STEP verify done
SHREK-INSTALL STEP write begin
SHREK-INSTALL STEP write done
SHREK-INSTALL STEP layout begin
SHREK-INSTALL STEP layout done
SHREK-INSTALL STEP firstboot begin
SHREK-INSTALL STEP firstboot done
SHREK-INSTALL DONE
EOF
if diff -u "$work/expected" "$work/out" >"$work/diff" 2>&1; then
    ok "stdout is exactly the canonical frame sequence"
else
    bad "stdout frame sequence mismatch"; sed 's/^/    /' "$work/diff"
fi
if grep -q '^prose:' "$work/out" || grep -q 'INSTALL-0 complete' "$work/out"; then
    bad "writer prose leaked into run stdout"
else
    ok "writer prose stayed out of run stdout"
fi
if grep -q 'prose: entering verify' "$LOG"; then ok "writer prose captured in the log"; else bad "writer prose missing from log"; fi

echo "=== 2. writer dies mid-write: FAIL names the in-flight phase, no DONE, exit mirrors writer ==="
make_stub "write"
set +e
SHREK_INSTALL_TARGET="$work/stub" SHREK_INSTALL_RUN_LOG="$LOG" \
    "$RUN" --target-disk /dev/fake >"$work/out2" 2>"$work/err2"
rc=$?
set -e
check "failure exit code mirrors writer (7)" "$rc" "7"
grep -q '^SHREK-INSTALL STEP write begin$' "$work/out2" && ok "emitted STEP write begin before the crash" || bad "missing STEP write begin"
grep -q '^SHREK-INSTALL STEP write done$'  "$work/out2" && bad "wrongly emitted STEP write done after a crash" || ok "did not emit a done for the crashed phase"
grep -q '^SHREK-INSTALL FAIL write ' "$work/out2" && ok "emitted FAIL naming the in-flight phase (write)" || bad "missing/incorrect FAIL frame"
grep -q '^SHREK-INSTALL DONE$' "$work/out2" && bad "emitted DONE despite failure" || ok "no DONE frame on failure"

echo "=== 3. arg validation ==="
set +e
SHREK_INSTALL_TARGET="$work/stub" "$RUN" >/dev/null 2>&1; rc=$?
set -e
check "missing --target-disk exits 64" "$rc" "64"
set +e
SHREK_INSTALL_TARGET="$work/nonexistent" "$RUN" --target-disk /dev/fake >/dev/null 2>&1; rc=$?
set -e
check "missing writer binary exits 69" "$rc" "69"

echo "=== 4. shellcheck-lite: both scripts parse ==="
sh -n "$RUN" && ok "shrek-install-run parses" || bad "shrek-install-run sh -n failed"
sh -n "$TARGET_REAL" && ok "shrek-install-target parses" || bad "shrek-install-target sh -n failed"
grep -q 'emit_phase verify begin'    "$TARGET_REAL" && \
grep -q 'emit_phase firstboot done'  "$TARGET_REAL" && ok "writer carries the phase markers" || bad "writer missing phase markers"

echo
echo "=== install-orchestrator-proof: PASS=$PASS FAIL=$FAIL ==="
[ "$FAIL" -eq 0 ]
