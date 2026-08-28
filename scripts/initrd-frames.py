#!/usr/bin/env python3
# Shrek OS — per-frame diagnostic companion to scripts/initrd-usb-check.py. Prints, for each concatenated
# zstd frame in the UKI's .initrd, the real .ko member count plus which boot-critical and USB-storage
# modules it carries. Expected shape for a mkosi UKI: frame 1 = module-less base (0 .ko), frame 2 = the
# appended kernel-modules initrd (boot-crit {dm-verity,dm-mod,ext4,virtio_*,erofs} + usb {usb-storage,
# sd_mod,scsi_mod}). Requires python3-zstandard + binutils; run in a debian:trixie container (see the
# header of initrd-usb-check.py for the docker one-liner).
import sys, subprocess, zstandard, re
subprocess.run(['objcopy', '-O', 'binary', '--only-section=.initrd', sys.argv[1], '/tmp/i.bin'], check=True)
data = open('/tmp/i.bin', 'rb').read()
buf = data; dctx = zstandard.ZstdDecompressor(); fi = 0
while buf:
    if buf[:4] != b'\x28\xb5\x2f\xfd':
        n = buf.find(b'\x28\xb5\x2f\xfd', 1)
        if n < 0:
            break
        buf = buf[n:]; continue
    d = dctx.decompressobj()
    try:
        chunk = d.decompress(buf)
    except Exception:
        n = buf.find(b'\x28\xb5\x2f\xfd', 4)
        if n < 0:
            break
        buf = buf[n:]; continue
    fi += 1
    mods = set()
    for m in re.finditer(b'070701', chunk):
        j = m.start(); h = chunk[j:j+110]
        if len(h) < 110:
            continue
        try:
            mode = int(h[14:22], 16); nsz = int(h[94:102], 16)
        except Exception:
            continue
        if nsz <= 0 or nsz > 4096:
            continue
        nm = chunk[j+110:j+110+nsz-1]
        try:
            nm = nm.decode('ascii')
        except Exception:
            continue
        if (nm.endswith('.ko') or nm.endswith('.ko.xz')) and (mode & 0o170000) == 0o100000:
            mods.add(nm.split('/')[-1].split('.')[0])
    crit = [x for x in ['dm-verity', 'dm-mod', 'ext4', 'virtio_blk', 'virtio_pci', 'virtio_scsi', 'erofs'] if x in mods]
    usb = [x for x in ['usb-storage', 'sd_mod', 'scsi_mod'] if x in mods]
    print("frame %d: decoded=%d bytes, real .ko members=%d | boot-crit=%s | usb=%s" % (fi, len(chunk), len(mods), crit, usb))
    u = d.unused_data
    if len(u) >= len(buf):
        break
    buf = u
