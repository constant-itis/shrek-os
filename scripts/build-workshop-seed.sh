#!/usr/bin/env bash
# Build the Debian "workshop" Bench seed (ADR-002 mutable-compute plane) — the glibc base a Bench runs in
# to `apt-get install` real tooling over the sealed `debian-apt` egress profile, WITHOUT touching the
# sealed host /usr. Sibling of the tiny Alpine `scratch` seed (scripts/build-bench-seed.sh); produces a
# second OCI-archive baked into the SAME shrek-bench sysext at /usr/share/shrek/bench/seeds/debian.tar,
# `podman load`ed on demand by bench_plane's ensure_seed() (digest-keyed staleness), selected per-bench
# via `shrek bench create <name> --seed debian`.
#
# RUNTIME sources = deb822 pointing EVERY suite (trixie, -updates, -security) at https://deb.debian.org —
# ONE host, matching the one-rule sealed `debian-apt` egress profile (deb.debian.org fronts the security
# archive too; security.debian.org is a separately-rotating round-robin, deliberately NOT reachable). HTTPS
# because a plaintext :80 to a shared-CDN IP allow-list would let a `Host:` header reach any Fastly site.
# apt's built-in https method + ca-certificates are all it needs; /var/lib/apt/lists is PURGED (first-run
# `apt-get update` in the bench is the point, and it saves ~40M).
#
# REPRODUCIBILITY (before this seed is SIGNED): pin BOTH the base by digest (DEBIAN_DIGEST) AND build-time
# apt at a snapshot.debian.org timestamp (SNAPSHOT_TS) — apt version pins rot (point releases drop old
# versions from the live archive), so snapshot is the only reproducible source. snapshot is a BUILD-HOST
# concern ONLY; the baked RUNTIME sources always point at deb.debian.org (never snapshot — it is not in the
# sealed egress profile). Both pins are OPTIONAL here so the workshop is provable today from the live
# archive; set them (and commit the values) before the seed rides a signed sysext.
#
# The Containerfile + apt sources are generated on the HOST (plain heredocs); the in-docker step is a fixed
# podman build/save/self-check with NO interpolated shell — avoids the nested-quoting-in-`bash -c` landmine.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

# --- pins (set + commit before signing; empty = build from the live archive, non-reproducible) ----------
DEBIAN_DIGEST="${DEBIAN_DIGEST:-}"      # e.g. sha256:… — pin debian:trixie by digest
SNAPSHOT_TS="${SNAPSHOT_TS:-}"          # e.g. 20260815T000000Z — snapshot.debian.org archive timestamp
BASE="debian:trixie${DEBIAN_DIGEST:+@$DEBIAN_DIGEST}"

SEED_DIR="layers/shrek-bench/overlay/usr/share/shrek/bench/seeds"
OUT="$SEED_DIR/debian.tar"
CTX="out/workshop-seedctx"              # gitignored build context (out/ is a build dir)
mkdir -p "$SEED_DIR" "$CTX"

# --- RUNTIME deb822 sources baked into the seed: every suite on ONE host over https (the debian-apt
#     egress profile). debian-archive-keyring ships in the base, so Signed-By resolves offline. -----------
cat > "$CTX/debian.sources" <<EOF
Types: deb
URIs: https://deb.debian.org/debian
Suites: trixie trixie-updates
Components: main
Signed-By: /usr/share/keyrings/debian-archive-keyring.gpg

Types: deb
URIs: https://deb.debian.org/debian-security
Suites: trixie-security
Components: main
Signed-By: /usr/share/keyrings/debian-archive-keyring.gpg
EOF

# --- Containerfile. When SNAPSHOT_TS is set, install ca-certificates from a pinned snapshot (reproducible);
#     else from the base image's default archive. Then swap in the runtime https sources + purge lists. ----
if [ -n "$SNAPSHOT_TS" ]; then
  cat > "$CTX/build.sources" <<EOF
Types: deb
URIs: http://snapshot.debian.org/archive/debian/${SNAPSHOT_TS}/
Suites: trixie
Components: main
Signed-By: /usr/share/keyrings/debian-archive-keyring.gpg
EOF
  cat > "$CTX/Containerfile" <<EOF
FROM ${BASE}
COPY build.sources /etc/apt/sources.list.d/debian.sources
COPY debian.sources /work-ctx/debian.sources
RUN rm -f /etc/apt/sources.list \\
 && printf 'Acquire::Check-Valid-Until "false";\\n' > /etc/apt/apt.conf.d/10snapshot \\
 && apt-get update -o Acquire::Retries=3 \\
 && apt-get install -y --no-install-recommends ca-certificates \\
 && rm -f /etc/apt/apt.conf.d/10snapshot \\
 && cp /work-ctx/debian.sources /etc/apt/sources.list.d/debian.sources \\
 && grep -q 'https://deb.debian.org/debian' /etc/apt/sources.list.d/debian.sources \\
 && rm -rf /var/lib/apt/lists/* /work-ctx
EOF
else
  cat > "$CTX/Containerfile" <<EOF
FROM ${BASE}
COPY debian.sources /work-ctx/debian.sources
RUN apt-get update -o Acquire::Retries=3 \\
 && apt-get install -y --no-install-recommends ca-certificates \\
 && rm -f /etc/apt/sources.list \\
 && cp /work-ctx/debian.sources /etc/apt/sources.list.d/debian.sources \\
 && grep -q 'https://deb.debian.org/debian' /etc/apt/sources.list.d/debian.sources \\
 && rm -rf /var/lib/apt/lists/* /work-ctx
EOF
fi

echo "=== building the Debian workshop Bench seed (${BASE}, snapshot=${SNAPSHOT_TS:-live}) ==="
docker run --rm --privileged \
  -v "$REPO_ROOT:/work" -w /work \
  -e SEED_OUT="/work/$OUT" -e CTX="/work/$CTX" \
  -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
  debian:trixie bash /work/scripts/build-workshop-seed-inner.sh
echo "done. size: $(du -h "$OUT" | cut -f1). next: scripts/build-bench-layer.sh bakes it into the signed shrek-bench sysext."
