#!/usr/bin/env bash
# Phase-6 Swamp slice-1 — authority-filtered indexing + `shrek find`. End-to-end negative proof in a
# privileged debian:trixie oracle (the fast path before any VM confirm; swampd is availability-plane
# §10, off the sealed boot path). Proves the security-model §5 invariant EXECUTABLE, two ways:
#
#   CONFINEMENT (swamp.md §5): `swampd confine-probe` enforces the REAL Landlock allow-set, then an
#     open() of ~/Vault fails at the KERNEL boundary while an allow-set member opens fine.
#   QUERY GATE (swamp.md §9): a `shrek find` scoped to session A's grant (project app-a) returns ONLY
#     app-a objects — app-b (indexed, out of scope) and Vault (never indexed) are ABSENT and their
#     FTS tokens undiscoverable; a scope-widen attempt narrows to nothing; an unknown session sees
#     nothing. Authority is resolved from the root-owned session record, never the request.
#
# Usage: scripts/swamp-find-proof.sh
set -uo pipefail

if [ "${IN_CONTAINER:-0}" != "1" ]; then
  REPO_ROOT="$(git rev-parse --show-toplevel)"
  echo "=== building release swampd + gatekeeperd + shrek (host; bundled sqlite via cc) ==="
  ( cd "$REPO_ROOT" && cargo build --offline --release -p swampd -p gatekeeperd -p shrek ) || exit 3
  echo "=== launching privileged debian:trixie oracle ==="
  exec docker run --rm --privileged -e IN_CONTAINER=1 \
    -v "$REPO_ROOT/scripts/swamp-find-proof.sh:/proof.sh:ro" \
    -v "$REPO_ROOT/target/release/swampd:/swampd:ro" \
    -v "$REPO_ROOT/target/release/gatekeeperd:/gatekeeperd:ro" \
    -v "$REPO_ROOT/target/release/shrek:/shrek:ro" \
    debian:trixie bash /proof.sh
fi

# ---------------- inside the container ----------------
export DEBIAN_FRONTEND=noninteractive
mount -t tmpfs tmpfs /run 2>/dev/null || true

PASS=0; FAILN=0
pass() { echo "SHREK_GATE: PASS $1"; PASS=$((PASS+1)); }
fail() { echo "SHREK_GATE: FAIL $1"; FAILN=$((FAILN+1)); }
# assert a needle IS present in a value
has()    { printf '%s' "$2" | grep -q -- "$3" && pass "$1" || fail "$1 (missing '$3' in: $(printf '%s' "$2" | tr '\n' '|'))"; }
# assert a needle is ABSENT from a value
absent() { printf '%s' "$2" | grep -q -- "$3" && fail "$1 (LEAKED '$3' in: $(printf '%s' "$2" | tr '\n' '|'))" || pass "$1"; }
# assert a value has no output lines at all
empty()  { [ -z "$(printf '%s' "$2" | tr -d '[:space:]')" ] && pass "$1" || fail "$1 (expected empty, got: $(printf '%s' "$2" | tr '\n' '|'))"; }

# --- users: swamp (the daemon), tester (owns data + runs shrek find) ---
groupadd swamp 2>/dev/null || true
useradd -g swamp -M -s /usr/sbin/nologin swamp 2>/dev/null || true
useradd -m -s /bin/bash tester 2>/dev/null || true
TESTER_UID=$(id -u tester)

# --- seed the tree: two indexable projects + a Vault. Everything world-READABLE on purpose, so DAC
#     is NOT the thing denying Vault — the Landlock wall is. Unique tokens per tree isolate the FTS proof.
H=/home/tester
mkdir -p "$H/Projects/app-a/src" "$H/Projects/app-b/src" "$H/Vault" "$H/Documents"
echo 'fn main() { println!("isolation tiers"); }'      > "$H/Projects/app-a/src/main.rs"
echo 'app-a isolation overview AAONLY'                 > "$H/Projects/app-a/README.md"
echo 'isolation secret sauce in app-b token BBSECRET'  > "$H/Projects/app-b/src/lib.rs"
echo 'app-b readme isolation'                          > "$H/Projects/app-b/README.md"
echo 'isolation master password VAULTSECRET hunter2'   > "$H/Vault/passwords.txt"
chown -R tester:tester "$H"
chmod -R a+rX "$H"

# --- runtime dirs ---
mkdir -p /run/swamp && chown swamp:swamp /run/swamp && chmod 755 /run/swamp
mkdir -p /run/shrek/authority && chown root:swamp /run/shrek/authority && chmod 750 /run/shrek/authority

export SWAMP_HOME=$H
export SWAMP_STATE_DIR=/run/swamp
export SWAMP_AUTHORITY_DIR=/run/shrek/authority

# --- authority records (privileged writer = gatekeeperd, owns root:swamp) ---
/gatekeeperd authority-record --session sessA --grant "$H/Projects/app-a" --dir /run/shrek/authority || fail "record-write-A"
/gatekeeperd authority-record --session sessB --grant "$H/Projects/app-b" --dir /run/shrek/authority || fail "record-write-B"
# forge check: the untrusted 'tester' uid must NOT be able to read or write the record dir/files.
if runuser -u tester -- cat /run/shrek/authority/sessA >/dev/null 2>&1; then
  fail "gate=authority-record-unforgeable (tester read a root:swamp 0640 record)"
else
  pass "gate=authority-record-unreadable-by-workload"
fi

echo "=== CONFINEMENT: swampd confine-probe (enforces the real Landlock allow-set) ==="
PROBE_OUT=$(runuser -u swamp -- env SWAMP_HOME=$H SWAMP_STATE_DIR=/run/swamp SWAMP_AUTHORITY_DIR=/run/shrek/authority \
  /swampd confine-probe "$H/Vault/passwords.txt" "$H/Projects/app-a/README.md" "$H/Documents" "$H" /etc/passwd 2>/dev/null)
echo "$PROBE_OUT"
if ! printf '%s' "$PROBE_OUT" | grep -q '^PROBE '; then
  fail "gate=landlock-available (no PROBE output — Landlock unavailable/disabled in this kernel?)"
else
  # Vault: DAC would allow (world-readable), Landlock must DENY at the kernel boundary.
  printf '%s\n' "$PROBE_OUT" | grep -q "^PROBE $H/Vault/passwords.txt DENIED" \
    && pass "gate=confine-vault-open-denied-at-kernel" || fail "gate=confine-vault-open-denied-at-kernel"
  # home root itself is not a member ⇒ denied.
  printf '%s\n' "$PROBE_OUT" | grep -q "^PROBE $H DENIED" \
    && pass "gate=confine-home-root-denied" || fail "gate=confine-home-root-denied"
  # allow-set member + system dir: OK.
  printf '%s\n' "$PROBE_OUT" | grep -q "^PROBE $H/Projects/app-a/README.md OK" \
    && pass "gate=confine-member-open-ok" || fail "gate=confine-member-open-ok"
  printf '%s\n' "$PROBE_OUT" | grep -q "^PROBE $H/Documents OK" \
    && pass "gate=confine-member-dir-ok" || fail "gate=confine-member-dir-ok"
  printf '%s\n' "$PROBE_OUT" | grep -q "^PROBE /etc/passwd OK" \
    && pass "gate=confine-system-ok" || fail "gate=confine-system-ok"
fi

echo "=== serve: start swampd (as swamp), crawl + query socket ==="
runuser -u swamp -- env SWAMP_HOME=$H SWAMP_STATE_DIR=/run/swamp SWAMP_AUTHORITY_DIR=/run/shrek/authority \
  SWAMP_ALLOW_UID=$TESTER_UID /swampd serve >/tmp/swampd.log 2>&1 &
SWAMPD_PID=$!
# wait (bounded) for the socket — never a bare sleep.
for _ in $(seq 1 100); do [ -S /run/swamp/query.sock ] && break; sleep 0.1; done
if [ ! -S /run/swamp/query.sock ]; then
  fail "gate=swampd-serving (socket never appeared)"; echo "--- swampd.log ---"; cat /tmp/swampd.log
else
  pass "gate=swampd-serving"
  grep -q 'initial map objects=' /tmp/swampd.log && echo "  $(grep 'initial map' /tmp/swampd.log)"

  # tester runs shrek find; stderr (the summary) discarded — stdout is hit paths only.
  q() { runuser -u tester -- env SWAMP_QUERY_SOCK=/run/swamp/query.sock /shrek find "$@" 2>/dev/null; }

  echo "=== QUERY GATE: session A (granted app-a only) ==="
  A=$(q --session sessA isolation)
  has    "gate=A-sees-own-project"        "$A" "/Projects/app-a/"
  absent "gate=A-cannot-see-sibling-appb" "$A" "app-b"
  absent "gate=A-cannot-see-vault"        "$A" "Vault"

  echo "=== QUERY GATE: session B (granted app-b only) ==="
  B=$(q --session sessB isolation)
  has    "gate=B-sees-own-project"        "$B" "/Projects/app-b/"
  absent "gate=B-cannot-see-sibling-appa" "$B" "app-a"

  echo "=== FTS token isolation: unauthorized tokens never discoverable ==="
  empty "gate=A-cannot-fts-appb-token-out-of-scope" "$(q --session sessA BBSECRET)"
  empty "gate=A-cannot-fts-vault-token-never-indexed" "$(q --session sessA VAULTSECRET)"
  empty "gate=A-cannot-fts-vault-password" "$(q --session sessA hunter2)"

  echo "=== scope selector narrows only; unknown session sees nothing ==="
  empty "gate=scope-cannot-widen-to-appb" "$(q --session sessA --scope $H/Projects/app-b isolation)"
  # narrowing WITHIN the grant still works (app-a/src only)
  NARROW=$(q --session sessA --scope $H/Projects/app-a/src isolation)
  has    "gate=scope-narrows-within-grant" "$NARROW" "/app-a/src/"
  empty "gate=unknown-session-sees-nothing" "$(q --session no-such-session isolation)"

  echo "=== discover (path/name) intent is scoped identically ==="
  DISC=$(q --session sessA --intent discover README)
  has    "gate=discover-sees-own"          "$DISC" "/app-a/"
  absent "gate=discover-cannot-see-appb"   "$DISC" "app-b"

  kill "$SWAMPD_PID" 2>/dev/null
fi

echo
echo "  totals: PASS=$PASS FAIL=$FAILN"
if [ "$FAILN" = "0" ] && [ "$PASS" -ge 18 ]; then
  echo "SHREK_GATE: PASS swamp-find-slice1 ($PASS gates)"
  exit 0
else
  echo "SHREK_GATE: FAIL swamp-find-slice1 (pass=$PASS fail=$FAILN)"
  exit 1
fi
