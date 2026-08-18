# image/ — Phase-1 mkosi image definition (skeleton)

The build runs **inside a Debian trixie container** (mkosi + modern systemd live there; the
beepboop host stays untouched — see [`../docs/phase1-spike.md`](../docs/phase1-spike.md) §0).

## Layout

```
image/
  overlay/            files copied verbatim into the image root (mkosi `Overlay`/extra-tree):
    usr/lib/systemd/system/*.service   the 4 daemon units — DISABLED BY CONSTRUCTION
                                       (no [Install] section → cannot be enabled)
    usr/libexec/shrek/                 (build output) the 5 release binaries land here
    usr/share/doc/shrek/               (build output) the docs/ tree, for the units' Documentation=
  mkosi.conf          (TODO S2) Debian trixie base, minimal, no apt at runtime
  mkosi.conf.d/       (TODO S3) hardening drop-ins (AppArmor enforcing, sysctl, cmdline)
```

## Disabled-by-construction posture

A Phase-1 image boots a **bare hardened Debian** with the Shrek control plane present but inert.
The units carry no `[Install]` section, so neither a preset nor `systemctl enable` can wire them
to a target. This is what makes the critical-failure test (architecture.md §9) honest at Phase 1:
there is nothing enabled to stop, and the OS is fully functional without any Shrek daemon running.

## Not yet here (next spike increments — see phase1-spike.md §1)

`mkosi.conf` + hardening drop-ins (S2/S3), the dm-verity seal (S4), the signed UKI (S5), MOK
enrollment + sealed boot in a VM (S6), the bootc wrap (S7), and the rollback proof (S8). Those are
generated/validated in the trixie build container and KVM VMs, not on the host.
