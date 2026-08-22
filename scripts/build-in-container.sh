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
# Phase-6 slice-2: HERMETIC build. The coder's one dep (tinyjson) is vendored in-tree (vendor/ +
# .cargo/config.toml redirects crates-io to it); the sealed planes are dep-free. Force cargo OFFLINE for
# the whole stage so the sealed image can never silently pull an unaudited crate from the network — a
# build that would need crates.io fails loudly instead. (Removes the last non-hermetic seam in the seal.)
export CARGO_NET_OFFLINE=true
cargo build --release
# The sealed-VM gate (S6/S8) drives `gatekeeperd pin-verity` at runtime to provision fs-verity
# fixtures, so the IMAGE gatekeeperd is the spike build (finding F1: pin-verity is default-OFF). The
# whole gate is spike-only scaffolding on the pre-ship strip list; a production ship build omits both
# `--features spike` AND the gate, so the shipped gatekeeperd has no pin-verity surface.
cargo build --release -p gatekeeperd --features spike
install -d image/overlay/usr/libexec/shrek image/overlay/usr/share/doc/shrek
install -m0755 target/release/{swampd,agentd,gatekeeperd,oniond,shrekctl,shrek} image/overlay/usr/libexec/shrek/
# User-facing CLIs on PATH (Dogfood M2): `shrek` is the Phase-6 front door and `shrekctl` the operator
# CLI — both belong on PATH so the box behaves like a normal machine (owner hit `shrek: command not
# found`). The daemons stay in /usr/libexec/shrek (not PATH); shrek finds gatekeeperd/agentd via the
# SHREK_GATEKEEPERD/SHREK_AGENTD env set in etc/profile.d/10-shrek-env.sh.
install -d image/overlay/usr/bin
ln -sf ../libexec/shrek/shrek    image/overlay/usr/bin/shrek
ln -sf ../libexec/shrek/shrekctl image/overlay/usr/bin/shrekctl
# Phase-5 slice-7 (B1): the sealed closed-world in-sandbox acceptance probe (spike-only, strip before
# ship with the other gate scaffolding). Enrolled in gatekeeperd's compiled-in CLOSED_WORLD so it
# legitimately derives T-first on the sealed image (a shell cannot — B1 treats it as open-world).
# Phase-5 slice-9: rebuild it as a STATIC PIE (no PT_INTERP) so the S7 exec-island gate can RUN it as a
# pinned entrypoint (Fork A rejects a dynamically-linked pin). A static binary runs anywhere a dynamic
# one does, so S4/S6 are unaffected, and the S6 manifest bake measures whatever is installed here.
RUSTFLAGS="-C target-feature=+crt-static" cargo build --release -p shrek-gate-probe
install -m0755 target/release/gate-probe image/overlay/usr/libexec/shrek/gate-probe
install -m0644 docs/*.md image/overlay/usr/share/doc/shrek/

# Phase-5 slice-10 (S8 sealed-dynamic closure gate, spike — strip before ship with the other gate
# scaffolding). A DYNAMIC gate-probe carrying DT_RPATH $ORIGIN/lib, delivered with its FULL ldd closure
# (interpreter + transitive libs) so the sealed image can prove a pinned dynamic closure RUNS from the
# N-inode island (docs/phase5-slice10-sealed-dynamic.md §9). The closure is enumerated here on the host;
# STAGE 2 bakes each staged file's fs-verity digest into the v2 manifest (offline digest == runtime
# kernel measure, fs-verity being content-addressed). closure.meta carries the interp path + SONAMEs to
# STAGE 2. The probe runs against THESE delivered libs (self-contained), so a host/image libc skew is
# irrelevant. The static pin above still drives S6/S7 unchanged.
DYNPKG=image/overlay/usr/libexec/shrek/dynpkg
rm -rf "$DYNPKG"; install -d "$DYNPKG"
RUSTFLAGS='-C link-arg=-Wl,-rpath,$ORIGIN/lib -C link-arg=-Wl,--disable-new-dtags' cargo build --release -p shrek-gate-probe
install -m0755 target/release/gate-probe "$DYNPKG/dyn-probe"
DYN_INTERP=$(readelf -l "$DYNPKG/dyn-probe" | sed -n 's/.*program interpreter: \([^]]*\)\].*/\1/p')
[ -n "$DYN_INTERP" ] || { echo "S8 stage: dyn-probe has no PT_INTERP"; exit 1; }
install -m0755 "$DYN_INTERP" "$DYNPKG/$(basename "$DYN_INTERP")"
: > "$DYNPKG/closure.meta"
printf 'interp %s\n' "$DYN_INTERP" >> "$DYNPKG/closure.meta"
for L in $(ldd "$DYNPKG/dyn-probe" | awk '/=>/ && $3 ~ /^\//{print $3}'); do
  SO=$(basename "$L"); install -m0755 "$L" "$DYNPKG/$SO"; printf 'lib %s\n' "$SO" >> "$DYNPKG/closure.meta"
done
echo "--- staged S8 dynamic closure: interp=$DYN_INTERP libs=$(grep -c '^lib ' "$DYNPKG/closure.meta") ---"
# Restore target/release/gate-probe to the STATIC build (some later steps expect the sealed variant).
RUSTFLAGS="-C target-feature=+crt-static" cargo build --release -p shrek-gate-probe >/dev/null

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

# Dogfood-0 (M0): DOGFOOD=1 builds an INTERACTIVE image. The spike acceptance gates run late and then
# power the VM OFF (shrek-mount-gate owns SuccessAction/FailureAction=poweroff-force) — correct for the
# headless CI proof, fatal for a machine you want to sit at. Mask both the poweroff-owning mount gate and
# the now-redundant headless desktop proof (the interactive tty1 session IS the surface proof) via
# /etc/systemd/system/<unit> → /dev/null, which outranks the /usr unit. These masks are gitignored and
# REMOVED on a normal build, so the default image + scripts/desktop-sealed-proof.sh are byte-unchanged.
DOGFOOD_MASKS="image/overlay/etc/systemd/system"
# Dogfood-0 (M1): the persistent /home mount + the persistence/services acceptance probe are ENABLED
# only on DOGFOOD images — they assume the writable shrek-data disk (home.mount would otherwise wait on
# an absent device on a CI image) and the probe REBOOTS the guest (never wanted in CI). The unit FILES
# ship in the sealed overlay unconditionally; only these enable symlinks are DOGFOOD-gated. Gitignored,
# removed on a normal build — so the default CI image + scripts/desktop-sealed-proof.sh stay byte-clean.
LOCALFS_WANTS="image/overlay/usr/lib/systemd/system/local-fs.target.wants"
MU_WANTS="image/overlay/usr/lib/systemd/system/multi-user.target.wants"
if [ "${DOGFOOD:-0}" = "1" ]; then
  echo "!!! DOGFOOD=1: interactive image — masking self-poweroff spike gates (shrek-mount-gate, shrek-desktop-gate) !!!"
  install -d "$DOGFOOD_MASKS"
  ln -sf /dev/null "$DOGFOOD_MASKS/shrek-mount-gate.service"
  ln -sf /dev/null "$DOGFOOD_MASKS/shrek-desktop-gate.service"
  echo "!!! DOGFOOD=1: enabling persistent /home (home.mount) + the M1 persistence probe !!!"
  install -d "$LOCALFS_WANTS" "$MU_WANTS"
  ln -sf ../home.mount "$LOCALFS_WANTS/home.mount"
  ln -sf ../shrek-dogfood-persist.service "$MU_WANTS/shrek-dogfood-persist.service"
else
  rm -f "$DOGFOOD_MASKS/shrek-mount-gate.service" "$DOGFOOD_MASKS/shrek-desktop-gate.service"
  rm -f "$LOCALFS_WANTS/home.mount" "$MU_WANTS/shrek-dogfood-persist.service"
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
    # the T2 sandbox rootfs userland (Phase-5 slice-6 seal), static so it needs no in-rootfs libs. tcc =
    # the Phase-6 slice-1a freestanding C compiler sealed into the T2 rootfs (real edit/build/execute).
    apt-get install -y --no-install-recommends \
      mkosi systemd-ukify sbsigntool erofs-utils dosfstools mtools apparmor busybox-static fsverity tcc
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
    # Phase-5 slice-10 (S8): extend the bake to a v2 manifest that ALSO pins the sealed-dynamic closure —
    # the dynamic entrypoint (`entry`), its interpreter at its exact PT_INTERP path (`interp`), and every
    # transitive lib by SONAME (`lib`). The static pin (above) keeps S6/S7; the closure adds S8. Same
    # content-addressed fs-verity digest ⇒ this offline bake equals the runtime kernel measure (I10:
    # sealed manifest + runtime re-measure is authority; the host enumeration in STAGE 1 only GENERATES).
    DYNPKG_OVL=/work/image/overlay/usr/libexec/shrek/dynpkg
    fsv() { fsverity digest --hash-alg=sha256 --block-size=4096 "$1" | cut -d: -f2 | cut -d" " -f1; }
    {
      printf "shrek-pin-manifest v2\n"
      printf "sha256 %s closed-world\n" "$PIN_HEX"
      if [ -f "$DYNPKG_OVL/closure.meta" ]; then
        printf "entry sha256 %s closed-world\n" "$(fsv "$DYNPKG_OVL/dyn-probe")"
        while read -r kind arg; do
          case "$kind" in
            interp) printf "interp sha256 %s %s\n" "$(fsv "$DYNPKG_OVL/$(basename "$arg")")" "$arg" ;;
            lib)    printf "lib sha256 %s %s\n" "$(fsv "$DYNPKG_OVL/$arg")" "$arg" ;;
          esac
        done < "$DYNPKG_OVL/closure.meta"
      fi
    } > /work/image/overlay/usr/lib/shrek/pin-manifest
    echo "--- baked v2 pin-manifest (S6/S7 static pin + S8 dynamic closure) ---"
    cat /work/image/overlay/usr/lib/shrek/pin-manifest
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
