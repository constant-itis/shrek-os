#!/usr/bin/env bash
# publish-release.sh — cut a public GitHub Release of a Shrek OS build's UPDATE payload.
#
# GitHub is the transparency authority for updates (docs/adr-007 Q6b decision, 2026-09-04): every versioned
# build publishes its systemd-sysupdate payload — the split root partition, its dm-verity hash partition, and
# the Secure-Boot-signed UKI — plus a SHA256SUMS manifest, as a public, immutable Release. The updater
# (systemd-sysupdate) fetches these over https://updates.shrekos.iambu.dev/<channel>/ (an owner-controlled
# front over these releases), checksum-verifies, and A/B installs. Authority is the SB-signed UKI + the
# checksums, so the transport/front is untrusted plumbing — a mirror cannot ship a bootable tampered image.
#
# The monolithic out/shrek_<v>_x86-64.raw (fresh-install disk image, multi-GB) is NOT published here:
# GitHub caps a release asset at 2 GiB, and it is not an update artifact. Installer-image distribution is a
# separate concern.
#
#   scripts/publish-release.sh [VERSION] [--draft] [--repo OWNER/REPO] [--signing-key DIR] [--notes-only]
#     VERSION        build/version tag component (default: 1 -> release tag v1). Matches build-in-container.sh <v>.
#     --draft        create the release as a DRAFT (staged + uploaded, not public until you hit Publish).
#     --repo         target repo (default: constant-itis/shrek-os).
#     --signing-key  GNUPGHOME dir holding the update-signing PRIVATE key. Default: the vault location
#                    below (also settable via $UPDATE_GNUPGHOME). Publishing FAILS CLOSED if this is
#                    absent — a signed manifest is mandatory (systemd-sysupdate refuses an unsigned one).
#     --notes-only   print the resolved artifact set + notes and exit WITHOUT touching GitHub (dry run).
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

VERSION=1
DRAFT=0
NOTES_ONLY=0
REPO=constant-itis/shrek-os
GH_PUSH_USER=constant-itis          # the account that owns REPO
GH_RESTORE_USER=BuberryWorldwide    # the default active account to restore afterward
ARCH=x86-64
# The update-signing PRIVATE keyring lives OUTSIDE the checkout (the repo carries only the public half,
# keys/shrek-update-pub.gpg). Default home is the encrypted vault; override with --signing-key or
# $UPDATE_GNUPGHOME (e.g. after restoring from the offline backup to a different path).
SIGNING_KEY="${UPDATE_GNUPGHOME:-$HOME/vault/shrek-os/update-signing/gnupg}"

while [ $# -gt 0 ]; do
  case "$1" in
    --draft) DRAFT=1 ;;
    --notes-only) NOTES_ONLY=1 ;;
    --repo) shift; REPO="$1" ;;
    --signing-key) shift; SIGNING_KEY="$1" ;;
    -h|--help) sed -n '2,22p' "$0"; exit 0 ;;
    --*) echo "unknown flag: $1" >&2; exit 2 ;;
    *) VERSION="$1" ;;
  esac
  shift
done

BASE="out/shrek_${VERSION}_${ARCH}"
TAG="v${VERSION}"

# Resolve the CURRENT build's split artifacts by newest mtime (content-hash names accumulate across builds).
newest() { ls -t $1 2>/dev/null | head -1; }
ROOT="$(newest "${BASE}.root-${ARCH}.*.raw")"
VERITY="$(newest "${BASE}.root-${ARCH}-verity.*.raw")"
UKI="${BASE}.efi"

for pair in "root:$ROOT" "verity:$VERITY" "uki:$UKI"; do
  name="${pair%%:*}"; path="${pair#*:}"
  { [ -n "$path" ] && [ -s "$path" ]; } || { echo "MISSING/EMPTY $name artifact (looked for ${BASE}...) — run scripts/build-in-container.sh ${VERSION} first" >&2; exit 1; }
done

# The mkosi root partition is a FIXED 2 GiB (== GitHub's release-asset cap, which requires assets to be
# strictly UNDER 2 GiB), so the raw split cannot be a release asset. Compress the root + verity splits with
# zstd — this fits the cap AND is exactly what systemd-sysupdate consumes over the wire (it fetches+decompresses
# .zst natively). The UKI (~76M) ships uncompressed. Compression is idempotent (skip if the .zst is newer).
ZOPT="-T0 -19"
zst() { local src="$1"; local dst="${src}.zst"; if [ ! -f "$dst" ] || [ "$src" -nt "$dst" ]; then echo "  compressing $(basename "$src") ..." >&2; zstd -q -f $ZOPT "$src" -o "$dst"; fi; echo "$dst"; }
echo "Compressing update payload (zstd${ZOPT})..." >&2
ROOTZ="$(zst "$ROOT")"
VERITYZ="$(zst "$VERITY")"

CAP=2147483648
fail=0
echo "Resolved update payload for ${TAG}:"
for pair in "root(zst):$ROOTZ" "verity(zst):$VERITYZ" "uki:$UKI"; do
  name="${pair%%:*}"; path="${pair#*:}"; sz=$(stat -c%s "$path")
  printf '  %-11s %-9s %s\n' "$name" "$(numfmt --to=iec "$sz")" "$(basename "$path")"
  if [ "$sz" -ge "$CAP" ]; then echo "  ^ STILL AT/OVER 2GiB — GitHub will reject this asset" >&2; fail=1; fi
done
[ "$fail" = 0 ] || { echo "aborting: an asset is at/over GitHub's 2 GiB cap even after compression" >&2; exit 1; }

# SHA256SUMS over the published payload (basenames, so it verifies wherever the assets are downloaded to).
# Checksums are over the COMPRESSED bytes — exactly what systemd-sysupdate's Type=url-file expects.
SUMS="out/SHA256SUMS"
( cd out && sha256sum "$(basename "$ROOTZ")" "$(basename "$VERITYZ")" "$(basename "$UKI")" > "$(basename "$SUMS")" )
echo "SHA256SUMS:"; sed 's/^/  /' "$SUMS"

# GPG-sign the manifest -> SHA256SUMS.gpg (detached). systemd-sysupdate MANDATES this: with verification
# on (the unattended default) it fetches SHA256SUMS.gpg and refuses the update without a good signature
# against the pubkey baked into the sealed image's /usr/lib/systemd/import-pubring.gpg (proven 2026-09-05).
# The private key lives at $SIGNING_KEY (default: the encrypted vault, outside the checkout). FAIL CLOSED
# if it is missing — an unsigned manifest is worse than useless (every sealed client will reject it), so
# we refuse to publish rather than ship a release the fleet can never install.
SUMS_SIG="${SUMS}.gpg"
if [ ! -d "$SIGNING_KEY" ]; then
  echo "ABORT: update-signing key not found at: $SIGNING_KEY" >&2
  echo "       A signed SHA256SUMS.gpg is mandatory (systemd-sysupdate refuses unsigned)." >&2
  echo "       Pass --signing-key DIR / set \$UPDATE_GNUPGHOME, or restore from the offline backup" >&2
  echo "       (~/vault/shrek-os/update-signing/backup/, see docs/update-key-rotation.md)." >&2
  exit 1
fi
GNUPGHOME="$SIGNING_KEY" gpg --batch --yes --detach-sign -o "$SUMS_SIG" "$SUMS"
# Fail closed: a manifest that does not verify against its own key would brick the update path.
GNUPGHOME="$SIGNING_KEY" gpg --verify "$SUMS_SIG" "$SUMS" 2>/dev/null \
  || { echo "SHA256SUMS.gpg failed self-verify against $SIGNING_KEY — aborting" >&2; exit 1; }
echo "signed manifest -> $(basename "$SUMS_SIG") ($(stat -c%s "$SUMS_SIG")B) [key: $SIGNING_KEY]"

NOTES="$(cat <<EOF
Shrek OS ${TAG} — sealed A/B update payload.

Transparency: GitHub is the authoritative, immutable public record of what Shrek OS ships. This release is
the systemd-sysupdate payload — the zstd-compressed split root partition, its dm-verity hash partition, and
the Secure-Boot-signed UKI — plus \`SHA256SUMS\`. Authority is the signed UKI (which carries the verity roothash)
+ the checksums, so any mirror or front (e.g. updates.shrekos.iambu.dev) is untrusted plumbing: it cannot
ship a bootable tampered image.

Verify: \`sha256sum -c SHA256SUMS\`. The UKI is signed by the Shrek Secure Boot key; the running OS only
boots what that key signed.

Not included: the multi-GB fresh-install disk image (exceeds GitHub's 2 GiB asset cap; distributed
separately).
EOF
)"

if [ "$NOTES_ONLY" = 1 ]; then
  echo "--- release notes (dry run, GitHub untouched) ---"; echo "$NOTES"; exit 0
fi

# --- GitHub release (owner account) --------------------------------------------------------------
prev="$(gh auth status 2>&1 | awk '/Active account: true/{getline; next} /Logged in/{a=$0} END{}' )" || true
gh auth switch --user "$GH_PUSH_USER" >/dev/null 2>&1 || { echo "could not switch gh to $GH_PUSH_USER" >&2; exit 1; }
trap 'gh auth switch --user "$GH_RESTORE_USER" >/dev/null 2>&1 || true' EXIT

draftflag=(); [ "$DRAFT" = 1 ] && draftflag=(--draft)
if gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1; then
  echo "release $TAG already exists on $REPO — uploading/clobbering assets" >&2
  gh release upload "$TAG" --repo "$REPO" --clobber "$ROOTZ" "$VERITYZ" "$UKI" "$SUMS" ${SUMS_SIG:+"$SUMS_SIG"}
else
  gh release create "$TAG" --repo "$REPO" "${draftflag[@]}" \
    --title "Shrek OS ${TAG}" --notes "$NOTES" \
    "$ROOTZ" "$VERITYZ" "$UKI" "$SUMS" ${SUMS_SIG:+"$SUMS_SIG"}
fi
echo "published: $(gh release view "$TAG" --repo "$REPO" --json url -q .url 2>/dev/null)"
