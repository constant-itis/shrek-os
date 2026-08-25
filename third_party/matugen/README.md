# matugen — vendored binary provenance

Shrek OS stages the upstream **matugen** release binary into the `shrek-desktop`
sysext at `overlay/usr/bin/matugen`. DankMaterialShell drives it via its own
`dms matugen queue` wrapper to turn the active wallpaper into a Material palette
(the S5 dynamic-theming slice). matugen is invoked as a standalone executable —
nothing in Shrek links against it — so this is mere aggregation on the image, not
a derived work.

Pinned upstream:

- Repository: https://github.com/InioX/matugen
- Release tag: `v4.2.0`
- Asset: `matugen-4.2.0-x86_64.tar.gz`
- Asset sha256: `a2e3b50e49ed6439999ba3c252ed04fabd98ee4e9d12e5e5dff2e66370569751`
  (matches the GitHub release `digest`)
- Extracted `matugen` binary sha256: `688ac6eaa02c03d4d62d361f33fc7886a92b78ec06f13c5b37638416e1faac62`
- License: GPL-2.0-or-later

Build/runtime notes:

- The upstream binary is **glibc-dynamic** (ELF PIE, `NEEDED` libc/libm/libgcc_s,
  requires `GLIBC_2.39`). It is **not** a musl static build despite the "static
  binary" shorthand in earlier sprint notes. This is fine for the sealed image:
  Debian trixie ships glibc 2.41 (≥ 2.39) and all three shared deps already live
  in the base `/usr`, so no extra packages are needed.
- Because it needs glibc ≥ 2.39, the binary does **not** run on older hosts (e.g.
  a Pop!_OS 22.04 build box on glibc 2.35). Smoke-test it inside `debian:trixie`,
  not on the host.
- Verified before staging (in `debian:trixie`): `matugen 4.2.0` runs and extracts
  a palette from the shipped swamp wallpaper
  (`matugen image wallpaper.jpg --type scheme-tonal-spot --prefer saturation --json hex`).

To refresh: download the pinned asset, verify the asset sha256 above, `tar xzf`,
confirm the extracted-binary sha256, then `install -m 0755 matugen
layers/shrek-desktop/overlay/usr/bin/matugen`.
