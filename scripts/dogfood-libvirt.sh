#!/usr/bin/env bash
# Dogfood-0 (M0) — generate the AUTHORITATIVE, reproducible libvirt domain for the owner's persistent
# day-to-day Dogfood VM (docs/dogfood-0.md). virt-manager is only the GUI onto this domain; any manual
# tweak there is non-authoritative and should be folded back into THIS generator.
#
# Emits (all gitignored, under out/):
#   out/dogfood_VARS.fd    persistent OVMF NVRAM, seeded from the host's SETUP-MODE vars template so the
#                          FIRST boot auto-enrolls the Shrek key, then the enrolled state PERSISTS across
#                          reboots (unlike the throwaway oracle) — Secure Boot stays enforcing.
#   out/dogfood-shrek.xml  the domain: OVMF secboot + q35/smm, root raw + signed layer store (both RO,
#                          they are dm-verity/signed by design), SPICE + virtio-gpu (virgl when the host
#                          supports it, else plain virtio-gpu/llvmpipe), virtio keyboard + tablet input.
#
# This script only GENERATES + prints import steps — it does NOT define the domain (never mutates the
# owner's libvirt unprompted). Run on the host (beepboop) where libvirt/virt-manager live.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"; mkdir -p out

RAW="${RAW:-$(ls -t out/shrek_*_x86-64.raw 2>/dev/null | head -1)}"
[ -n "$RAW" ] && [ -f "$RAW" ] || { echo "no out/shrek_*_x86-64.raw — build DOGFOOD=1 first" >&2; exit 1; }
STORE="${STORE:-out/layer-store.raw}"
[ -f "$STORE" ] || { echo "no $STORE — run scripts/build-layers.sh desktop first" >&2; exit 1; }
RAW_ABS="$REPO_ROOT/$RAW"; STORE_ABS="$REPO_ROOT/$STORE"; NVRAM_ABS="$REPO_ROOT/out/dogfood_VARS.fd"
NAME="${NAME:-shrek-dogfood}"

# --- locate host OVMF secboot firmware (Debian/Ubuntu/Pop!_OS `ovmf` package) ---
find_fw() { for p in "$@"; do [ -f "$p" ] && { echo "$p"; return; }; done; }
CODE="$(find_fw /usr/share/OVMF/OVMF_CODE_4M.secboot.fd /usr/share/OVMF/OVMF_CODE.secboot.fd /usr/share/edk2/x64/OVMF_CODE.secboot.4m.fd)"
VARS_TMPL="$(find_fw /usr/share/OVMF/OVMF_VARS_4M.fd /usr/share/OVMF/OVMF_VARS.fd /usr/share/edk2/x64/OVMF_VARS.4m.fd)"
[ -n "${CODE:-}" ] && [ -n "${VARS_TMPL:-}" ] || {
  echo "could not find host OVMF secboot firmware — install the 'ovmf' package (apt install ovmf)" >&2
  echo "  looked for OVMF_CODE_4M.secboot.fd + OVMF_VARS_4M.fd under /usr/share/OVMF" >&2; exit 1; }

# --- seed the persistent NVRAM from the SETUP-MODE template (only if absent — never clobber enrolled state) ---
if [ ! -f "$NVRAM_ABS" ]; then cp "$VARS_TMPL" "$NVRAM_ABS"; echo "seeded persistent NVRAM (setup mode): $NVRAM_ABS"; else echo "keeping existing NVRAM: $NVRAM_ABS"; fi

# --- GPU: virgl (accelerated) when the host has a render node + qemu virgl support; else plain/llvmpipe ---
if [ -e /dev/dri/renderD128 ] && qemu-system-x86_64 -device help 2>/dev/null | grep -q virtio-vga-gl; then
  echo "host virgl available → virtio-gpu with 3D acceleration + SPICE GL"
  VIDEO="    <video><model type='virtio' heads='1'><acceleration accel3d='yes'/></model></video>"
  GRAPHICS="    <graphics type='spice' autoport='yes'><listen type='address'/><gl enable='yes'/></graphics>"
else
  echo "host virgl unavailable → plain virtio-gpu (llvmpipe software render in-guest)"
  VIDEO="    <video><model type='virtio' heads='1'/></video>"
  GRAPHICS="    <graphics type='spice' autoport='yes'><listen type='address'/></graphics>"
fi

cat > out/dogfood-shrek.xml <<XML
<domain type='kvm'>
  <name>${NAME}</name>
  <memory unit='MiB'>4096</memory>
  <vcpu>4</vcpu>
  <os firmware='efi'>
    <type arch='x86_64' machine='q35'>hvm</type>
    <loader readonly='yes' secure='yes' type='pflash'>${CODE}</loader>
    <nvram template='${VARS_TMPL}'>${NVRAM_ABS}</nvram>
    <boot dev='hd'/>
  </os>
  <features><acpi/><apic/><smm state='on'/></features>
  <cpu mode='host-passthrough'/>
  <clock offset='utc'/>
  <devices>
    <emulator>/usr/bin/qemu-system-x86_64</emulator>
    <!-- dm-verity sealed root: read-only by design (the guest never writes it; /var is a volatile tmpfs). -->
    <disk type='file' device='disk'>
      <driver name='qemu' type='raw'/>
      <source file='${RAW_ABS}'/>
      <target dev='vda' bus='virtio'/>
      <readonly/>
    </disk>
    <!-- signed Onion layer store (shrek-onion.service mounts it RO by fs-label shrek-layers). -->
    <disk type='file' device='disk'>
      <driver name='qemu' type='raw'/>
      <source file='${STORE_ABS}'/>
      <target dev='vdb' bus='virtio'/>
      <readonly/>
    </disk>
    <input type='keyboard' bus='virtio'/>
    <input type='tablet' bus='virtio'/>
    <rng model='virtio'><backend model='random'>/dev/urandom</backend></rng>
    <channel type='spicevmc'><target type='virtio' name='com.redhat.spice.0'/></channel>
${VIDEO}
${GRAPHICS}
  </devices>
</domain>
XML

echo
echo "wrote out/dogfood-shrek.xml"
echo
echo "Import (as the owner, on the host):"
echo "  virsh --connect qemu:///system define $REPO_ROOT/out/dogfood-shrek.xml"
echo "  virsh --connect qemu:///system start ${NAME}      # or open it in virt-manager and hit ▶"
echo
echo "First boot enrolls the Shrek Secure Boot key into the persistent NVRAM (a reboot), then lands at"
echo "the Sway + Quickshell desktop over SPICE. NVRAM persists, so later boots skip enrollment."
echo "NOTE: qemu:///system needs the raw + store + NVRAM readable by libvirt-qemu (or use qemu:///session)."
