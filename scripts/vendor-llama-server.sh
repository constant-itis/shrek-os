#!/usr/bin/env bash
# vendor-llama-server.sh — build the sealed llama.cpp inference runtime FROM the pinned immutable source
# commit and stage it into the shrek-ai Onion overlay (ADR-006 §3, docs/adr-006-slice6-dogfood.md). Run on
# the HOST by scripts/build-ai-layer.sh before mkosi, so ExtraTrees=overlay ships the binary.
#
#   pinned commit (layers/shrek-ai/llama-server.pin)
#     -> build   (debian:trixie container, CPU-only, GGML_NATIVE=OFF + static internal libs + curl off)
#     -> strip   (drop symbols; smaller + more deterministic)
#     -> allowlist (readelf NEEDED must be a subset of LDD_ALLOWLIST — proves the sealed base can run it)
#     -> digest  (verify against BINARY_SHA256, or bake it with REBUILD_PIN=1)
#     -> stage   (layers/shrek-ai/overlay/usr/lib/shrek/ai/llama-server, mode 0755)
#
# A prebuilt release binary is deliberately NOT used: it would be an external opaque trust root inside a
# signed offline Onion. Building from a pinned commit keeps the runtime auditable + reproducible.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

PIN="layers/shrek-ai/llama-server.pin"
DEST="layers/shrek-ai/overlay/usr/lib/shrek/ai/llama-server"
[ -f "$PIN" ] || { echo "vendor-llama-server: missing pin $PIN" >&2; exit 1; }

# --- read the pin (KEY=VALUE lines; values may contain spaces, so cut at the first '=') ---
pinval() { grep "^$1=" "$PIN" | head -n1 | cut -d= -f2- ; }
SOURCE_REPO="$(pinval SOURCE_REPO)"
SOURCE_COMMIT="$(pinval SOURCE_COMMIT)"
BUILD_FLAGS="$(pinval BUILD_FLAGS)"
LINKER_FLAGS="$(pinval LINKER_FLAGS)"
BUILD_TARGET="$(pinval BUILD_TARGET)"
LDD_ALLOWLIST="$(pinval LDD_ALLOWLIST)"
WANT_SHA="$(pinval BINARY_SHA256)"
[ -n "$SOURCE_REPO" ] && [ -n "$SOURCE_COMMIT" ] && [ -n "$BUILD_TARGET" ] || {
  echo "vendor-llama-server: pin $PIN is missing SOURCE_REPO/SOURCE_COMMIT/BUILD_TARGET" >&2; exit 1; }

HOST_UID="$(id -u)"; HOST_GID="$(id -g)"
mkdir -p out
# Reuse an existing checkout at the pinned commit if provided (fast iteration); else fetch the exact commit.
SRC="${LLAMA_SRC:-out/llama.cpp-vendor-src}"
if [ -d "$SRC/.git" ] && [ "$(git -C "$SRC" rev-parse HEAD 2>/dev/null || true)" = "$SOURCE_COMMIT" ]; then
  echo "=== reusing checkout at pinned commit: $SRC ($SOURCE_COMMIT) ==="
else
  echo "=== fetching ${SOURCE_REPO}@${SOURCE_COMMIT} into $SRC ==="
  rm -rf "$SRC"; mkdir -p "$SRC"
  git -C "$SRC" init -q
  git -C "$SRC" remote add origin "https://github.com/${SOURCE_REPO}.git"
  git -C "$SRC" fetch -q --depth 1 origin "$SOURCE_COMMIT"
  git -C "$SRC" checkout -q FETCH_HEAD
fi
GOT_COMMIT="$(git -C "$SRC" rev-parse HEAD)"
[ "$GOT_COMMIT" = "$SOURCE_COMMIT" ] || { echo "vendor-llama-server: checkout $GOT_COMMIT != pinned $SOURCE_COMMIT" >&2; exit 1; }

echo "=== building $BUILD_TARGET (CPU-only, static internal libs) in debian:trixie ==="
mkdir -p out/llama-build
docker run --rm \
  -v "${REPO_ROOT}:/work" -w /work \
  -e HOST_UID="${HOST_UID}" -e HOST_GID="${HOST_GID}" \
  -e SRC="$SRC" -e BUILD_FLAGS="$BUILD_FLAGS" -e LINKER_FLAGS="$LINKER_FLAGS" -e BUILD_TARGET="$BUILD_TARGET" \
  debian:trixie \
  bash -euo pipefail -c '
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq >/dev/null
    apt-get install -y --no-install-recommends -qq \
      build-essential cmake git ca-certificates libstdc++-14-dev >/dev/null
    rm -rf /work/out/llama-build; mkdir -p /work/out/llama-build
    cmake -S "/work/$SRC" -B /work/out/llama-build \
      $BUILD_FLAGS -DCMAKE_EXE_LINKER_FLAGS="$LINKER_FLAGS"
    cmake --build /work/out/llama-build --target "$BUILD_TARGET" -j "$(nproc)"
    bin="$(find /work/out/llama-build -type f -name "$BUILD_TARGET" -perm -u+x | head -n1)"
    [ -n "$bin" ] || { echo "vendor-llama-server: built $BUILD_TARGET not found" >&2; exit 1; }
    strip --strip-unneeded "$bin" || true
    cp "$bin" /work/out/llama-server.built
    chown "$HOST_UID:$HOST_GID" /work/out/llama-server.built
  '
BUILT="out/llama-server.built"
[ -s "$BUILT" ] || { echo "vendor-llama-server: build produced no binary" >&2; exit 1; }

# --- runtime shared-object allowlist: every NEEDED lib must be in the base's guaranteed closure ---
echo "=== runtime shared-object allowlist check ==="
needed="$(readelf -d "$BUILT" | awk -F'[][]' '/NEEDED/{print $2}')"
echo "NEEDED: $(echo "$needed" | tr '\n' ' ')"
bad=0
for lib in $needed; do
  case " $LDD_ALLOWLIST " in
    *" $lib "*) : ;;
    *) echo "vendor-llama-server: NEEDED '$lib' is NOT in the sealed-base allowlist ($LDD_ALLOWLIST)" >&2; bad=1 ;;
  esac
done
[ "$bad" = 0 ] || { echo "vendor-llama-server: runtime closure escapes the sealed base — refusing" >&2; exit 1; }
echo "allowlist OK — binary runs against base-guaranteed libs only"

# --- digest pin: verify against the recorded sha, or bake it with REBUILD_PIN=1 ---
GOT_SHA="$(sha256sum "$BUILT" | cut -d' ' -f1)"
echo "built llama-server sha256: $GOT_SHA ($(du -h "$BUILT" | cut -f1))"
if [ -z "$WANT_SHA" ] || [ "${REBUILD_PIN:-0}" = "1" ]; then
  # bake the recorded digest into the pin (first vendoring, or an intentional source/flag change)
  if grep -q '^BINARY_SHA256=' "$PIN"; then
    sed -i "s|^BINARY_SHA256=.*|BINARY_SHA256=$GOT_SHA|" "$PIN"
  else
    printf 'BINARY_SHA256=%s\n' "$GOT_SHA" >> "$PIN"
  fi
  echo "=== baked BINARY_SHA256=$GOT_SHA into $PIN ==="
elif [ "$GOT_SHA" != "$WANT_SHA" ]; then
  echo "vendor-llama-server: BINARY DIGEST MISMATCH" >&2
  echo "  got  $GOT_SHA" >&2
  echo "  want $WANT_SHA  (rebuild intentionally with REBUILD_PIN=1 if SOURCE_COMMIT/BUILD_FLAGS changed)" >&2
  exit 1
else
  echo "binary digest matches pin ($WANT_SHA)"
fi

# --- stage into the Onion overlay (0755 — it is an ExecStart target; #3014 exec-bit lesson) ---
mkdir -p "$(dirname "$DEST")"
install -m 0755 "$BUILT" "$DEST"
echo "=== staged llama-server -> $DEST (0755, sha256 $GOT_SHA) ==="
