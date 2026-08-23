#!/usr/bin/env bash
# Build the signed shrek-desktop sysext DDI.
#
# DMS-BOOT-0 installs packaged DankMaterialShell and Debian backports Quickshell into the desktop layer.
# Core Shrek services, onion policy, gatekeeperd, agent runtime, and security boundaries stay untouched.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"
HOST_UID="$(id -u)"; HOST_GID="$(id -g)"
[ -s keys/secureboot.key ] && [ -s keys/secureboot.crt ] || {
  echo "missing keys/secureboot.{key,crt} — run scripts/build-in-container.sh once first" >&2; exit 1; }

APT_SANDBOX=out/dms-apt-sandbox

echo "=== building shrek-desktop sysext (DMS 1.5.3 + Debian backports Quickshell) in debian:trixie ==="
rm -rf "$APT_SANDBOX"
mkdir -p out/layers out/mkosi-vartmp \
  "$APT_SANDBOX/etc/apt/sources.list.d" \
  "$APT_SANDBOX/etc/apt/keyrings" \
  "$APT_SANDBOX/etc/apt/trusted.gpg.d"

curl -fsSL https://download.opensuse.org/repositories/home:/AvengeMedia:/dms/Debian_13/Release.key \
  | gpg --batch --yes --dearmor -o "$APT_SANDBOX/etc/apt/keyrings/avengemedia-dms.gpg"
curl -fsSL https://download.opensuse.org/repositories/home:/AvengeMedia:/danklinux/Debian_13/Release.key \
  | gpg --batch --yes --dearmor -o "$APT_SANDBOX/etc/apt/keyrings/danklinux.gpg"

cat > "$APT_SANDBOX/etc/apt/sources.list.d/trixie-backports.list" <<'EOF'
deb [signed-by=/usr/share/keyrings/debian-archive-keyring.gpg] http://deb.debian.org/debian trixie-backports main
EOF
cat > "$APT_SANDBOX/etc/apt/sources.list.d/avengemedia-dms.list" <<'EOF'
deb [signed-by=/etc/apt/keyrings/avengemedia-dms.gpg] https://download.opensuse.org/repositories/home:/AvengeMedia:/dms/Debian_13/ /
EOF
cat > "$APT_SANDBOX/etc/apt/sources.list.d/danklinux.list" <<'EOF'
deb [signed-by=/etc/apt/keyrings/danklinux.gpg] https://download.opensuse.org/repositories/home:/AvengeMedia:/danklinux/Debian_13/ /
EOF

docker run --rm --privileged \
  -v "${REPO_ROOT}:/work" -w /work \
  -v "${REPO_ROOT}/out/mkosi-vartmp:/var/tmp" \
  -e HOST_UID="${HOST_UID}" -e HOST_GID="${HOST_GID}" \
  debian:trixie \
  bash -euo pipefail -c '
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq >/dev/null
    apt-get install -y --no-install-recommends -qq \
      ca-certificates curl gpg openssl \
      mkosi systemd-ukify erofs-utils squashfs-tools dosfstools e2fsprogs systemd fdisk >/dev/null

    cd /work
    echo "=== building base tree (sealed-base runtime closure) for the overlay delta ==="
    mkosi -d debian -r trixie -t directory --output basetree --output-dir /work/out/dt-out --force \
      -p systemd -p udev -p dbus-broker -p libcryptsetup12 -p systemd-resolved -p systemd-container \
      -p ca-certificates -p apparmor -p apparmor-utils -p less -p iproute2 -p nftables -p e2fsprogs \
      build

    SIGN="--verity=yes --verity-key=/work/keys/secureboot.key --verity-certificate=/work/keys/secureboot.crt"
    cd /work/layers/shrek-desktop
    mkosi --force --sandbox-tree /work/out/dms-apt-sandbox:/ $SIGN \
      --base-tree /work/out/dt-out/basetree --overlay build
    cd /work

    rm -rf /work/out/dt-out; rm -rf /var/tmp/* 2>/dev/null || true
    echo "--- built desktop layer artifact ---"; ls -l out/layers/shrek-desktop* 2>/dev/null || true
    chown -R "$HOST_UID:$HOST_GID" out/layers out/mkosi-vartmp out/dms-apt-sandbox 2>/dev/null || true
  '

rmdir out/mkosi-vartmp 2>/dev/null || true
rm -rf "$APT_SANDBOX"
echo "done. next: assemble layer store with scripts/build-layers.sh desktop, then boot the DMS proof."
