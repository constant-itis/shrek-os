#!/usr/bin/env bash
# ADR-008 S4 — gatekeeperd resolves privileged egress pins OFF `resolved` (Option 4), host oracle.
#
# The #3121 principle applied to gatekeeperd's privileged pin path: don't harden the owner-controlled
# resolver, stop USING it as a security oracle. gatekeeperd no longer calls getaddrinfo (files+resolved)
# for egress-profile pins. Instead, per host CLASS (disjoint):
#   * a sealed ALIAS (the 4 owner-bindable model brokers + swamp) resolves ONLY from the root-owned
#     /run/shrek/hosts projection; unbound => fail-closed;
#   * a public DNS name resolves ONLY over the shared sealed-DoT client — the hosts file is NEVER
#     consulted, so a poisoned hosts entry (the analog of a uid-1000 NM ipv4.dns / resolved steer) can
#     NEVER steer a public pin.
#
# This drives the oracle-env `gatekeeperd resolve-egress` verb (compiled OUT of production) through the
# EXACT files-then-DoT path the T2 constructor uses, against redirected paths (SHREK_HOSTS_PROJECTION for
# the root hosts file, SHREK_DOT_CA for the sealed trust base). No root, no nft, no VM.
#
# Gates:
#   GK-alias-files    a bound alias resolves to its projection IP (files path, no network)
#   GK-unbound-alias  an unbound alias fails closed ("no brain connected") — never leaks to a resolver
#   GK-poison-ignored a POISONED hosts entry for a public name (github.com -> 6.6.6.6) is IGNORED: the
#                     public pin is resolved over sealed DoT, never the hosts file  ← the money gate
#   GK-live-dot       (network-gated) the public name actually resolves to a real IP over sealed DoT
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

BASE="out/gatekeeperd-egress-resolve-s4-proof"
rm -rf "$BASE"; mkdir -p "$BASE"
ABS="$(readlink -f "$BASE")"

PASS=0; FAIL=0; WARN=0
gate() { if [ "$1" = ok ]; then echo "SHREK_GATE: PASS $2"; PASS=$((PASS+1)); elif [ "$1" = warn ]; then echo "SHREK_GATE: WARN $2"; WARN=$((WARN+1)); else echo "SHREK_GATE: FAIL $2"; FAIL=$((FAIL+1)); fi; }

echo "=== building gatekeeperd (oracle-env) ==="
CARGO_NET_OFFLINE=true cargo build -p gatekeeperd --features oracle-env >/dev/null 2>&1 || \
  cargo build -p gatekeeperd --features oracle-env
BIN="target/debug/gatekeeperd"

export SHREK_DOT_CA="$(readlink -f image/overlay/usr/lib/shrek/dot-ca-roots.pem)"
PROJ="$ABS/hosts"
export SHREK_HOSTS_PROJECTION="$PROJ"

echo "--- GK-alias-files (a bound alias resolves from the root hosts projection, no network) ---"
printf '127.0.0.1 localhost\n::1 localhost\n192.168.1.152 shrek-model\n' > "$PROJ"
OUT="$("$BIN" resolve-egress model-local 2>&1 || true)"
echo "  $OUT"
if echo "$OUT" | grep -qx 'host shrek-model 192.168.1.152'; then gate ok GK-alias-files; else gate no GK-alias-files; fi

echo "--- GK-unbound-alias (alias absent from the projection fails closed, no DoT leak) ---"
printf '127.0.0.1 localhost\n' > "$PROJ"
UOUT="$("$BIN" resolve-egress model-local 2>&1 || true)"
echo "  $UOUT"
if echo "$UOUT" | grep -qi 'unbound' && ! echo "$UOUT" | grep -q '^host shrek-model'; then gate ok GK-unbound-alias; else gate no GK-unbound-alias; fi

echo "--- GK-poison-ignored (a poisoned hosts entry for a public name is IGNORED; DoT is the path) ---"
# 6.6.6.6 is the attacker IP a uid-1000 NM/resolved steer would inject. gatekeeperd must NEVER pin it for
# github.com (a public name) — that name resolves over sealed DoT, the hosts file is not consulted.
printf '127.0.0.1 localhost\n6.6.6.6 github.com\n6.6.6.6 codeload.github.com\n6.6.6.6 objects.githubusercontent.com\n' > "$PROJ"
POUT="$("$BIN" resolve-egress github-https 2>&1 || true)"
echo "$POUT" | sed 's/^/  /'
if echo "$POUT" | grep -q '6.6.6.6'; then
  gate no GK-poison-ignored   # the poison leaked into a pin — Option 4 is broken
else
  gate ok GK-poison-ignored   # poison never used, regardless of whether DoT reached the network
fi

echo "--- GK-live-dot (network-gated: the public name resolves to a REAL IP over sealed DoT) ---"
# If DoT reached Cloudflare/Quad9, github.com pins a real (non-poison) IP. If the network/853 is blocked
# in this env, resolve fails closed (no pin) — still no poison, so we WARN rather than FAIL.
if echo "$POUT" | grep -Eq '^host github\.com ([0-9]+\.){3}[0-9]+' && ! echo "$POUT" | grep -q 'host github.com 6.6.6.6'; then
  gate ok GK-live-dot
else
  gate warn "GK-live-dot (sealed DoT unreachable in this env — poison still ignored; run where :853 egress is open)"
fi

echo ""
echo "==================================================================="
echo "PASS=$PASS WARN=$WARN FAIL=$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
