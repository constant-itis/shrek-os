#!/usr/bin/env bash
# In-docker half of build-workshop-seed.sh (kept in its OWN file, not a `bash -c '…'` string, to avoid the
# nested-quoting landmine). Fixed content — all variables come from the env the outer script exports
# ($SEED_OUT, $CTX, $HOST_UID, $HOST_GID). Builds localhost/debian from the pre-generated $CTX, saves the
# OCI-archive, and self-checks the load round-trip + the runtime sources.
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null
apt-get install -y --no-install-recommends -qq podman uidmap crun ca-certificates >/dev/null

# vfs everywhere in this build container: docker-on-overlayfs cannot back podman's native overlay driver and
# this image has no fuse-overlayfs. The emitted OCI-archive is driver-independent — the sealed system loads
# it under its own native-overlay driver.
mkdir -p /etc/containers
printf '[storage]\ndriver = "vfs"\ngraphroot = "/var/lib/containers/storage"\nrunroot = "/run/containers/storage"\n' > /etc/containers/storage.conf

echo "--- building localhost/debian from $CTX ---"
# --network=host for the RUN (apt) step: use the container host netns so the build needs no netavark/nft
# (affects the BUILD only; the shipped seed image carries no network config).
podman build -q --network=host -t localhost/debian -f "$CTX/Containerfile" "$CTX"
IMG_ID="$(podman image inspect localhost/debian --format '{{.Id}}')"
echo "    image id: $IMG_ID"

echo "--- saving OCI-archive -> $SEED_OUT ---"
rm -f "$SEED_OUT"
podman save --format oci-archive -o "$SEED_OUT" localhost/debian
printf '%s\n' "$IMG_ID" > "${SEED_OUT}.digest"

# SELF-CHECK: a fresh store must `podman load` this archive AND restore the localhost/debian tag at the SAME
# image id — exactly what ensure_seed() does on the sealed boot. (The runtime-sources sanity is asserted at
# BUILD time inside the Containerfile RUN, where the build store works — a post-save `podman run` in the
# nested vfs check-store is unreliable, so we do NOT run the container here, mirroring the scratch seed.)
echo "--- self-check: load into a fresh store, tag + id round-trip ---"
podman --root /tmp/chkroot --runroot /tmp/chkrun --storage-driver vfs load -i "$SEED_OUT" >/dev/null 2>&1
LID="$(podman --root /tmp/chkroot --runroot /tmp/chkrun --storage-driver vfs image inspect localhost/debian --format '{{.Id}}' 2>/dev/null || echo MISSING)"
[ "$LID" = "$IMG_ID" ] || { echo "    FAIL: load did not restore localhost/debian (got $LID)"; exit 1; }
echo "    OK: podman load restores localhost/debian @ $LID"

chown "$HOST_UID:$HOST_GID" "$SEED_OUT" "${SEED_OUT}.digest"
echo "--- seed artifacts ---"; ls -l "$SEED_OUT" "${SEED_OUT}.digest"
