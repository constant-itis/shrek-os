#!/usr/bin/env bash
# Shrek OS Phase-2 — build the sysext/confext layer DDIs and assemble the external "layer store" disk
# (docs/phase2-onion.md). The store is the VM's 2nd drive (fs label `shrek-layers`); shrek-onion.service
# mounts it and merges the signed layers onto the sealed base. Cheap to rebuild — this is how we iterate
# gates L1–L4 without rebuilding the sealed root.
#
#   scripts/build-layers.sh good      signed-verity sysext + signed-verity confext  → should MERGE (L1/L2/L4, O1)
#   scripts/build-layers.sh select    TWO signed sysext (hello+extra) + signed confext → only ENABLED merge (O2)
#   scripts/build-layers.sh inject    like select + an oniond-inject marker naming shrek-extra → the
#                                     wall must refuse the compromised-brain request (G3, phase4-gatekeeperd)
#   scripts/build-layers.sh unsigned  sysext with verity but NO signature           → should be REFUSED (L3a, O3)
#   scripts/build-layers.sh tamper    signed sysext with a flipped byte (verity fails) → should be REFUSED (L3b, O3)
#
# Runs mkosi in an ephemeral --privileged debian:trixie container (loop devices for the DDI + verity);
# the beepboop host stays untouched. Reuses the throwaway Shrek key from scripts/build-in-container.sh.
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"
HOST_UID="$(id -u)"; HOST_GID="$(id -g)"
MODE="${1:-good}"
case "$MODE" in good|select|inject|unsigned|tamper|desktop) ;; *) echo "usage: $0 [good|select|inject|unsigned|tamper|desktop]" >&2; exit 1 ;; esac
# Desktop Bootstrap-0: the signed shrek-desktop sysext is a SEPARATE, heavier build (DMS + Qt runtime)
# produced by scripts/build-desktop-layer.sh; this script only ASSEMBLES it into the store, so
# require the DDI to already exist rather than rebuilding it here.
if [ "$MODE" = "desktop" ] && ! ls out/layers/shrek-desktop*.raw >/dev/null 2>&1; then
  echo "MODE=desktop needs a built desktop DDI — run scripts/build-desktop-layer.sh first" >&2; exit 1
fi
[ -s keys/secureboot.key ] && [ -s keys/secureboot.crt ] || {
  echo "missing keys/secureboot.{key,crt} — run scripts/build-in-container.sh once first" >&2; exit 1; }

echo "=== building layer DDIs + store (mode=${MODE}) in debian:trixie ==="
mkdir -p out/layers
docker run --rm --privileged \
  -v "${REPO_ROOT}:/work" -w /work \
  -e HOST_UID="${HOST_UID}" -e HOST_GID="${HOST_GID}" -e MODE="${MODE}" \
  debian:trixie \
  bash -euo pipefail -c '
    apt-get update -qq >/dev/null
    apt-get install -y --no-install-recommends -qq \
      mkosi systemd-ukify erofs-utils squashfs-tools dosfstools e2fsprogs openssl systemd fdisk >/dev/null
    # mkosi 25.3: --verity is a tri-state FEATURE (yes/no/auto); the roothash is SIGNED purely by
    # supplying --verity-key + --verity-certificate. signed = verity=yes + key + cert. mkosi REFUSES
    # verity=yes without a key ("Verity= is enabled but no verity key is configured"), so the unsigned
    # refusal case is verity=no — a plain, unprotected DDI (no verity, no signature) that an =signed
    # image policy rejects because it is not Verity-authenticated at all.
    SIGN="--verity=yes --verity-key=/work/keys/secureboot.key --verity-certificate=/work/keys/secureboot.crt"

    # --- sysext DDI: signed, except MODE=unsigned ships an unprotected (no-verity) image ---
    cd /work/layers/shrek-hello
    if [ "$MODE" = "unsigned" ]; then
      mkosi --force --verity=no build
    else
      mkosi --force $SIGN build
    fi

    # --- confext DDI: always signed (assembled into the store in good + select modes) ---
    cd /work/layers/shrek-conf
    mkosi --force $SIGN build

    # --- shrek-extra sysext DDI: signed SECOND layer, built for the selection gate (O2) and the
    # injection gate (G3). It is a valid signed layer; oniond must OMIT it (not sealed-enabled), and
    # the wall must REFUSE it even when a compromised oniond explicitly requests it. ---
    if [ "$MODE" = "select" ] || [ "$MODE" = "inject" ]; then
      cd /work/layers/shrek-extra
      mkosi --force $SIGN build
    fi
    cd /work

    echo "--- built layer artifacts ---"; ls -l out/layers/
    HELLO=$(ls out/layers/shrek-hello*.raw | head -1)
    echo "--- dissecting the sysext DDI (partition designators + signature) ---"
    systemd-dissect "$HELLO" 2>&1 | head -40 || true

    # --- tamper: corrupt the DDI so it fails verification DETERMINISTICALLY ---
    # A mid-image data-block flip is caught only lazily (dm-verity verifies a block on read), so it
    # can slip through the merge. Instead corrupt the verity-SIGNATURE partition: the PKCS#7 roothash
    # signature is checked at device setup (against the kernel keyring / /usr/lib/verity.d cert) BEFORE
    # any data is read, so a bad signature is refused every time. Self-locate the sig partition offset.
    if [ "$MODE" = "tamper" ]; then
      SIG_SECTOR=$(sfdisk -d "$HELLO" | grep -a verity-sig | grep -aoE "start=[[:space:]]*[0-9]+" | grep -aoE "[0-9]+")
      OFF=$(( SIG_SECTOR * 512 + 16 ))
      echo "--- MODE=tamper: corrupting verity signature at sector ${SIG_SECTOR} (offset ${OFF}) of ${HELLO} ---"
      printf "\xff\xff\xff\xff\xff\xff\xff\xff" | dd of="$HELLO" bs=1 seek="$OFF" count=8 conv=notrunc status=none
    fi

    # --- assemble the layer store: extensions/ (sysext) + confexts/ (confext, good mode only) ---
    rm -rf out/store-stage; mkdir -p out/store-stage/extensions out/store-stage/confexts
    cp "$HELLO" out/store-stage/extensions/shrek-hello.raw
    if [ "$MODE" = "select" ] || [ "$MODE" = "inject" ]; then
      cp "$(ls out/layers/shrek-extra*.raw | head -1)" out/store-stage/extensions/shrek-extra.raw
    fi
    if [ "$MODE" = "good" ] || [ "$MODE" = "select" ] || [ "$MODE" = "inject" ] || [ "$MODE" = "desktop" ]; then
      cp "$(ls out/layers/shrek-conf*.raw | head -1)" out/store-stage/confexts/shrek-conf.raw
    fi
    # desktop: stage the pre-built signed shrek-desktop sysext (scripts/build-desktop-layer.sh) beside
    # shrek-hello. onion-policy enables it → the broker merges it → shrek-desktop-gate.service proves it.
    if [ "$MODE" = "desktop" ]; then
      cp "$(ls out/layers/shrek-desktop*.raw | head -1)" out/store-stage/extensions/shrek-desktop.raw
      # Dogfood-0 M2: also stage the shrek-dev toolchain sysext WHEN it was built (scripts/build-dev-
      # layer.sh). Optional — a plain desktop store omits it and the sealed onion-policy just does not
      # merge the listed-but-absent layer. When present, oniond merges the toolchain onto /usr.
      if ls out/layers/shrek-dev*.raw >/dev/null 2>&1; then
        cp "$(ls out/layers/shrek-dev*.raw | head -1)" out/store-stage/extensions/shrek-dev.raw
        echo "--- staged shrek-dev toolchain sysext into the store ---"
      fi
    fi
    # inject: drop the compromised-brain marker oniond reads off the (untrusted) store. World-readable
    # so the unprivileged oniond can read it from the ro mount. The wall (gatekeeperd) must still refuse.
    if [ "$MODE" = "inject" ]; then
      echo "shrek-extra" > out/store-stage/oniond-inject
      chmod 0644 out/store-stage/oniond-inject
    fi
    rm -f out/layer-store.raw
    # 256M fits the ~1MB marker layers; the desktop sysext (Qt6 + Sway + Mesa closure) is far bigger,
    # so size the store from the staged bytes (2x + headroom) when it is present.
    STORE_MB=256
    if [ "$MODE" = "desktop" ]; then STORE_MB=$(( $(du -sm out/store-stage | cut -f1) * 2 + 128 )); fi
    mkfs.ext4 -q -L shrek-layers -d out/store-stage out/layer-store.raw "${STORE_MB}M"
    chown -R "${HOST_UID}:${HOST_GID}" out
  '
echo "=== layer store ready: out/layer-store.raw (label shrek-layers, mode=${MODE}) ==="
echo "    boot it against the Phase-2 root with:"
echo "      STORE=out/layer-store.raw RAW=out/shrek_1_x86-64.raw scripts/boot-vm.sh"
