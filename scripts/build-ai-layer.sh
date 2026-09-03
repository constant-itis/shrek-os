#!/usr/bin/env bash
# Build the signed shrek-ai sysext DDI (ADR-006 M1 slice 1, docs/adr-006-slice1-onion-skeleton.md) — the
# SKELETON of the optional on-device AI layer. Runs mkosi in an ephemeral --privileged debian:trixie
# container (same idiom as scripts/build-dev-layer.sh / scripts/build-bench-layer.sh); the build host
# stays untouched. Reuses the throwaway Shrek key from scripts/build-in-container.sh.
#
# As of slice 4 the Onion carries the python3 RUNTIME (the on-box Shrek Memory API service is stdlib
# python3), so — like build-dev-layer.sh / build-bench-layer.sh — it builds with `--base-tree <sealed base>
# --overlay` so only the genuinely-new python files land in the DDI (mkosi refuses Packages= in a sysext
# without a base tree). NO pip closure ships (owner decision: no FastMCP/pydantic/uvicorn); just stdlib
# python3 + libsqlite3 (FTS5). The inference-server binary + shell arrive in later slices.
#
# Produces out/layers/shrek-ai*.raw; scripts/build-layers.sh stages it into the store when INCLUDE_AI=1 and
# the sealed onion-policy (`enable shrek-ai`) makes oniond merge it. A box built without INCLUDE_AI=1 simply
# never carries the layer — listed-but-absent is a clean no-op (same tolerance as shrek-dev/shrek-bench).
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"
HOST_UID="$(id -u)"; HOST_GID="$(id -g)"
[ -s keys/secureboot.key ] && [ -s keys/secureboot.crt ] || {
  echo "missing keys/secureboot.{key,crt} — run scripts/build-in-container.sh once first" >&2; exit 1; }

# Assemble the hardened mycolink-shell derivative into the (git-ignored) Onion overlay BEFORE mkosi, so
# ExtraTrees=overlay ships it. Vendors the pinned commit, removes every host-exec/escalation/dispatch/
# process-spawn primitive, adds the Shrek adapters, and fails the build on any surviving exec site or a
# hardened-tree digest mismatch (ADR-006 §3/§6). Skippable for a shell-less spike via SKIP_SHELL=1.
if [ "${SKIP_SHELL:-0}" != "1" ]; then
  scripts/vendor-agent-harness.sh
else
  echo "=== SKIP_SHELL=1 — not vendoring mycolink-shell (shell-less shrek-ai build) ==="
fi

# Slice 6: build the sealed llama.cpp inference runtime FROM the pinned immutable commit and stage it into
# the same (git-ignored) overlay (ADR-006 §3, docs/adr-006-slice6-dogfood.md). ExtraTrees=overlay ships it
# at /usr/lib/shrek/ai/llama-server (where shrek-ai-model launches it). Skippable for a runtime-less spike
# via SKIP_LLAMA=1 (the layer then stays model-dormant — shrek-ai-model exits 1 "inference runtime missing").
if [ "${SKIP_LLAMA:-0}" != "1" ]; then
  scripts/vendor-llama-server.sh
else
  echo "=== SKIP_LLAMA=1 — not vendoring llama-server (runtime-less shrek-ai build) ==="
fi

# Slice 6 (DOGFOOD only): bake the per-box model-as-data descriptor so the acceptance VM has a CONFIGURED
# model (an unconfigured box ships NO *.gguf.digest and stays dormant by design — README/reference-model).
# The descriptor is `sha256sum`-shaped: "<64-hex>  <name>.gguf". Git-ignored, staged only under DOGFOOD —
# exactly the gating idiom build-in-container.sh uses for the DOGFOOD home.mount / owner seed. The matching
# multi-GB GGUF is delivered to the VM's /home out-of-band by scripts/dogfood-vm.sh (never sealed).
MODEL_SEAL_DIR="layers/shrek-ai/overlay/usr/share/shrek/ai/model"
if [ "${DOGFOOD:-0}" = "1" ]; then
  GGUF_SRC="${SHREK_AI_GGUF:-out/ai-model/granite-4.2-3b-Q4_K_M.gguf}"
  [ -f "$GGUF_SRC" ] || { echo "build-ai-layer: DOGFOOD needs the GGUF at $GGUF_SRC (set SHREK_AI_GGUF)" >&2; exit 1; }
  gname="$(basename "$GGUF_SRC")"
  ghash="$(sha256sum "$GGUF_SRC" | cut -d' ' -f1)"
  printf '%s  %s\n' "$ghash" "$gname" > "$MODEL_SEAL_DIR/${gname}.digest"
  echo "=== DOGFOOD: baked model descriptor $MODEL_SEAL_DIR/${gname}.digest ($ghash) ==="
else
  # normal build: ensure no stray DOGFOOD descriptor leaks into a product Onion (dormant-by-default).
  rm -f "$MODEL_SEAL_DIR"/*.gguf.digest 2>/dev/null || true
fi

echo "=== building shrek-ai sysext (python3 runtime + AI overlay) in debian:trixie ==="
mkdir -p out/layers out/mkosi-vartmp
# Bind-mount a host ext4 dir OVER /var/tmp so mkosi's overlayfs workspace lands on real ext4, not docker's
# overlay2 (overlayfs-on-overlayfs mounts EINVAL). Same guard as build-dev-layer.sh / build-bench-layer.sh.
docker run --rm --privileged \
  -v "${REPO_ROOT}:/work" -w /work \
  -v "${REPO_ROOT}/out/mkosi-vartmp:/var/tmp" \
  -e HOST_UID="${HOST_UID}" -e HOST_GID="${HOST_GID}" \
  debian:trixie \
  bash -euo pipefail -c '
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq >/dev/null
    apt-get install -y --no-install-recommends -qq \
      ca-certificates openssl \
      mkosi systemd-ukify erofs-utils squashfs-tools dosfstools e2fsprogs systemd fdisk >/dev/null

    # (1) base tree = the SEALED BASE runtime closure, so the --overlay delta contains only the NEW python3
    #     files (mkosi 25.3 refuses Packages= in a sysext without a base tree). Mirrors build-dev-layer.sh.
    echo "=== building base tree (sealed-base runtime closure) for the overlay delta ==="
    mkosi -d debian -r trixie -t directory --output basetree --output-dir /work/out/dt-out-ai --force \
      -p systemd -p udev -p dbus-broker -p libcryptsetup12 -p systemd-resolved -p systemd-container \
      -p ca-certificates -p apparmor -p apparmor-utils -p less -p iproute2 -p nftables -p e2fsprogs \
      build

    # (2) signed sysext DDI as an OVERLAY on the base tree — emits only the python3 delta. Keys on the CLI.
    SIGN="--verity=yes --verity-key=/work/keys/secureboot.key --verity-certificate=/work/keys/secureboot.crt"
    cd /work/layers/shrek-ai
    mkosi --force $SIGN --base-tree /work/out/dt-out-ai/basetree --overlay build
    cd /work
    rm -rf /work/out/dt-out-ai; rm -rf /var/tmp/* 2>/dev/null || true
    echo "--- built shrek-ai layer artifact ---"; ls -l out/layers/shrek-ai* 2>/dev/null || true
    chown -R "$HOST_UID:$HOST_GID" out/layers out/mkosi-vartmp 2>/dev/null || true
  '
rmdir out/mkosi-vartmp 2>/dev/null || true
echo "done. next: INCLUDE_AI=1 scripts/build-layers.sh desktop assembles shrek-ai into the store (if present),"
echo "      then boot — the sealed onion-policy enables it and oniond merges the AI-layer skeleton onto /usr."
