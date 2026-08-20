#!/usr/bin/env bash
# Phase-5 slice-6 — assemble the T2 (gVisor) sealed runtime artifacts into an mkosi ExtraTree so
# `mkosi build` copies them into the dm-verity `/usr` at /usr/lib/shrek/{runsc,t2-rootfs} — the
# compiled-in PROD default paths gatekeeperd's t2_plane reads (crates/gatekeeperd/src/t2_plane.rs:
# sealed_runsc_path/sealed_rootfs_path). The SHREK_T2_* env overrides are ORACLE-ONLY; the sealed
# image has none, so the constructor reads only read-only, roothash-authenticated /usr — no writable
# authority source.
#
#   seal-t2-artifacts.sh <extra-tree-root> <verified-runsc-path>
#
# Invoked from scripts/build-in-container.sh STAGE 2 (inside the ephemeral debian:trixie container,
# where busybox-static is a clean apt install). The runsc is fetched + sha256-verified on the HOST in
# STAGE 1 (from image/supply/gvisor.pin, NEVER 'latest'); this script RE-verifies before sealing —
# an unverified binary must never enter the sealed image — then builds the minimal busybox rootfs.
set -euo pipefail

TREE="${1:?usage: seal-t2-artifacts.sh <extra-tree-root> <verified-runsc-path>}"
RUNSC_SRC="${2:?usage: seal-t2-artifacts.sh <extra-tree-root> <verified-runsc-path>}"

# Pinned identity — MUST equal the sha recorded in image/supply/gvisor.pin (drift-guarded) and the
# oracle (scripts/t2-construct-proof.sh). release-20260810.0, x86_64.
PIN_SHA256="670bcd3cbc103f00d8bb5098edc370f32397ee4c134231436bafa659bb3c068e"

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PIN="$REPO_ROOT/image/supply/gvisor.pin"
# Drift guard: the hash we are about to seal MUST be the one recorded in the pin manifest.
grep -q "$PIN_SHA256" "$PIN" || { echo "SEAL ABORT: $PIN_SHA256 not present in $PIN (pin drift)"; exit 1; }

# Re-verify the handed-off runsc before it enters the sealed /usr.
GOT="$(sha256sum "$RUNSC_SRC" | awk '{print $1}')"
[ "$GOT" = "$PIN_SHA256" ] || { echo "SEAL ABORT: runsc sha256 $GOT != pinned $PIN_SHA256"; exit 1; }

DEST="$TREE/usr/lib/shrek"
install -d "$DEST"
install -m0755 "$RUNSC_SRC" "$DEST/runsc"

# Minimal pinned sandbox rootfs = busybox-static + RELATIVE applet symlinks. Absolute links break
# inside the sandbox: `busybox --install -s` writes /rootfs/bin/busybox targets that do not exist at
# the sandbox root ("failed to load /bin/sh") — see scripts/t2-construct-proof.sh. The applet set
# matches the oracle rootfs so the sealed VM S5 gate exercises the identical userland.
BB="$(command -v busybox)" || { echo "SEAL ABORT: busybox-static not installed in build container"; exit 1; }
ROOTFS="$DEST/t2-rootfs"
rm -rf "$ROOTFS"
install -d "$ROOTFS/bin"
install -m0755 "$BB" "$ROOTFS/bin/busybox"
# cp + chmod added for Phase-6 slice-1a: the coding workload copies the source template into the writable
# grant and marks its build artifact (tcc leaves it +x already; chmod kept for an explicit edit step).
for a in sh cat ls nc timeout echo test cp chmod; do ln -sf busybox "$ROOTFS/bin/$a"; done

# --- Phase-6 slice-1a: seal a MINIMAL REAL C compiler (tcc) + its dynamic closure into the rootfs so a
#     T2 untrusted-ingest coding session can do a real edit → compile → execute loop. -nostdlib -static
#     needs NEITHER libtcc1.a NOR the tcc include dir (a freestanding raw-syscall _start program uses no
#     libc), so the footprint is just tcc + its own ELF interpreter + libc/libm (~3.4 MB). tcc has its own
#     internal linker — no external `ld` in the rootfs. NOT `tcc -run` (that is JIT/anon-exec = PN5). ---
TCC="$(command -v tcc)" || { echo "SEAL ABORT: tcc not installed in build container"; exit 1; }
install -D -m0755 "$TCC" "$ROOTFS/usr/bin/tcc"
# The PT_INTERP the tcc ELF names must exist at that exact path inside the rootfs; on x86_64 glibc it is
# /lib64/ld-linux-x86-64.so.2 (also emitted by ldd below, so this is belt-and-suspenders).
INTERP=/lib64/ld-linux-x86-64.so.2
[ -e "$INTERP" ] && install -D -m0755 "$INTERP" "$ROOTFS$INTERP"
# Every DT_NEEDED shared lib (+ the interpreter line) at its resolved real path. install dereferences, so
# a SONAME symlink lands as a regular file the loader finds by name in its default (no-cache) search path.
for so in $(ldd "$TCC" | grep -oE '/[^ ]+\.so[^ ]*'); do
  install -D -m0755 "$so" "$ROOTFS$so"
done

# --- Phase-6 slice-2: seal the CODER AGENT binary (the model-driven WORKLOAD of a `shrek run` T2
#     session, docs/phase6-slice2-coder-agent.md) + its dynamic closure into the rootfs, exactly like
#     tcc above. The coder is FIRST-PARTY code whose integrity comes from this seal (dm-verity /usr),
#     NOT from the ingest admit-list (that measures the runsc HARNESS, not the workload) — so runsc
#     stays the admitted harness and the coder rides in as a sealed rootfs tool. It is built HERMETICALLY
#     on the host (STAGE 1 `cargo build --release --offline`, tinyjson vendored in-tree) and lands at
#     target/release/coder. glibc-dynamic (musl-static one-inode is the documented next step): its ldd
#     closure adds libgcc_s.so.1 beyond tcc's libc/ld-linux — install each at its resolved real path. ---
CODER="$REPO_ROOT/target/release/coder"
[ -x "$CODER" ] || { echo "SEAL ABORT: coder release binary missing at $CODER (run cargo build --release --offline first)"; exit 1; }
install -D -m0755 "$CODER" "$ROOTFS/usr/bin/coder"
for so in $(ldd "$CODER" | grep -oE '/[^ ]+\.so[^ ]*'); do
  install -D -m0755 "$so" "$ROOTFS$so"
done

# The freestanding source template the workload copies into the writable project, then compiles. Kept in
# the rootfs (read-only /usr at runtime) so the workload authors a REAL .c into the mutable grant.
install -d "$ROOTFS/coder-src"
cat > "$ROOTFS/coder-src/hello.c" <<'CEOF'
/* Freestanding: no libc, raw x86-64 syscalls, own _start. Built with `tcc -nostdlib -static`.
   Proves a real compiler turned freshly-written, mutable project bytes into an executable ELF that
   the sandbox then runs — no libc needed in the guest rootfs. */
static long s(long n, long a, long b, long c) {
    long r;
    __asm__ volatile("syscall" : "=a"(r) : "a"(n), "D"(a), "S"(b), "d"(c) : "rcx", "r11", "memory");
    return r;
}
void _start(void) {
    const char m[] = "REAL-COMPILE-RUN-OK";
    s(1, 1, (long)m, sizeof(m) - 1); /* write(1, m, len) */
    s(60, 42, 0, 0);                 /* exit(42) — a distinctive code only a real compiled ELF produces */
}
CEOF

# --- Phase-6 slice-1a: bake the ingest admit-list = the fs-verity identity of THIS runsc. fs-verity
#     digest is content-addressed (sha256 over 4096-byte Merkle blocks), so this OFFLINE digest EQUALS the
#     runtime kernel FS_IOC_MEASURE_VERITY the P6 VM gate provisions on a loopback (offline bake == kernel
#     measure — the same property the pin-manifest bake relies on). A sealed image thus authenticates its
#     own harness with no writable authority source; gatekeeperd reads this compiled-in dm-verity path. ---
command -v fsverity >/dev/null 2>&1 || { echo "SEAL ABORT: fsverity (fsverity-utils) not in build container"; exit 1; }
ADMIT_HEX="$(fsverity digest --hash-alg=sha256 --block-size=4096 "$RUNSC_SRC" | cut -d: -f2 | cut -d' ' -f1)"
[ "${#ADMIT_HEX}" = 64 ] || { echo "SEAL ABORT: unexpected runsc fsverity digest [$ADMIT_HEX]"; exit 1; }
{
  printf 'shrek-t2-ingest-admit v1\n'
  printf '# authorised T2 untrusted-ingest harness: runsc %s (fs-verity sha256, offline bake == kernel measure)\n' "$PIN_SHA256"
  printf 'sha256 %s\n' "$ADMIT_HEX"
} > "$DEST/t2-ingest-admit"

echo "seal-t2-artifacts: runsc $(stat -c%s "$DEST/runsc") bytes (sha256 verified) + t2-rootfs ($(ls "$ROOTFS/bin" | wc -l) applets + tcc + coder $(stat -c%s "$ROOTFS/usr/bin/coder") bytes) + t2-ingest-admit (runsc fs-verity sha256 $ADMIT_HEX) -> $DEST"
