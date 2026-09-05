#!/usr/bin/env bash
# Shrek OS Phase-1 — S7: perform ONE A/B update (v1 → v2) offline, with systemd-sysupdate.
#
# The built v1 disk has root slot A populated + slot B empty (Label=_empty). This applies v2's split
# artifacts into that empty slot and drops a boot-counted v2 UKI into the ESP — exactly what an
# on-appliance `systemd-sysupdate update` would do, but run offline against a COPY of the disk
# (--image=) so the built artifacts are never mutated and the result is reproducible. Boot the result
# with `RAW=out/shrek-updated.raw scripts/boot-vm.sh` — systemd-boot picks the newest UKI (v2).
#
# Runs in an ephemeral debian:trixie --privileged container (systemd-sysupdate ships in
# systemd-container on trixie; --privileged for the loop devices it needs to open the disk image).
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"
HOST_UID="$(id -u)"; HOST_GID="$(id -g)"

FROM="out/shrek_1_x86-64.raw"          # installed v1 disk (slot A = v1, slot B = _empty)
UPDATED="out/shrek-updated.raw"
[ -f "$FROM" ]                 || { echo "missing $FROM — run scripts/build-in-container.sh 1 first" >&2; exit 1; }
ls out/shrek_2_x86-64.* >/dev/null 2>&1 || { echo "missing v2 split artifacts — run scripts/build-in-container.sh 2 first" >&2; exit 1; }

# The networked [Source] MatchPattern now expects xz-compressed splits (...raw.xz) — see the flip in
# image/overlay/usr/lib/sysupdate.d/. (xz, not zstd: sysupdate 257 decompresses .xz/.gz on write but NOT
# .zst.) The offline proof feeds a LOCAL pool via --transfer-source, so that pool must ALSO carry .xz (the
# url Path is overridden host-side, but the filename pattern is not). Compress the v2 root+verity splits
# (idempotent; UKI stays uncompressed). This is what keeps the S7 offline proof working after the flip.
echo "=== compressing v2 root/verity splits to .xz for the local pool ==="
for f in out/shrek_2_x86-64.root-*.raw; do
  [ -e "$f" ] || continue
  [ -f "$f.xz" ] && [ "$f.xz" -nt "$f" ] || { echo "  xz $(basename "$f")"; xz -q -f -T0 -6 -c "$f" > "$f.xz"; }
done

echo "=== copying $FROM → $UPDATED (built artifact stays untouched) ==="
cp --reflink=auto "$FROM" "$UPDATED"

echo "=== applying v2 into the empty slot of $UPDATED (offline systemd-sysupdate) ==="
# -v /dev:/dev: systemd-sysupdate --image dissects the disk via a loop device with partition scanning.
# Docker's default tmpfs /dev has no devtmpfs/udev, so loop partition nodes (loopNp1..p5) never appear
# and dissection fails ("Cannot dissect image: No such file or directory"). Sharing the host /dev lets
# the host's udev create those nodes. Transient loop devices only; the host is otherwise untouched.
# UID/GID passed as env (not shell-quote-juggled) so the container script stays cleanly single-quoted.
# --image dissects the target disk and reads its transfer defs from the image's own
# /usr/lib/sysupdate.d/ (no --definitions needed — auto-found once --image sets the root).
# --transfer-source stays host-side (the external pool of new-version split artifacts).
# NOTE: do NOT pass --offline — on systemd 257 it suppresses discovery of AVAILABLE versions from the
# local --transfer-source dir (verified: `list 2` finds v2 without it, "Update '2' not found" with it).
# --verify=no: spike source has no signed manifest; update integrity rides on the sbsigned UKI
# (roothash) at boot, as elsewhere in Phase 1.
docker run --rm --privileged -v /dev:/dev \
  -v "${REPO_ROOT}:/work" -w /work \
  -e HOST_UID="${HOST_UID}" -e HOST_GID="${HOST_GID}" \
  debian:trixie \
  bash -euo pipefail -c '
    apt-get update -qq >/dev/null
    apt-get install -y --no-install-recommends -qq \
      systemd systemd-repart systemd-boot systemd-container >/dev/null
    SU=/usr/lib/systemd/systemd-sysupdate
    COMMON="--image=/work/out/shrek-updated.raw --transfer-source=/work/out --verify=no"
    echo "--- versions visible BEFORE update ---"
    "$SU" $COMMON list || true
    echo "--- update to version 2 ---"
    "$SU" $COMMON update 2
    echo "--- versions AFTER update ---"
    "$SU" $COMMON list || true
    chown "${HOST_UID}:${HOST_GID}" /work/out/shrek-updated.raw
  '
echo "=== done — the A/B-updated disk is $UPDATED; boot it with:"
echo "      RAW=out/shrek-updated.raw scripts/boot-vm.sh"