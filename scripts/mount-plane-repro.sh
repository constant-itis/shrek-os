#!/usr/bin/env bash
# Phase-5 slice-1 — M1 host/container repro: the caps-enforced mount plane, validated in a privileged
# debian:trixie container (the fast oracle before the ~35-min VM cycle). Mirrors exactly what
# gatekeeperd constructs at M3: a synthetic OS-shaped root with an EMPTY grant tree, the granted path
# bound read-only, UID isolation via --private-users. Asserts the four enforcement properties as
# anchored SHREK_GATE lines (docs/phase5-slice1-mount.md, M1/M4).
#
# Usage: scripts/mount-plane-repro.sh          # runs the whole repro in a throwaway container
#        (inside the container it re-execs itself via IN_CONTAINER=1)
set -uo pipefail

if [ "${IN_CONTAINER:-0}" != "1" ]; then
  REPO_ROOT="$(git rev-parse --show-toplevel)"
  echo "=== building M2 fd-pinning proof (host) ==="
  ( cd "$REPO_ROOT" && cargo build --example mount_toctou_proof ) || exit 3
  echo "=== launching privileged debian:trixie oracle ==="
  exec docker run --rm --privileged -e IN_CONTAINER=1 \
    -v "$REPO_ROOT/scripts/mount-plane-repro.sh:/repro.sh:ro" \
    -v "$REPO_ROOT/target/debug/examples/mount_toctou_proof:/m2:ro" \
    debian:trixie bash /repro.sh
fi

# ---------------- inside the container ----------------
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null 2>&1
apt-get install -y --no-install-recommends systemd-container busybox-static >/dev/null 2>&1

# nspawn-in-container prerequisites (environment, not design — see M0 results).
mount --make-rshared / 2>/dev/null || true
mount -t tmpfs tmpfs /run 2>/dev/null || true

FAIL=0
gate() { # gate <name> <cond-rc> <detail>
  if [ "$2" = "0" ]; then echo "SHREK_GATE: PASS gate=$1 $3"; else echo "SHREK_GATE: FAIL gate=$1 reason=$3"; FAIL=1; fi
}

echo "=== M2: fd-pinning TOCTOU proof (relocate defeats a source swap) ==="
if /m2; then echo "  (M2 proof: all gates PASS)"; else echo "SHREK_GATE: FAIL gate=m2-proof reason=nonzero"; FAIL=1; fi

echo
echo "=== M1: nspawn caps-enforced mount plane ==="
# host tree: granted 'project', denied 'vault'
mkdir -p /srv/project /srv/vault
echo PROJECT > /srv/project/marker
echo VAULT   > /srv/vault/marker

# broker-controlled staging = the relocated grant (M2 proves this bind is race-safe). Here we bind it
# plainly; the point under test is nspawn's enforcement once handed the controlled path.
ID=m1
STAGE=/run/shrek/$ID/grants
mkdir -p "$STAGE/project"
mount --bind -o ro /srv/project "$STAGE/project"

# synthetic OS-shaped root: /usr present (busybox), grant tree /srv EMPTY (this is what hides vault).
ROOT=/run/shrek/$ID/root
mkdir -p "$ROOT/usr/bin" "$ROOT/srv" "$ROOT/etc" "$ROOT/proof"
cp /bin/busybox "$ROOT/usr/bin/busybox"
for a in ls cat id sh stat; do ln -sf busybox "$ROOT/usr/bin/$a"; done
ln -sf usr/bin "$ROOT/bin"
printf 'ID=shrek-sandbox\n' > "$ROOT/etc/os-release"

# --console=pipe: no pty, so the workload's stdout is a clean pipe (a pty would inject \r\n and break
# exact assertions). This is also how gatekeeperd drives a non-interactive workload.
NS="systemd-nspawn -q --console=pipe --register=no --keep-unit --machine=shrek-$ID --directory=$ROOT"
BIND="--bind-ro=$STAGE/project:/srv/project"
clean() { tr -d '\r'; }  # belt-and-suspenders against any stray CR

# (a) granted path readable inside
OUT=$($NS $BIND /bin/cat /srv/project/marker 2>/dev/null | clean)
[ "$OUT" = "PROJECT" ]; gate project-readable $? "marker=$OUT"

# (b) denied path is ENOENT (absent), not merely unreadable
$NS $BIND /bin/cat /srv/vault/marker >/dev/null 2>/tmp/vault.err
if grep -qi 'No such file' /tmp/vault.err; then gate vault-enoent 0 "ENOENT"; else gate vault-enoent 1 "$(cat /tmp/vault.err | clean)"; fi

# (c) vault absent from the parent listing (only 'project' appears under /srv)
LS=$($NS $BIND /bin/ls /srv 2>/dev/null | clean | tr '\n' ',')
if [ "$LS" = "project," ]; then gate vault-absent-parent 0 "ls-srv=$LS"; else gate vault-absent-parent 1 "ls-srv=$LS"; fi

# (d) --private-users: the bound marker is owned by HOST uid 0; with UID isolation that host root is
#     NOT mapped into the sandbox userns, so it appears as the overflow uid 65534 (nobody) inside.
#     Without isolation it would read 0. This proves the sandbox is not sharing the host uid space.
INSIDE_OWNER=$($NS --private-users=pick $BIND /bin/stat -c '%u' /srv/project/marker 2>/dev/null | clean)
if [ "$INSIDE_OWNER" = "65534" ]; then
  gate private-users 0 "host-root-file reads as uid=65534 (unmapped) inside"
else
  gate private-users 1 "host-root-file reads as uid=$INSIDE_OWNER inside (expected 65534)"
fi

echo
if [ "$FAIL" = "0" ]; then echo "SHREK_GATE: PASS M1 mount-plane repro"; else echo "SHREK_GATE: FAIL M1 mount-plane repro"; fi
exit $FAIL
