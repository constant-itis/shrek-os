#!/usr/bin/env bash
# Build the GPLv3 Caelestia.Blobs QML module used by the Shrek Shell port.
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

HOST_UID="$(id -u)"
HOST_GID="$(id -g)"
OUT_DIR="${OUT_DIR:-out/caelestia-blobs}"

mkdir -p "$OUT_DIR"

docker run --rm -v "${REPO_ROOT}:/work" -w /work \
  -e HOST_UID="${HOST_UID}" -e HOST_GID="${HOST_GID}" -e OUT_DIR="${OUT_DIR}" \
  debian:trixie bash -euo pipefail -c '
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq >/dev/null
    apt-get install -y --no-install-recommends -qq \
      ca-certificates cmake ninja-build build-essential \
      qt6-base-dev qt6-declarative-dev qt6-shadertools-dev >/dev/null

    cmake -S third_party/caelestia-shell -B "$OUT_DIR/build" -GNinja \
      -DCMAKE_BUILD_TYPE=Release \
      -DCMAKE_INSTALL_PREFIX=/usr \
      -DQT_QML_OUTPUT_DIRECTORY="/work/$OUT_DIR/qml" >/tmp/caelestia-blobs-cmake.log 2>&1 || {
        echo "CMAKE FAILED"
        tail -80 /tmp/caelestia-blobs-cmake.log
        exit 1
      }

    ninja -C "$OUT_DIR/build" >/tmp/caelestia-blobs-ninja.log 2>&1 || {
      echo "BUILD FAILED"
      tail -80 /tmp/caelestia-blobs-ninja.log
      exit 1
    }

    chown -R "$HOST_UID:$HOST_GID" "$OUT_DIR"
  '

echo "CAELESTIA-BLOBS: built $OUT_DIR/qml/Caelestia/Blobs"
