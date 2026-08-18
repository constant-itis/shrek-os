#!/usr/bin/env bash
# Shrek OS Phase-1 — build the image INSIDE an ephemeral debian:trixie container so the Pop!_OS
# host is never mutated (see docs/phase1-spike.md §0). Drafted by the local coder tier, reviewed
# on the primary model.
#
# SPIKE SCOPE: dm-verity (S4, image/mkosi.repart/) + UKI signing (S5, image/mkosi.conf.d/
# 20-secureboot.conf) are wired, so this produces a SEALED verity root whose roothash-bearing UKI is
# sbsigned with the throwaway Shrek key. The bootc wrap (S7) is still NOT wired. Enrolling the key +
# booting under Secure Boot is S6 (scripts/boot-vm.sh).
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"
HOST_UID="$(id -u)"; HOST_GID="$(id -g)"

echo "=== STAGE 1 (host): build binaries + stage the image overlay ==="
cargo build --release
install -d image/overlay/usr/libexec/shrek image/overlay/usr/share/doc/shrek
install -m0755 target/release/{swampd,agentd,gatekeeperd,oniond,shrekctl} image/overlay/usr/libexec/shrek/
install -m0644 docs/*.md image/overlay/usr/share/doc/shrek/
mkdir -p out    # must exist before the bind-mount, or docker creates it root-owned

# S5: throwaway Shrek Secure Boot signing key (keys/ is gitignored, never shipped). Idempotent —
# generated once, reused across builds. MOK-enrolled in the VM at S6; not a shim-review key.
if [ ! -s keys/secureboot.key ] || [ ! -s keys/secureboot.crt ]; then
  echo "--- generating throwaway Shrek Secure Boot key (keys/) ---"
  install -d -m0700 keys
  openssl req -new -x509 -newkey rsa:2048 -nodes -sha256 -days 3650 \
    -keyout keys/secureboot.key -out keys/secureboot.crt \
    -subj "/CN=Shrek OS Secure Boot (throwaway spike key)/"
  chmod 0600 keys/secureboot.key
fi

echo "=== STAGE 2 (container): mkosi build in throwaway debian:trixie ==="
# --privileged: mkosi needs loop devices to assemble a disk image. Ephemeral container, host untouched.
docker run --rm --privileged \
  -v "${REPO_ROOT}:/work" -w /work/image \
  -e HOST_UID="${HOST_UID}" -e HOST_GID="${HOST_GID}" \
  debian:trixie \
  bash -euo pipefail -c '
    apt-get update
    # Package names verified on trixie at the S2 build: ukify=systemd-ukify, the EFI stub=systemd-boot-efi
    # (image pkg, see mkosi.conf); systemd-repart IS a separate package here (pulled as an mkosi dep).
    # sbsigntool provides sbsign for the S5 UKI signing (SecureBootSignTool=sbsign).
    apt-get install -y --no-install-recommends \
      mkosi systemd-ukify sbsigntool erofs-utils dosfstools mtools apparmor
    # S5 key/cert paths supplied here (harness knows the /work mount); SecureBoot=yes lives in config.
    mkosi --force \
      --secure-boot-key /work/keys/secureboot.key \
      --secure-boot-certificate /work/keys/secureboot.crt \
      build
    chown -R "${HOST_UID}:${HOST_GID}" /work/out
  '

echo "=== done — artifacts in out/ (dm-verity SEALED root + sbsigned UKI; S7 bootc still to wire) ==="
