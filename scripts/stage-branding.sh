#!/usr/bin/env bash
# Stage the small Shrek-owned branding package into image/layer overlay paths.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

install -d image/overlay/usr/share/shrek/branding
install -m 0644 brand/logos/helmet-primary.svg image/overlay/usr/share/shrek/branding/shrek-os-logo.svg
install -m 0644 brand/logos/png/helmet-primary-256.png image/overlay/usr/share/shrek/branding/shrek-os-logo-256.png
install -m 0644 brand/logos/png/helmet-primary-512.png image/overlay/usr/share/shrek/branding/shrek-os-logo-512.png
install -m 0644 brand/terminal/fastfetch.txt image/overlay/usr/share/shrek/branding/shrek-os-fastfetch.txt
install -m 0644 brand/terminal/fastfetch-green.txt image/overlay/usr/share/shrek/branding/shrek-os-fastfetch-green.txt
install -m 0644 brand/terminal/fastfetch.jsonc image/overlay/usr/share/shrek/branding/fastfetch.jsonc
install -m 0644 brand/wallpapers/shrek-os-swamp.jpg image/overlay/usr/share/shrek/branding/shrek-os-wallpaper.jpg
install -m 0644 brand/palette.json image/overlay/usr/share/shrek/branding/palette.json
install -m 0644 brand/tokens.css image/overlay/usr/share/shrek/branding/tokens.css
install -m 0644 brand/tokens.qml image/overlay/usr/share/shrek/branding/tokens.qml

install -d layers/shrek-desktop/overlay/usr/share/shrek/desktop
install -m 0644 brand/wallpapers/shrek-os-swamp.jpg layers/shrek-desktop/overlay/usr/share/shrek/desktop/wallpaper.jpg
