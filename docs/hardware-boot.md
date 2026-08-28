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

## 4. Apple firmware: rEFInd shim

2012-era Mac firmware hangs on systemd-boot directly. rEFInd boots fine and chainloads the
UKI. `dd` wipes the ESP, so reinstall rEFInd on the stick's ESP (partition 1) after writing:

```
# fetch the loader (no root needed)
apt-get download refind && dpkg-deb -x refind_*.deb /tmp/refind-x
udisksctl mount -b /dev/sdX1                       # mounts the ESP
ESP=/run/media/$USER/<esp-label>
cp /tmp/refind-x/usr/share/refind/refind/refind_x64.efi "$ESP/EFI/BOOT/BOOTX64.EFI"
```

`"$ESP/EFI/BOOT/refind.conf"`:

```
timeout 20
use_nvram false
scanfor manual
menuentry "Shrek OS" {
    loader /EFI/Linux/shrek_1_x86-64.efi
}
# diagnostics: forces the physical screen as primary console
menuentry "Shrek OS (debug console=tty0)" {
    loader /EFI/Linux/shrek_1_x86-64.efi
    options "console=tty0 loglevel=7"
}
```

Boot: hold Option at chime -> "EFI Boot" -> rEFInd -> Shrek OS. Use wired Ethernet (the
BCM4331 Wi-Fi has no in-image driver). If it still fails, boot the debug entry and read the
screen.
