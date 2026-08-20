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
# Phase-5 slice-7 (B1): the sealed closed-world in-sandbox acceptance probe (spike-only, strip before
# ship with the other gate scaffolding). Enrolled in gatekeeperd's compiled-in CLOSED_WORLD so it
# legitimately derives T-first on the sealed image (a shell cannot — B1 treats it as open-world).
# Phase-5 slice-9: rebuild it as a STATIC PIE (no PT_INTERP) so the S7 exec-island gate can RUN it as a
# pinned entrypoint (Fork A rejects a dynamically-linked pin). A static binary runs anywhere a dynamic
# one does, so S4/S6 are unaffected, and the S6 manifest bake measures whatever is installed here.
RUSTFLAGS="-C target-feature=+crt-static" cargo build --release -p shrek-gate-probe
install -m0755 target/release/gate-probe image/overlay/usr/libexec/shrek/gate-probe
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

# Phase-5 slice-6: fetch + sha256-verify the PINNED runsc (image/supply/gvisor.pin) on the host into a
# cache (never re-downloaded across builds; NEVER 'latest'). Bind-mounted into STAGE 2, where
# seal-t2-artifacts.sh re-verifies it and seals it + the busybox rootfs under dm-verity /usr. Same
# pin/cache path as the oracle (scripts/t2-construct-proof.sh).
GVISOR_SHA256="670bcd3cbc103f00d8bb5098edc370f32397ee4c134231436bafa659bb3c068e"
GVISOR_URL="https://storage.googleapis.com/gvisor/releases/release/20260810.0/x86_64/runsc"
CACHE="${XDG_CACHE_HOME:-$HOME/.cache}/shrek"; mkdir -p "$CACHE"
RUNSC="$CACHE/runsc-20260810.0"
if [ ! -f "$RUNSC" ] || [ "$(sha256sum "$RUNSC" | awk '{print $1}')" != "$GVISOR_SHA256" ]; then
  echo "--- fetching pinned runsc (release-20260810.0) ---"
  curl -fsSL -m 300 -o "$RUNSC" "$GVISOR_URL"
fi
[ "$(sha256sum "$RUNSC" | awk '{print $1}')" = "$GVISOR_SHA256" ] || { echo "runsc PIN MISMATCH — aborting build"; exit 1; }
echo "--- runsc pinned + verified ($GVISOR_SHA256) ---"

echo "=== STAGE 2 (container): mkosi build in throwaway debian:trixie ==="
# --privileged: mkosi needs loop devices to assemble a disk image. Ephemeral container, host untouched.
docker run --rm --privileged \
  -v "${REPO_ROOT}:/work" -w /work/image \
  -e HOST_UID="${HOST_UID}" -e HOST_GID="${HOST_GID}" \
  -v "${RUNSC}:/t2-runsc-verified:ro" \
  debian:trixie \
  bash -euo pipefail -c '
    apt-get update
    # Package names verified on trixie at the S2 build: ukify=systemd-ukify, the EFI stub=systemd-boot-efi
    # (image pkg, see mkosi.conf); systemd-repart IS a separate package here (pulled as an mkosi dep).
    # sbsigntool provides sbsign for the S5 UKI signing (SecureBootSignTool=sbsign). busybox-static =
    # the T2 sandbox rootfs userland (Phase-5 slice-6 seal), static so it needs no in-rootfs libs.
    apt-get install -y --no-install-recommends \
      mkosi systemd-ukify sbsigntool erofs-utils dosfstools mtools apparmor busybox-static fsverity
    # Phase-5 slice-6: assemble the T2 gVisor artifacts into the mkosi.extra.t2 ExtraTree BEFORE mkosi
    # runs (30-t2-gvisor.conf seals it into /usr). Re-verifies the pinned runsc + builds the rootfs.
    bash /work/scripts/seal-t2-artifacts.sh /work/image/mkosi.extra.t2 /t2-runsc-verified
    # Phase-5 slice-8 (S6 positive-pin VM gate, spike): bake the sealed pin-manifest BEFORE mkosi seals
    # the overlay under dm-verity /usr. The gate copies the sealed gate-probe onto a runtime fs-verity fs
    # and must DERIVE T-pinned, so the manifest pins gate-probe`s fs-verity digest. fs-verity digest is
    # content-addressed (sha256 over 4096-byte Merkle blocks), so this OFFLINE `fsverity digest` equals
    # the kernel FS_IOC_MEASURE_VERITY measurement the gate takes at runtime (verified — see #2589). The
    # manifest grammar is `<algo> <hex> <class>`; fsverity prints `sha256:<hex>`, so split off the algo.
    GP_OVL=/work/image/overlay/usr/libexec/shrek/gate-probe
    PIN_HEX=$(fsverity digest --hash-alg=sha256 --block-size=4096 "$GP_OVL" | cut -d: -f2 | cut -d" " -f1)
    [ "${#PIN_HEX}" = 64 ] || { echo "S6 bake: unexpected fsverity digest [$PIN_HEX]"; exit 1; }
    install -d /work/image/overlay/usr/lib/shrek
    printf "shrek-pin-manifest v1\nsha256 %s closed-world\n" "$PIN_HEX" \
      > /work/image/overlay/usr/lib/shrek/pin-manifest
    echo "--- baked pin-manifest (S6): sha256 $PIN_HEX closed-world ---"
    # S5 key/cert paths supplied here (harness knows the /work mount); SecureBoot=yes lives in config.
    mkosi --force \
      --secure-boot-key /work/keys/secureboot.key \
      --secure-boot-certificate /work/keys/secureboot.crt \
      build
    chown -R "${HOST_UID}:${HOST_GID}" /work/out /work/image/mkosi.extra.t2
  '

echo "=== done — version ${VERSION} in out/ ==="
echo "    disk: ${RAW}   (A/B: root slot A populated, slot B empty; /var volatile)"
echo "    split artifacts (systemd-sysupdate [Source]):"
ls -1 "out/shrek_${VERSION}_"* 2>/dev/null | sed 's/^/      /' || true
