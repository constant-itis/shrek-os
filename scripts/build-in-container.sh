#!/usr/bin/env bash
# Shrek OS Phase-1 — build the image INSIDE an ephemeral debian:trixie container so the Pop!_OS
# host is never mutated (see docs/phase1-spike.md §0). Drafted by the local coder tier, reviewed
# on the primary model.
#
# SPIKE SCOPE: dm-verity (S4), UKI signing (S5), and the bootc wrap (S7) are NOT wired yet, so
# this currently produces an UNSEALED base image — enough to prove the trixie base + scaffold
# assemble and boot. Every `# VERIFY` is confirmed the first time this actually runs.
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

echo "=== STAGE 2 (container): mkosi build in throwaway debian:trixie ==="
# --privileged: mkosi needs loop devices to assemble a disk image. Ephemeral container, host untouched.
docker run --rm --privileged \
  -v "${REPO_ROOT}:/work" -w /work/image \
  -e HOST_UID="${HOST_UID}" -e HOST_GID="${HOST_GID}" \
  debian:trixie \
  bash -euo pipefail -c '
    apt-get update
    # VERIFY: exact package names on trixie. mkosi pulls most tooling via Recommends; ukify ships
    # in systemd-ukify; systemd-repart ships in the systemd package on Debian (not a separate pkg).
    apt-get install -y --no-install-recommends \
      mkosi systemd-ukify erofs-utils dosfstools mtools apparmor
    mkosi --force build      # VERIFY: subcommand/flags on the installed mkosi version
    chown -R "${HOST_UID}:${HOST_GID}" /work/out
  '

echo "=== done — artifacts in out/ (UNSEALED base image; S4/S5/S7 still to wire) ==="
