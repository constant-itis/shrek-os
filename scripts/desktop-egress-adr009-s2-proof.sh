#!/usr/bin/env bash
# Shrek OS — ADR-009 v2 S2: host oracle for the DATA-DRIVEN capability layer (catalog loader, the
# generalized @cap_pinned set, the sealed-source /etc/hosts delivery bridge, and the owner-manifest
# ceremony verbs + want-inbox). Proves the S2 machine fast, without a VM, via the `oracle-env` build's
# SHREK_EGRESS_*/SHREK_HOSTS_* overrides. The @cap_pinned union enforcement against REAL nft is proven in
# desktop-egress-s2-proof.sh (16/16); THIS oracle proves the ADR-009 additions layered on top.
#
#   D-catalog     the sealed weather.capability loads into the state view (source=sealed feature=dms:weather
#                 + the root-authored title/purpose card lines)
#   D-deliver     a blessed sealed weather pin is LIFTED into /run/shrek/hosts (the delivery bridge)
#   D-owner-iso   an owner-source host is NEVER lifted into /run/shrek/hosts (§4.4 structural isolation)
#   D-hostile     a malformed owner manifest is inert — the catalog still loads weather, the bogus name is
#                 absent (fail-closed per-file; a rejected manifest ⇒ capability absent)
#   D-install     confirmed-manifest-install via the ROOT relay commits a valid owner manifest (state
#                 shows source=owner); the daemon is the sole writer of the live owner dir
#   D-refuse      confirmed-manifest-install REFUSES an owner manifest naming a system-reserved host (§4.4
#                 layer 2) — nothing is committed
#   D-remove      confirmed-manifest-remove drops the owner manifest (catalog no longer has it)
#   D-want        `want <catalog-token>` files a pending request; `want <unknown>` is refused (§2: a catalog
#                 token, never free text)
set -uo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

PASS=0; FAIL=0
check() { if [ "$3" -eq 0 ]; then echo "  PASS $1 — $2"; PASS=$((PASS+1)); else echo "  FAIL $1 — $2"; FAIL=$((FAIL+1)); fi; }

echo "=== building egressd (release, oracle-env) ==="
CARGO_NET_OFFLINE=true cargo build --release -p egressd --features oracle-env >/dev/null 2>&1 || \
  cargo build --release -p egressd --features oracle-env
B="$REPO_ROOT/target/release/egressd"
NFT_FILE="$REPO_ROOT/image/overlay/usr/lib/shrek/desktop-egress.nft"
SEALED_SRC="$REPO_ROOT/image/overlay/usr/lib/shrek/egress-capabilities/weather.capability"

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
S="$WORK/store"
HRUN="$WORK/run"                 # SHREK_HOSTS_RUN  → /run/shrek        (holds the composed hosts file)
ERUN="$HRUN/egress"              # SHREK_EGRESS_RUN → /run/shrek/egress (holds pinned/state/wants)
HHOME="$WORK/home"               # SHREK_HOSTS_HOME → /home/.shrek-system (binding store)
SEALED="$WORK/cap-sealed"; OWNER="$WORK/cap-owner"; STAGING="$WORK/cap-staging"
mkdir -p "$ERUN" "$HHOME" "$SEALED" "$OWNER" "$STAGING"
cp "$SEALED_SRC" "$SEALED/weather.capability"

export SHREK_EGRESS_STORE="$S" SHREK_EGRESS_RUN="$ERUN"
export SHREK_HOSTS_HOME="$HHOME" SHREK_HOSTS_RUN="$HRUN"
export SHREK_EGRESS_CAP_SEALED="$SEALED" SHREK_EGRESS_CAP_OWNER="$OWNER" SHREK_EGRESS_CAP_STAGING="$STAGING"
"$B" store init >/dev/null

# ── arena 1: catalog + delivery bridge (CLI + fs; no daemon, no nft) ──────────────────────────────────
echo "=== 1. catalog + delivery bridge (CLI) ==="

# D-catalog: the sealed weather manifest surfaces in the state view with its source + card text.
"$B" store project >/dev/null
STATE="$ERUN/state"
grep -q '^profile weather .*source=sealed feature=dms:weather' "$STATE" \
  && grep -q '^title weather Weather$' "$STATE" \
  && grep -q '^purpose weather Local forecast and location search$' "$STATE"
check "D-catalog" "sealed weather in state view w/ source+card text" $?

# D-deliver: bless + pin weather (seed the store like the s2-proof; no live DoT needed), then compose the
# hosts bridge — the sealed weather pins must be lifted into /run/shrek/hosts.
"$B" store bless --profile weather --tier one-click --at 100 >/dev/null
"$B" store pin --profile weather --at 100 \
  --pin api.open-meteo.com=104.16.1.1 --pin geocoding-api.open-meteo.com=104.16.2.2 >/dev/null
"$B" store project >/dev/null
"$B" compose-hosts >/dev/null
HOSTS="$HRUN/hosts"
grep -q '104.16.1.1 api.open-meteo.com' "$HOSTS" && grep -q '104.16.2.2 geocoding-api.open-meteo.com' "$HOSTS"
check "D-deliver" "sealed weather pins lifted into /run/shrek/hosts" $?

# D-owner-iso: an OWNER-source host, even present verbatim in the egress pinned map, is NEVER lifted into
# host-wide resolution (§4.4 layer 1). Append a foreign/owner host to the pinned map + recompose.
printf 'radar.example.com 7.7.7.7\n' >> "$ERUN/pinned"
"$B" compose-hosts >/dev/null
grep -q '104.16.1.1 api.open-meteo.com' "$HOSTS" && ! grep -q '7.7.7.7' "$HOSTS" && ! grep -q 'radar.example.com' "$HOSTS"
check "D-owner-iso" "owner-source host excluded from /run/shrek/hosts (weather still lifted)" $?

# D-hostile: a malformed manifest in the OWNER dir is inert — the catalog still loads weather; the bad
# name never appears. (A rejected manifest ⇒ capability absent, fail-closed per-file.)
printf 'this is not a manifest\n' > "$OWNER/broken.capability"
printf 'ignore me\n' > "$OWNER/README.txt"
"$B" store project >/dev/null
grep -q '^profile weather .*source=sealed' "$STATE" && ! grep -q '^profile broken ' "$STATE"
check "D-hostile" "malformed owner manifest inert; weather still active" $?
rm -f "$OWNER/broken.capability" "$OWNER/README.txt"

# ── arena 2: the daemon manifest ceremony verbs + want-inbox (root peer via userns + netns) ───────────
# The confirmed-manifest-* verbs are ROOT-peer only; `unshare -rn` maps the oracle user to uid 0 (so
# SO_PEERCRED reports root) AND gives a fresh netns where the baked table is loaded — `remove` reconciles
# @cap_pinned (real nft) to withdraw any tuples, so the table must be present. `install`/`want` touch only
# the store/owner dir/`/run` (no nft). DESKTOP_UID=0 so the identity-gated `want` verb is served to the
# mapped-root peer. State is captured INSIDE the block (install → snapshot → remove) so the assertions do
# not race the later remove.
echo "=== 2. manifest ceremony verbs + want-inbox (daemon, userns+netns root) ==="
cat > "$WORK/client.py" <<'PY'
import socket, os, time, sys
SOCK=os.environ["SOCK"]
def req(line):
    for _ in range(60):
        try:
            s=socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.settimeout(6); s.connect(SOCK); break
        except (FileNotFoundError, ConnectionRefusedError): time.sleep(0.1)
    else: return "NO-SOCKET"
    try: s.sendall((line+"\n").encode())
    except (BrokenPipeError, ConnectionResetError): pass
    d=b""
    try:
        while True:
            c=s.recv(4096)
            if not c: break
            d+=c
    except (socket.timeout, ConnectionResetError): pass
    s.close(); return d.decode(errors="replace").strip() or "REJECTED-CLOSED"
print(req(sys.argv[1]))
PY

# a valid owner candidate (deliver none, a fresh host) + a REFUSED one (names a reserved host).
cat > "$STAGING/radar.capability" <<'CAP'
schema shrek-egress-capability/1
name radar
title Weather Radar
purpose Regional precipitation radar
feature dms:radar
tier ceremony
deliver none
host radar.example.com tcp 443
CAP
cat > "$STAGING/evil.capability" <<'CAP'
schema shrek-egress-capability/1
name evil
title Evil
purpose Reserved-host grab
feature owner:evil
tier ceremony
deliver none
host api.open-meteo.com tcp 443
CAP

D2="$WORK/d2.out"
unshare -rn sh -c '
  export SOCK="'"$WORK"'/sock"
  export SHREK_EGRESS_STORE="'"$S"'" SHREK_EGRESS_RUN="'"$ERUN"'"
  export SHREK_HOSTS_HOME="'"$HHOME"'" SHREK_HOSTS_RUN="'"$HRUN"'"
  export SHREK_EGRESS_CAP_SEALED="'"$SEALED"'" SHREK_EGRESS_CAP_OWNER="'"$OWNER"'" SHREK_EGRESS_CAP_STAGING="'"$STAGING"'"
  export SHREK_EGRESS_SOCK="$SOCK" SHREK_EGRESS_DESKTOP_UID=0
  nft -f "'"$NFT_FILE"'" || { echo "NFT-LOAD-FAIL"; exit 1; }
  setsid "'"$B"'" daemon >"'"$WORK"'/d2.log" 2>&1 &
  DP=$!
  cl() { SOCK="$SOCK" python3 "'"$WORK"'/client.py" "$1"; }
  # install → snapshot state + file existence INSIDE the block (before remove races them)
  echo "INSTALL_OK=$(cl "confirmed-manifest-install radar")"
  echo "INSTALL_FILE=$( [ -f "'"$OWNER"'/radar.capability" ] && echo yes || echo no )"
  echo "INSTALL_STATE=$(grep "^profile radar " "'"$ERUN"'/state" | tr -d "\n")"
  echo "INSTALL_REFUSE=$(cl "confirmed-manifest-install evil")"
  echo "REFUSE_FILE=$( [ -f "'"$OWNER"'/evil.capability" ] && echo yes || echo no )"
  echo "WANT_OK=$(cl "want weather")"
  echo "WANT_FILE=$(grep "^want weather " "'"$ERUN"'/wants" >/dev/null 2>&1 && echo yes || echo no)"
  echo "WANT_BOGUS=$(cl "want bogusxyz")"
  echo "REMOVE_OK=$(cl "confirmed-manifest-remove radar")"
  echo "REMOVE_FILE=$( [ -f "'"$OWNER"'/radar.capability" ] && echo yes || echo no )"
  kill $DP 2>/dev/null
' >"$D2" 2>/dev/null
cat "$D2" | sed 's/^/    /'
g() { grep -m1 "^$1=" "$D2" | cut -d= -f2-; }

# D-install: radar committed to the live owner dir + it shows source=owner in the state view.
echo "$(g INSTALL_OK)" | grep -q '^OK confirmed-manifest-install radar' \
  && [ "$(g INSTALL_FILE)" = "yes" ] \
  && echo "$(g INSTALL_STATE)" | grep -q 'source=owner feature=dms:radar'
check "D-install" "owner manifest committed via relay, state=owner" $?

# D-refuse: evil (names api.open-meteo.com, reserved) is refused; nothing committed.
echo "$(g INSTALL_REFUSE)" | grep -q 'ERR refused' && [ "$(g REFUSE_FILE)" = "no" ]
check "D-refuse" "install of a reserved-host owner manifest refused, nothing written" $?

# D-want: a catalog token is accepted into the inbox; an unknown token is refused.
echo "$(g WANT_OK)" | grep -q '^OK want weather' && [ "$(g WANT_FILE)" = "yes" ] \
  && echo "$(g WANT_BOGUS)" | grep -q 'ERR unknown-capability'
check "D-want" "want catalog-token filed; unknown token refused" $?

# D-remove: confirmed-manifest-remove dropped radar from the live owner dir.
echo "$(g REMOVE_OK)" | grep -q '^OK confirmed-manifest-remove radar' && [ "$(g REMOVE_FILE)" = "no" ]
check "D-remove" "owner manifest removed via relay" $?

echo "======================================================================"
echo "desktop-egress ADR-009 S2 oracle: PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
