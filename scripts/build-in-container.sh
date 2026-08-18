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

# S7: build a specific version (default 1). mkosi reads image/mkosi.version for the %v in Output/UKI
# names, so v1 and v2 produce distinct, versioned split artifacts that systemd-sysupdate can A/B
# between (docs/phase1-s7-sysupdate.md). image/mkosi.version is gitignored (a build input, not source).
VERSION="${1:-1}"
echo "$VERSION" > image/mkosi.version
RAW="out/shrek_${VERSION}_x86-64.raw"
echo "=== building Shrek OS version ${VERSION} → ${RAW} ==="

echo "=== STAGE 1 (host): build binaries + stage the image overlay ==="
cargo build --release
install -d image/overlay/usr/libexec/shrek image/overlay/usr/share/doc/shrek
install -m0755 target/release/{swampd,agentd,gatekeeperd,oniond,shrekctl} image/overlay/usr/libexec/shrek/
install -m0644 docs/*.md image/overlay/usr/share/doc/shrek/

# S8: deliberately-broken build. BREAK=1 stages a poison marker into the sealed image; the boot
# health gate (shrek-boot-health.service → /usr/lib/shrek/boot-health-check) fails on any version
# carrying it, forcing an automatic A/B rollback to the last-good UKI. The marker is gitignored and
# removed on every normal build, so `build-in-container.sh <v>` (no BREAK) is always a healthy build.
POISON=image/overlay/usr/lib/shrek/boot-poison
if [ "${BREAK:-0}" = "1" ]; then
  echo "!!! BREAK=1: staging poison marker — version ${VERSION} is a DELIBERATELY BROKEN update (S8) !!!"
  install -d image/overlay/usr/lib/shrek
  printf 'shrek-os S8 poison: version %s built with BREAK=1 — boot health gate fails on purpose.\n' "$VERSION" > "$POISON"
else
  rm -f "$POISON"
fi

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

# Phase 2 (Onion): bake the Shrek cert into the sealed root at /usr/lib/verity.d/shrek.crt so
# systemd-sysext/confext trust layers signed by the same throwaway key (shrek-onion.service merges
# under --image-policy=…signed). The cert is a build artifact (keys/ gitignored) — staged, not committed.
install -d image/overlay/usr/lib/verity.d
install -m0644 keys/secureboot.crt image/overlay/usr/lib/verity.d/shrek.crt

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

echo "=== done — version ${VERSION} in out/ ==="
echo "    disk: ${RAW}   (A/B: root slot A populated, slot B empty; /var volatile)"
echo "    split artifacts (systemd-sysupdate [Source]):"
ls -1 "out/shrek_${VERSION}_"* 2>/dev/null | sed 's/^/      /' || true
