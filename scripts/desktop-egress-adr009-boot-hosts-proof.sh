#!/usr/bin/env bash
# Shrek OS — ADR-009 v2 boot-lag fix: host oracle for the /etc/hosts RE-COMPOSE inside `egressd
# reconcile`. Proves the delivery half of the boot-lag fix (#3207 follow-up) without a VM, via the
# `oracle-env` build's SHREK_EGRESS_*/SHREK_HOSTS_* overrides.
#
# THE BUG: the base `shrek-hosts-compose` oneshot is ordered `Before local-fs.target`, i.e. it composes
# `/etc/hosts` BEFORE egressd (ordered `After local-fs.target`) repopulates the tmpfs `/run` pin map from
# the persisted store. So at oneshot time the pin map is empty and the composed `/etc/hosts` carries NO
# deliverable pins — weather stays dark on a rebooted box until the next bless. The fix: `reconcile` now
# recomposes `/etc/hosts` from the pins it just re-projected (§5), OFFLINE (already-stored pins, no DoT).
#
#   B-deadzone       an early oneshot compose against an EMPTY /run leaves weather DARK in /etc/hosts
#                    (reproduces the boot ordering the fix targets)
#   B-reconcile-lift `egressd reconcile` alone (no separate compose-hosts) recomposes the PERSISTED weather
#                    pins into /etc/hosts — run in a fresh netns (NO network), so the lift is PROVABLY
#                    offline (a DoT re-resolve is impossible there)
#   B-owner-iso      a foreign/owner host planted in the live /run pin map does NOT survive a boot
#                    reconcile into /etc/hosts — the reconcile re-projects the pin map from the sealed
#                    store before composing, so §4.4 isolation holds on the boot path too
set -uo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

PASS=0; FAIL=0
check() { if [ "$3" -eq 0 ]; then echo "  PASS $1 — $2"; PASS=$((PASS+1)); else echo "  FAIL $1 — $2"; FAIL=$((FAIL+1)); fi; }

echo "=== building egressd (release, oracle-env) ==="
CARGO_NET_OFFLINE=true cargo build --release -p egressd --features oracle-env >/dev/null 2>&1 || \
  cargo build --release -p egressd --features oracle-env
B="$REPO_ROOT/target/release/egressd"
SEALED_SRC="$REPO_ROOT/image/overlay/usr/lib/shrek/egress-capabilities/weather.capability"

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
S="$WORK/store"
HRUN="$WORK/run"                 # SHREK_HOSTS_RUN  → /run/shrek        (holds the composed hosts file)
ERUN="$HRUN/egress"              # SHREK_EGRESS_RUN → /run/shrek/egress (holds pinned/state)
HHOME="$WORK/home"               # SHREK_HOSTS_HOME → /home/.shrek-system (binding store)
SEALED="$WORK/cap-sealed"; OWNER="$WORK/cap-owner"; STAGING="$WORK/cap-staging"
mkdir -p "$ERUN" "$HHOME" "$SEALED" "$OWNER" "$STAGING"
cp "$SEALED_SRC" "$SEALED/weather.capability"

export SHREK_EGRESS_STORE="$S" SHREK_EGRESS_RUN="$ERUN"
export SHREK_HOSTS_HOME="$HHOME" SHREK_HOSTS_RUN="$HRUN"
export SHREK_EGRESS_CAP_SEALED="$SEALED" SHREK_EGRESS_CAP_OWNER="$OWNER" SHREK_EGRESS_CAP_STAGING="$STAGING"
"$B" store init >/dev/null
HOSTS="$HRUN/hosts"

echo "=== 1. persist a blessed weather pin (survives the 'reboot') ==="
# Seed the store exactly as a prior bless left it: a blessed weather profile + its resolved pins. This is
# the persistent state a reboot starts from; no live DoT needed (mirrors the s2-proof D-deliver seed).
"$B" store bless --profile weather --tier one-click --at 100 >/dev/null
"$B" store pin --profile weather --at 100 \
  --pin api.open-meteo.com=104.16.1.1 --pin geocoding-api.open-meteo.com=104.16.2.2 >/dev/null

echo "=== 2. reproduce the boot dead-zone: oneshot composes against an empty /run ==="
# The shrek-hosts-compose oneshot runs BEFORE egressd repopulates /run — so the pin map is empty and the
# composed /etc/hosts carries no deliverable pins. Clear the /run projection to model that instant.
rm -f "$ERUN/pinned"
"$B" compose-hosts >/dev/null
! grep -q 'open-meteo' "$HOSTS"
check "B-deadzone" "early oneshot compose (empty /run) leaves weather DARK in /etc/hosts" $?

echo "=== 3. boot reconcile recomposes /etc/hosts OFFLINE ==="
# `egressd reconcile` is root-only, so map to uid 0 with `unshare -rn`; the fresh netns has NO network, so
# a DoT re-resolve is impossible — if weather lands in /etc/hosts it can ONLY be the offline lift of the
# persisted pin. reconcile_cap's nft apply fails without a loaded table in the netns, but that is caught
# (deny floor stands) and the §5 hosts recompose still runs. No separate `compose-hosts` is invoked here.
unshare -rn "$B" reconcile >/dev/null 2>&1
grep -q '104.16.1.1 api.open-meteo.com' "$HOSTS" && grep -q '104.16.2.2 geocoding-api.open-meteo.com' "$HOSTS"
check "B-reconcile-lift" "boot reconcile alone recomposes weather into /etc/hosts offline" $?

echo "=== 4. §4.4 isolation holds on the reconcile path ==="
# Plant a foreign/owner host directly in the live /run pin map. The boot reconcile re-projects the pin
# map from the sealed STORE (project_pinned, §4 — only sealed store pins) BEFORE the §5 compose, so the
# planted line is discarded and never reaches /etc/hosts. (The compose-time sealed-source filter itself is
# proven separately in the hosts unit tests; this proves the boot path does not leak a live-/run poison.)
printf 'radar.example.com 7.7.7.7\n' >> "$ERUN/pinned"
unshare -rn "$B" reconcile >/dev/null 2>&1
grep -q '104.16.1.1 api.open-meteo.com' "$HOSTS" && ! grep -q '7.7.7.7' "$HOSTS" && ! grep -q 'radar.example.com' "$HOSTS"
check "B-owner-iso" "boot recompose lifts sealed weather only; foreign/owner host excluded" $?

echo "=== $PASS passed, $FAIL failed ==="
[ "$FAIL" -eq 0 ]
