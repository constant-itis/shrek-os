#!/usr/bin/env bash
# Phase-5 slice-6 — assemble the T2 (gVisor) sealed runtime artifacts into an mkosi ExtraTree so
# `mkosi build` copies them into the dm-verity `/usr` at /usr/lib/shrek/{runsc,t2-rootfs} — the
# compiled-in PROD default paths gatekeeperd's t2_plane reads (crates/gatekeeperd/src/t2_plane.rs:
# sealed_runsc_path/sealed_rootfs_path). The SHREK_T2_* env overrides are ORACLE-ONLY; the sealed
# image has none, so the constructor reads only read-only, roothash-authenticated /usr — no writable
# authority source.
#
#   seal-t2-artifacts.sh <extra-tree-root> <verified-runsc-path>
#
# Invoked from scripts/build-in-container.sh STAGE 2 (inside the ephemeral debian:trixie container,
# where busybox-static is a clean apt install). The runsc is fetched + sha256-verified on the HOST in
# STAGE 1 (from image/supply/gvisor.pin, NEVER 'latest'); this script RE-verifies before sealing —
# an unverified binary must never enter the sealed image — then builds the minimal busybox rootfs.
set -euo pipefail

TREE="${1:?usage: seal-t2-artifacts.sh <extra-tree-root> <verified-runsc-path>}"
RUNSC_SRC="${2:?usage: seal-t2-artifacts.sh <extra-tree-root> <verified-runsc-path>}"

# Pinned identity — MUST equal the sha recorded in image/supply/gvisor.pin (drift-guarded) and the
# oracle (scripts/t2-construct-proof.sh). release-20260810.0, x86_64.
PIN_SHA256="670bcd3cbc103f00d8bb5098edc370f32397ee4c134231436bafa659bb3c068e"

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PIN="$REPO_ROOT/image/supply/gvisor.pin"
# Drift guard: the hash we are about to seal MUST be the one recorded in the pin manifest.
grep -q "$PIN_SHA256" "$PIN" || { echo "SEAL ABORT: $PIN_SHA256 not present in $PIN (pin drift)"; exit 1; }

# Re-verify the handed-off runsc before it enters the sealed /usr.
GOT="$(sha256sum "$RUNSC_SRC" | awk '{print $1}')"
[ "$GOT" = "$PIN_SHA256" ] || { echo "SEAL ABORT: runsc sha256 $GOT != pinned $PIN_SHA256"; exit 1; }

DEST="$TREE/usr/lib/shrek"
install -d "$DEST"
install -m0755 "$RUNSC_SRC" "$DEST/runsc"

# Minimal pinned sandbox rootfs = busybox-static + RELATIVE applet symlinks. Absolute links break
# inside the sandbox: `busybox --install -s` writes /rootfs/bin/busybox targets that do not exist at
# the sandbox root ("failed to load /bin/sh") — see scripts/t2-construct-proof.sh. The applet set
# matches the oracle rootfs so the sealed VM S5 gate exercises the identical userland.
BB="$(command -v busybox)" || { echo "SEAL ABORT: busybox-static not installed in build container"; exit 1; }
ROOTFS="$DEST/t2-rootfs"
rm -rf "$ROOTFS"
install -d "$ROOTFS/bin"
install -m0755 "$BB" "$ROOTFS/bin/busybox"
for a in sh cat ls nc timeout echo test; do ln -sf busybox "$ROOTFS/bin/$a"; done

echo "seal-t2-artifacts: runsc $(stat -c%s "$DEST/runsc") bytes (sha256 verified) + t2-rootfs ($(ls "$ROOTFS/bin" | wc -l) entries) -> $DEST"
