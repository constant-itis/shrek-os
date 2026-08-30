#!/usr/bin/env bash
# Build the offline "Scratch" Bench seed (ADR-003 Part 2 step 6) — the base image a Bench actually works
# in. Produces a podman-save OCI-archive baked into the shrek-bench sysext at
# /usr/share/shrek/bench/seeds/scratch.tar, loaded on the sealed boot via `podman load` (the empirically
# chosen delivery — the step-6 de-risk proved `podman load` green end-to-end on native rootless overlay,
# while `additionalimagestores` under the already-overlayed merged /usr risks the kernel's overlay
# stacking-depth-2 limit; `podman load` reads a plain file + writes layers to the /home graphroot on ext4,
# a depth-1 mount). OCI-archive (not docker-archive) keeps the sysext payload ~52M, not ~133M.
#
# Seed contents = a REAL minimal base: alpine (musl, tiny) + coreutils + ffmpeg (the step-8 media
# north-star: an offline video convert in a Bench) + the exit42 helper (the rule-2 proof binary — a real
# static ELF that execs off the fresh overlay superblock and exits 42, baked in so the seed IS the proof
# image). Tagged localhost/scratch — the bench_plane.rs default (SHREK_BENCH_SEED); a fully-qualified
# offline name resolved against the local store only (registries.conf unqualified-search is empty).
#
# Reproducible for a SIGNED sysext: the base is pinned by DIGEST and every apk by exact version-release, so
# two builds of one commit yield the same image (bump the pins below via a deliberate commit). The build
# needs network (pull + apk) exactly like every other layer's apt; the RESULT runs fully offline.
#
# Output (both GITIGNORED, rebuilt on demand — a 52M artifact does not belong in git history, same posture
# as the source-built quickshell binaries): scratch.tar + scratch.tar.digest (the loaded image Id, the
# staleness key bench_plane's ensure_seed() compares against so an OS-shipped seed update re-loads).
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

# --- pins (bump deliberately; resolve replacements with `apk policy <pkg>` in the alpine image) ----------
ALPINE_DIGEST="sha256:c64c687cbea9300178b30c95835354e34c4e4febc4badfe27102879de0483b5e" # alpine:3.20
COREUTILS_VER="9.5-r2"
FFMPEG_VER="6.1.1-r8"

SEED_DIR="layers/shrek-bench/overlay/usr/share/shrek/bench/seeds"
ELF="$SEED_DIR/exit42.elf"
OUT="$SEED_DIR/scratch.tar"
[ -s "$ELF" ] || { echo "missing $ELF (the exit42 helper — the rule-2 proof ELF, committed)" >&2; exit 1; }

echo "=== building the Scratch Bench seed (alpine@${ALPINE_DIGEST%%:*}… + coreutils=$COREUTILS_VER + ffmpeg=$FFMPEG_VER + exit42) ==="
docker run --rm --privileged \
  -v "$REPO_ROOT:/work" -w /work \
  -e ALPINE_DIGEST="$ALPINE_DIGEST" -e COREUTILS_VER="$COREUTILS_VER" -e FFMPEG_VER="$FFMPEG_VER" \
  -e SEED_OUT="/work/$OUT" -e SEED_ELF="/work/$ELF" \
  -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
  debian:trixie bash -euo pipefail -c '
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq >/dev/null
    apt-get install -y --no-install-recommends -qq podman uidmap crun ca-certificates coreutils >/dev/null
    # vfs everywhere in this build container: docker-on-overlayfs cannot back podman'\''s native overlay
    # driver and this image has no fuse-overlayfs. vfs needs no overlay and the OCI-archive it emits is
    # driver-independent — the sealed system loads it under its own (native overlay) driver.
    mkdir -p /etc/containers
    printf "[storage]\ndriver = \"vfs\"\ngraphroot = \"/var/lib/containers/storage\"\nrunroot = \"/run/containers/storage\"\n" > /etc/containers/storage.conf

    ctx=/tmp/seedctx; mkdir -p "$ctx"
    install -m0755 "$SEED_ELF" "$ctx/exit42"
    cat > "$ctx/Containerfile" <<EOF
FROM docker.io/library/alpine@${ALPINE_DIGEST}
RUN apk add --no-cache coreutils=${COREUTILS_VER} ffmpeg=${FFMPEG_VER}
COPY exit42 /usr/local/bin/exit42
EOF
    echo "--- building localhost/scratch ---"
    # --network=host for the RUN (apk) step: use the container host netns so the build needs no netavark/nft
    # (this affects the BUILD only; the shipped seed image carries no network config).
    podman build -q --network=host -t localhost/scratch -f "$ctx/Containerfile" "$ctx"
    IMG_ID="$(podman image inspect localhost/scratch --format "{{.Id}}")"
    echo "    image id: $IMG_ID"

    echo "--- saving OCI-archive -> $SEED_OUT ---"
    rm -f "$SEED_OUT"
    podman save --format oci-archive -o "$SEED_OUT" localhost/scratch
    printf "%s\n" "$IMG_ID" > "${SEED_OUT}.digest"

    # SELF-CHECK: a fresh store must `podman load` this archive AND restore the localhost/scratch tag at the
    # SAME image id (this is exactly what ensure_seed() does on the sealed boot — fail the build if it wont).
    echo "--- self-check: load the archive into a fresh store, tag + id must round-trip ---"
    # vfs for the check store: it only loads + inspects (no exec), and vfs needs no overlay backing, so the
    # round-trip is validated even inside docker-on-overlayfs where a fresh overlay store cannot mount.
    CHK=/tmp/chkroot
    podman --root "$CHK" --runroot /tmp/chkrun --storage-driver vfs load -i "$SEED_OUT" >/dev/null 2>&1
    LID="$(podman --root "$CHK" --runroot /tmp/chkrun --storage-driver vfs image inspect localhost/scratch --format "{{.Id}}" 2>/dev/null || echo MISSING)"
    [ "$LID" = "$IMG_ID" ] && echo "    OK: podman load restores localhost/scratch @ $LID" \
      || { echo "    FAIL: load did not restore localhost/scratch (got $LID)"; exit 1; }

    chown "$HOST_UID:$HOST_GID" "$SEED_OUT" "${SEED_OUT}.digest"
    echo "--- seed artifacts ---"; ls -l "$SEED_OUT" "${SEED_OUT}.digest"
  '
echo "done. size: $(du -h "$OUT" | cut -f1). next: scripts/build-bench-layer.sh bakes it into the signed shrek-bench sysext."
