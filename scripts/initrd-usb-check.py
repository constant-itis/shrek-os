#!/usr/bin/env python3
# Shrek OS — PRE-DEPLOY GATE for real-hardware USB boot (the gate mandated by
# image/mkosi.conf.d/40-usb-boot.conf). Run this on the built UKI BEFORE dd'ing any physical stick:
# it proves the appended kernel-modules initrd carries the USB storage stack, without which a real
# machine drops to emergency (dev-disk-by-partuuid root device never appears). VM boot does NOT prove
# this — the VM boots via virtio, not USB.
#
# The mkosi UKI .initrd section is a module-LESS base initrd (frame 1) + an APPENDED per-kernel modules
# initrd (frame 2), concatenated as separate zstd frames with padding between them. A naive `zstd -dc`
# or single decompressobj stops after frame 1 and reports EVERY module missing (dm-verity included) —
# a nonsense result, since the VM boots with dm-verity. If you see that contradiction, it is the DECODER
# that is wrong, not the initrd: this tool scans for each zstd frame magic and decodes all frames.
#
# Requires python3-zstandard + binutils(objcopy). If the build host lacks python3-zstandard, run in a
# throwaway container:
#   docker run --rm -v $PWD/out/shrek_1_x86-64.efi:/uki.efi:ro -v $PWD/scripts/initrd-usb-check.py:/c.py:ro \
#     debian:trixie bash -c 'apt-get update -qq && apt-get install -y -qq python3-zstandard binutils && python3 /c.py /uki.efi'
# Exit 0 = PASS (safe to flash); exit 1 = FAIL (do NOT flash). See scripts/initrd-frames.py for a
# per-frame boot-crit/usb breakdown.
import sys, re, subprocess
uki = sys.argv[1]
raw = '/tmp/chk-initrd.bin'
subprocess.run(['objcopy', '-O', 'binary', '--only-section=.initrd', uki, raw], check=True)
data = open(raw, 'rb').read()
print("UKI:", uki, "| .initrd bytes:", len(data))
import zstandard
out = bytearray(); buf = data; frames = 0
dctx = zstandard.ZstdDecompressor()
while buf:
    if buf[:4] != b'\x28\xb5\x2f\xfd':
        nxt = buf.find(b'\x28\xb5\x2f\xfd', 1)
        if nxt < 0:
            break
        buf = buf[nxt:]; continue
    try:
        dobj = dctx.decompressobj()
        chunk = dobj.decompress(buf)
    except Exception:
        nxt = buf.find(b'\x28\xb5\x2f\xfd', 4)
        if nxt < 0:
            break
        buf = buf[nxt:]; continue
    out += chunk; frames += 1
    unused = dobj.unused_data
    if len(unused) >= len(buf):
        break
    buf = unused
b = bytes(out)
print("decoded frames:", frames, "| decoded bytes:", len(b))
mods = sorted(set(m.decode() for m in re.findall(rb'kernel/[a-z0-9/_.-]+\.ko(?:\.[a-z]+)?', b)))
print("total .ko modules in initrd:", len(mods))
REQUIRED = ['usb-storage', 'uas', 'sd_mod', 'scsi_mod', 'usbcore', 'usb-common',
            'xhci-hcd', 'xhci-pci', 'ehci-hcd', 'ehci-pci']
ok = True
for r in REQUIRED:
    hit = [m for m in mods if m.split('/')[-1].split('.')[0] == r]
    if not hit:
        ok = False
    print("  [%s] %-12s %s" % ('OK ' if hit else 'MISSING', r, hit[0] if hit else ''))
print("\nRESULT:", "PASS — USB storage stack present in initrd" if ok else "FAIL — required modules missing")
sys.exit(0 if ok else 1)
