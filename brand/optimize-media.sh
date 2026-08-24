#!/usr/bin/env bash
# Shrink Shrek OS media without visible quality loss.
# Photographic wallpapers  -> JPEG q90 (~82% smaller) or WebP q88 (~91%, needs DE support)
# Flat/diagram-style PNGs   -> pngquant PNG-8 (~70% smaller, keeps transparency)
#
# Usage:
#   ./optimize-media.sh jpg  <files...>     # photo wallpapers -> .jpg alongside
#   ./optimize-media.sh webp <files...>     # max savings -> .webp alongside
#   ./optimize-media.sh png  <files...>     # flat art -> optimized .png in place (backup .orig)
set -euo pipefail
mode="${1:?usage: optimize-media.sh <jpg|webp|png> <files...>}"; shift
for f in "$@"; do
  [ -f "$f" ] || { echo "skip (missing): $f"; continue; }
  o=$(stat -c%s "$f")
  case "$mode" in
    jpg)  out="${f%.*}.jpg";  convert "$f" -quality 90 "$out" ;;
    webp) out="${f%.*}.webp"; if command -v cwebp >/dev/null; then cwebp -q 88 "$f" -o "$out" >/dev/null 2>&1; else convert "$f" -quality 88 "$out"; fi ;;
    png)  cp -n "$f" "$f.orig"; pngquant --quality=65-90 --force --output "$f" "$f"; out="$f" ;;
    *) echo "unknown mode: $mode"; exit 1 ;;
  esac
  n=$(stat -c%s "$out")
  printf '%-55s %7.2fMB -> %7.2fMB  (-%.0f%%)\n' "$(basename "$out")" \
    "$(echo "$o/1048576"|bc -l)" "$(echo "$n/1048576"|bc -l)" "$(echo "(1-$n/$o)*100"|bc -l)"
done
