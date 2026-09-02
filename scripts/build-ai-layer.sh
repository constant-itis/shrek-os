#!/usr/bin/env bash
# Build the signed shrek-ai sysext DDI (ADR-006 M1 slice 1, docs/adr-006-slice1-onion-skeleton.md) — the
# SKELETON of the optional on-device AI layer. Runs mkosi in an ephemeral --privileged debian:trixie
# container (same idiom as scripts/build-dev-layer.sh / scripts/build-bench-layer.sh); the build host
# stays untouched. Reuses the throwaway Shrek key from scripts/build-in-container.sh.
#
# Slice 1 carries NO Packages= (marker-only, the shrek-hello shape), so — unlike build-dev-layer.sh /
# build-bench-layer.sh — there is NO base-tree overlay delta to compute: mkosi builds the signed sysext
# directly from the ExtraTrees overlay. The Mode-A process set (inference server, on-box mycelium runtime,
# `shrek ai` shell, seed brain — ADR-006 §3) arrives in later slices; when it brings real packages this
# script grows the --base-tree/--overlay dance then.
#
# Produces out/layers/shrek-ai*.raw; scripts/build-layers.sh stages it into the store when INCLUDE_AI=1 and
# the sealed onion-policy (`enable shrek-ai`) makes oniond merge it. A box built without INCLUDE_AI=1 simply
# never carries the layer — listed-but-absent is a clean no-op (same tolerance as shrek-dev/shrek-bench).
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"
HOST_UID="$(id -u)"; HOST_GID="$(id -g)"
[ -s keys/secureboot.key ] && [ -s keys/secureboot.crt ] || {
  echo "missing keys/secureboot.{key,crt} — run scripts/build-in-container.sh once first" >&2; exit 1; }

echo "=== building shrek-ai sysext (slice 1 skeleton — marker only, no packages) in debian:trixie ==="
mkdir -p out/layers out/mkosi-vartmp
# Bind-mount a host ext4 dir OVER /var/tmp so mkosi's overlayfs workspace lands on real ext4, not docker's
# overlay2 (overlayfs-on-overlayfs mounts EINVAL). Same guard as build-dev-layer.sh / build-bench-layer.sh.
docker run --rm --privileged \
  -v "${REPO_ROOT}:/work" -w /work \
  -v "${REPO_ROOT}/out/mkosi-vartmp:/var/tmp" \
  -e HOST_UID="${HOST_UID}" -e HOST_GID="${HOST_GID}" \
  debian:trixie \
  bash -euo pipefail -c '
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq >/dev/null
    apt-get install -y --no-install-recommends -qq \
      ca-certificates openssl \
      mkosi systemd-ukify erofs-utils squashfs-tools dosfstools e2fsprogs systemd fdisk >/dev/null

    # Signed marker-only sysext: verity=yes + key + cert. No Packages= ⇒ no base tree needed; mkosi emits
    # the DDI straight from ExtraTrees (the shrek-hello path). Keys stay on the CLI (config is path-agnostic).
    SIGN="--verity=yes --verity-key=/work/keys/secureboot.key --verity-certificate=/work/keys/secureboot.crt"
    cd /work/layers/shrek-ai
    mkosi --force $SIGN build
    cd /work
    rm -rf /var/tmp/* 2>/dev/null || true
    echo "--- built shrek-ai layer artifact ---"; ls -l out/layers/shrek-ai* 2>/dev/null || true
    chown -R "$HOST_UID:$HOST_GID" out/layers out/mkosi-vartmp 2>/dev/null || true
  '
rmdir out/mkosi-vartmp 2>/dev/null || true
echo "done. next: INCLUDE_AI=1 scripts/build-layers.sh desktop assembles shrek-ai into the store (if present),"
echo "      then boot — the sealed onion-policy enables it and oniond merges the AI-layer skeleton onto /usr."
