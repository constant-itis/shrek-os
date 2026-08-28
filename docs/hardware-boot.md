# Booting Shrek OS on real hardware off a USB stick

Shrek OS is a sealed dm-verity whole-disk image, not a live ISO. To run it on physical
hardware you write an installed-system disk image to a USB stick and boot it. Two things
are needed beyond the VM path: the initrd must carry the USB storage stack, and old/quirky
firmware (e.g. Apple) needs a loader shim.

## 1. USB storage stack in the initrd (`image/mkosi.conf.d/40-usb-boot.conf`)

mkosi builds the UKI initrd as a module-less base plus an appended per-kernel modules
initrd. Its default module set includes dm-verity + virtio (so VMs boot) but omits the USB
host controllers and the USB->SCSI->disk translation layer. Off a USB stick the controller
enumerates the device but nothing exposes it as `/dev/sdX`, so the dm-verity data partition
(`dev-disk-by-partuuid-<data>.device`) never appears, `veritysetup@root` fails, and boot
lands in emergency mode.

The drop-in pins the appended kernel-modules initrd to `default` (the essentials — keep
this, or dm-verity/virtio drop out and even VM boot breaks) plus the USB stack
(`usb-storage uas xhci-hcd xhci-pci ehci-hcd ehci-pci ohci-hcd ohci-pci uhci-hcd sd_mod
scsi_mod usbcore usb-common`), with `Exclude=.*` to keep it minimal. This is a general
"boots in a VM but not on metal off USB" fix, not Mac-specific.

## 2. Console visibility (`image/mkosi.conf.d/85-hw-console.conf`)

The sealed cmdline otherwise carries only `console=ttyS0` (serial), so a machine with no
serial port shows a blank screen through boot. The drop-in adds `console=tty0`, ordered
*before* `console=ttyS0` so `ttyS0` stays `/dev/console` (the headless VM oracles read the
serial log) while `tty0` still renders kernel output on the physical panel.

## 3. Build, verify, deploy

```
# installed-system base (persistent /home) + payload + installed target disk
INSTALLABLE=1 scripts/build-in-container.sh 1
cp out/shrek_1_x86-64.raw out/shrek-install-base.raw
scripts/build-installer-payload.sh
TARGET=out/install0-target-<name>.raw scripts/install0-writer-proof.sh
```

Pre-deploy gate — before writing any stick, confirm the UKI initrd carries BOTH the USB
stack and the boot-critical set: extract `.initrd` (`objcopy --only-section=.initrd`),
decode the zstd frames, and check for `usb-storage`/`sd_mod` AND `dm-verity`/`virtio_blk`.
Then boot the sealed image once in the VM oracle (`scripts/boot-vm.sh`) — it must reach
`Using verity root device /dev/mapper/root` and a login prompt.

Write the installed disk to the removable stick (verify it is the removable target, not an
internal NVMe/SATA disk — `lsblk -o NAME,SIZE,RM,MODEL`, expect `RM=1`):

```
sudo dd if=out/install0-target-<name>.raw of=/dev/sdX bs=4M status=progress conv=fsync
```

Verify the flash before trusting it — `dd` exiting 0 does not mean the bytes landed. Cheap USB
2.0 sticks silently drop bits, and dm-verity fails the whole boot on a single bad block (it
lands in emergency mode with `device-mapper: verity: reached maximum errors`). Read the stick
back over its own USB path and check both verity partitions against the roothash (the roothash
is in the sealed cmdline — `objcopy --only-section=.cmdline` on the UKI, or `grep roothash=` the
staged `shrek-cmdline.txt`):

```
sudo veritysetup verify /dev/sdX2 /dev/sdX3 <roothash>
```

Run the source image as a control the same way (`losetup -fP out/install0-target-<name>.raw`,
then `veritysetup verify <loop>p2 <loop>p3 <roothash>`) — the control must pass, so a stick
failure means the media, not the command. If the stick fails, re-`dd` and re-verify; a second
failure means the drive is bad — use a different stick (ideally USB 3.0).

## 4. Apple firmware: rEFInd shim + loose kernel/initrd

Old Apple firmware fails the UKI in two independent ways, so booting it needs both a loader
shim and a change in how the initrd is delivered:

1. **systemd-boot won't run** — 2012-era Mac firmware hangs on `systemd-bootx64.efi`. rEFInd
   runs fine and chainloads from the ESP, so install rEFInd as the removable-media loader.
2. **The UKI's embedded initrd is never delivered** — even launched via rEFInd, pointing at the
   whole UKI (`loader /EFI/Linux/shrek_1_x86-64.efi`) boots the kernel with *no initrd* and
   panics `Kernel panic - not syncing: No working init found`. systemd-stub hands the embedded
   initrd to the kernel through the EFI `LINUX_EFI_INITRD_MEDIA_GUID` / `LoadFile2` protocol,
   which this firmware doesn't service — the initrd is silently dropped and the dm-verity root
   never assembles. The fix is to decompose the UKI on the ESP and let rEFInd deliver a *loose*
   kernel + initrd (the `initrd=` EFI-stub file load, the same mechanism that loads the kernel),
   which the firmware does support.

`dd` wipes the ESP, so after writing the stick reinstall rEFInd and stage the loose files:

```
# fetch the loader (no root needed)
apt-get download refind && dpkg-deb -x refind_*.deb /tmp/refind-x
udisksctl mount -b /dev/sdX1                       # mounts the ESP
ESP=/run/media/$USER/<esp-label>
cp /tmp/refind-x/usr/share/refind/refind/refind_x64.efi "$ESP/EFI/BOOT/BOOTX64.EFI"

# decompose the UKI into loose kernel + initrd + its sealed cmdline
UKI="$ESP/EFI/Linux/shrek_1_x86-64.efi"
objcopy -O binary --only-section=.linux   "$UKI" "$ESP/shrek-vmlinuz"
objcopy -O binary --only-section=.initrd  "$UKI" "$ESP/shrek-initrd.img"
objcopy -O binary --only-section=.cmdline "$UKI" /tmp/c.bin && tr -d '\0' < /tmp/c.bin; echo
# ^ prints the full sealed cmdline (roothash=... console=tty0 console=ttyS0) — paste it
#   verbatim into the options= line below; a bare vmlinuz has no embedded cmdline.
```

`"$ESP/EFI/BOOT/refind.conf"`:

```
timeout 20
use_nvram false
scanfor manual

# PRIMARY: loose kernel + initrd — the firmware-compatible initrd delivery.
menuentry "Shrek OS" {
    loader /shrek-vmlinuz
    initrd /shrek-initrd.img
    options "<full sealed cmdline from objcopy: roothash=... console=tty0 console=ttyS0>"
}

# Fallback: the whole UKI. Its embedded initrd is not delivered on 2012 Mac firmware
# (panics "No working init found"); kept only for firmware that does service LoadFile2.
menuentry "Shrek OS (UKI - may not boot on Mac)" {
    loader /EFI/Linux/shrek_1_x86-64.efi
}
```

Boot: hold Option at chime -> "EFI Boot" -> rEFInd -> "Shrek OS" (the loose-kernel entry, not
the UKI fallback). The sealed cmdline already carries `console=tty0`, so kernel output renders
on the physical panel; if it drops to emergency mode, read the screen there.

## 5. Broadcom BCM4331 Wi-Fi (optional, non-free)

The 2012 MacBook's BCM4331 uses the in-kernel `b43` driver, whose firmware is proprietary and
not in Debian main (Debian's firmware-b43-installer fetches it at install time). To avoid any
on-device download, `scripts/stage-b43-firmware.sh` extracts it once at build time (from the
broadcom-wl 5.100.138 `wl_apsta.o` blob via `b43-fwcutter`) into
`image/overlay/usr/lib/firmware/b43/`, which the base image bakes into the sealed dm-verity
`/usr` (`ExtraTrees=overlay`). The firmware lives in the ROOT, not the initrd — Wi-Fi is never
needed to boot — so `b43` loads post-boot when the PCIe device is probed (`bcma` -> `b43`) and
NetworkManager comes up. `b43`/`bcma` are already in the kernel module set and are not
blacklisted.

This is OPT-IN and hardware-specific: run the staging script before `build-in-container.sh`
when targeting Broadcom hardware; a plain build stays firmware-free and universal. The
extracted blobs are non-free and gitignored (only the staging recipe is tracked). If Wi-Fi is
unstable on BCM4331 (a known `b43` quirk on this chip), fall back to wired Ethernet.

```
scripts/stage-b43-firmware.sh          # one-time, before the base build
INSTALLABLE=1 scripts/build-in-container.sh 1
# ... then the same payload -> writer-proof -> dd -> rEFInd flow above
```
