#!/usr/bin/env bash
# prove-front.sh — prove the live update front BEFORE baking any trust into an image.
#
# Simulates a fresh client hitting https://shrekos-updates.iambu.dev/stable/ and checks the full contract:
# manifest fetch, GPG verification against the repo public key, real asset fetch + checksum, a missing-file
# 404, redirect/cache behavior, and a BAD-SIGNATURE negative case (a tampered manifest MUST fail). Nothing
# here trusts anything baked — it verifies against keys/shrek-update-pub.gpg from the repo.
#
#   deploy/update-front/prove-front.sh            # skips the 362M root asset checksum
#   FULL=1 deploy/update-front/prove-front.sh     # also downloads + checksums the big root split
#   HOST=https://... deploy/update-front/prove-front.sh   # test a staging host
set -uo pipefail
HOST="${HOST:-https://shrekos-updates.iambu.dev}"
BASE="$HOST/stable"
REPO_ROOT="$(git rev-parse --show-toplevel)"
PUBKEY="$REPO_ROOT/keys/shrek-update-pub.gpg"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
pass=0; fail=0
ok()   { echo "  PASS: $*"; pass=$((pass+1)); }
bad()  { echo "  FAIL: $*"; fail=$((fail+1)); }
hr()   { echo; echo "== $* =="; }

# Isolated keyring with ONLY the repo public key (a real fresh client's trust set).
export GNUPGHOME="$WORK/gnupg"; mkdir -p "$GNUPGHOME"; chmod 700 "$GNUPGHOME"
gpg --quiet --import "$PUBKEY" 2>/dev/null || { echo "cannot import $PUBKEY"; exit 2; }

curlf() { curl -fsS --max-time 60 "$@"; }
code()  { curl -s -o /dev/null -w '%{http_code}' --max-time 60 "$@"; }

hr "1. Manifest fetch"
if curlf "$BASE/SHA256SUMS" -o "$WORK/SHA256SUMS" && curlf "$BASE/SHA256SUMS.gpg" -o "$WORK/SHA256SUMS.gpg"; then
  ok "fetched SHA256SUMS ($(wc -l < "$WORK/SHA256SUMS") entries) + SHA256SUMS.gpg"
else
  bad "could not fetch manifest from $BASE — is the front deployed and the manifest release published?"
  echo; echo "RESULT: $pass passed, $fail failed"; exit 1
fi

hr "2. GPG verification (fresh client trust = repo pubkey only)"
if gpg --verify "$WORK/SHA256SUMS.gpg" "$WORK/SHA256SUMS" 2>"$WORK/verify.log" && grep -qi "good signature" "$WORK/verify.log"; then
  ok "manifest signature is GOOD against keys/shrek-update-pub.gpg"
else
  bad "manifest failed GPG verification"; sed 's/^/    /' "$WORK/verify.log"
fi

hr "3. Real asset fetch + checksum"
# Always checksum verity (small ~7M) + UKI (~76M). Root split (~362M) only with FULL=1.
while read -r want name; do
  [ -n "$name" ] || continue
  case "$name" in
    *verity*.raw.xz|*.efi) dl=1 ;;
    *.raw.xz)              [ "${FULL:-0}" = 1 ] && dl=1 || { echo "  SKIP: $name (set FULL=1 to checksum the big root split)"; continue; } ;;
    *)                     dl=1 ;;
  esac
  if curlf "$BASE/$name" -o "$WORK/$name"; then
    got="$(sha256sum "$WORK/$name" | awk '{print $1}')"
    if [ "$got" = "$want" ]; then ok "checksum OK: $name"; else bad "checksum MISMATCH: $name (want $want got $got)"; fi
    rm -f "$WORK/$name"
  else
    bad "could not fetch asset: $name"
  fi
done < "$WORK/SHA256SUMS"

hr "4. Client sees 200, not a raw redirect (front proxies GitHub's 3xx)"
# Without -L: a proxying front returns 200 itself; a naive redirect front would leak a 3xx to the client.
first="$(curl -s -o /dev/null -w '%{http_code}' --max-time 60 "$BASE/SHA256SUMS")"
if [ "$first" = "200" ]; then ok "GET /stable/SHA256SUMS -> 200 (no client-visible redirect)"; else bad "expected 200, got $first"; fi

hr "5. Cache + provenance headers"
hdr="$(curl -sI --max-time 60 "$BASE/SHA256SUMS")"
echo "$hdr" | grep -qi '^cache-control:' && ok "cache-control present on manifest" || bad "no cache-control header"
echo "$hdr" | grep -qi '^x-shrek-upstream-tag:' && ok "x-shrek-upstream-tag present (front provenance)" || echo "  note: no x-shrek-upstream-tag (non-fatal)"

hr "6. Missing / bad-name assets -> 404 (not 5xx, not a stray 200)"
for path in "does-not-exist.raw.zst" "shrek_999_x86-64.efi" "../etc/passwd" "evil.sh"; do
  c="$(code "$BASE/$path")"
  # A nonexistent-but-well-formed asset (shrek_999...) proxies to a missing release -> 404; malformed -> 404.
  if [ "$c" = "404" ]; then ok "/stable/$path -> 404"; else bad "/stable/$path -> $c (expected 404)"; fi
done
c="$(code "$HOST/nope/SHA256SUMS")"; [ "$c" = 404 ] && ok "unknown channel -> 404" || bad "unknown channel -> $c"

hr "7. BAD-SIGNATURE negative case (tampering MUST be rejected)"
# 7a. Flip a byte in the manifest; the real signature must no longer verify.
cp "$WORK/SHA256SUMS" "$WORK/tampered"; printf 'x\n' >> "$WORK/tampered"
if gpg --verify "$WORK/SHA256SUMS.gpg" "$WORK/tampered" 2>/dev/null; then
  bad "tampered manifest VERIFIED — signature check is not effective!"
else
  ok "tampered manifest correctly REJECTED"
fi
# 7b. A bogus signature over the real manifest must also fail.
head -c 400 /dev/urandom > "$WORK/bogus.gpg"
if gpg --verify "$WORK/bogus.gpg" "$WORK/SHA256SUMS" 2>/dev/null; then
  bad "bogus signature VERIFIED — trust set is wrong!"
else
  ok "bogus signature correctly REJECTED"
fi

echo; echo "==================================================="
echo "RESULT: $pass passed, $fail failed  (host: $HOST)"
if [ "$fail" -eq 0 ]; then
  echo "Front is PROVEN. Next: sysupdate list (verify ON) against this host, then the one-shot trust bake."
  exit 0
else
  echo "Front NOT proven — do NOT bake. Fix the failures above."
  exit 1
fi
