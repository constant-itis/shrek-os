# Phase-1 S7 — the A/B update wrap (systemd-sysupdate)

> S7 gate: *deliver the image with a working A/B update mechanism, and perform one update.*
> Records why S7 uses `systemd-sysupdate` (not bootc), the whole-root-verity A/B layout, and the
> writable-`/var` fix for the S6 logind restart-loop. **Status: PASSES — v1 built (slot A), v2
> installed into slot B by `systemd-sysupdate` offline, the updated disk boots v2 under enforcing
> Secure Boot (roothash=v2, `IMAGE_VERSION=2`). One A/B update, end to end, on Debian.**

## Decision 1: sysupdate, not bootc (resolves ADR-001 update transport)

Reconnaissance at S7 (probed in a debian:trixie container, 2026-08-18):

```
Debian trixie archive:
  bootc / composefs / bootupd → NOT PACKAGED
  ostree                      → 2025.2-1 (present, but only the layer, not the bootc engine)
```

`bootc` is not in Debian; getting it needs a source build of `bootc` **and** `composefs` (also
unpackaged) — the fragile "less trodden than Fedora" path. Per the pre-authorized decision tree
this is the **janky → fall back to sysupdate** branch: we **stay on Debian**; the update transport
is `systemd-sysupdate` (raw partition A/B), not bootc. composefs/bootc remain a later upgrade,
which `phase1-spike.md` §4 permits.

The full A/B stack **is** native to trixie (systemd 257.13):

| piece               | role                                     | trixie package        |
|---------------------|------------------------------------------|-----------------------|
| `systemd-repart`    | lays out the A/B partition slots         | `systemd-repart`      |
| `systemd-sysupdate` | the update engine (`list`/`update`)      | **`systemd-container`** (Debian bundles it here) |
| `systemd-boot`      | boots newest UKI + **boot assessment**   | `systemd-boot`        |
| `mkosi` 25.3        | `SplitArtifacts=`, `Version=`, `mkosi.sysupdate/` | `mkosi`       |

Canonical references (fetched verbatim, adapted): **`systemd/particleos`** (mkosi immutable-OS demo)
and `systemd/mkosi` `docs/root-verity.md` (the whole-`/` verity A/B template we follow).

## Decision 2: hold the seal at whole-root verity (change one variable)

S4–S6 proved a **whole-root** dm-verity image boots sealed under Secure Boot (S6, commit 226e156).
S7's job is to prove the **update mechanism**, so we hold the sealing method constant and change only
that variable — classic one-variable experimental design:

- **`/` (the whole root) stays sealed read-only** via dm-verity, exactly as S4/S6 (`Type=root` +
  `Verity=data`), roothash injected into the signed UKI cmdline (the integrity anchor, since we have
  no verity-signature partition — deferred as at S4).
- **`/var` is made writable** with `systemd.volatile=state` (a kernel-cmdline flag → `/var` is a
  fresh tmpfs each boot). This is the minimal fix for the **S6 follow-up**: `systemd-logind` +
  `systemd-networkd-persistent-storage` restart-looped for want of a writable `/var`.

The fuller **sealed-`/usr` + writable-`/`** model (particleos) — which would also give a writable
`/etc` and a *persistent* `/var` — is the intended long-term shape and is **deferred**: porting it
whole drags in TPM2 LUKS, verity-signature keys, stateless-`/etc` reconstruction, and first-boot disk
provisioning — a large surface area orthogonal to proving A/B. It becomes the next refinement once
read-only `/etc` or ephemeral `/var` proves limiting. (`systemd.volatile=state` is the spike form of
"writable /var over sealed root"; a persistent `/var` partition needs a machine-id-embedded UUID.)

## Disk layout (verified on the built v1 disk)

```
#  GPT type            label            size    role
1  esp                 esp              1G      systemd-boot + both A/B UKIs (shared)
2  root-x86-64 (8304)  shrek_1          2G      dm-verity DATA, slot A (populated with v1)
3  root-verity (830C)  shrek_1_verity   100M    dm-verity HASH, slot A
4  root-x86-64 (8304)  _empty           2G      slot B — sysupdate installs v2 here
5  root-verity (830C)  _empty           100M    slot B hash
                                        (no /var partition — /var is volatile tmpfs)
```

Slots are fixed-size (not `Minimize=`) so A and B match. `Label=%M_%A` expands to `shrek_<version>`
(`%M`=IMAGE_ID, `%A`=IMAGE_VERSION — systemd os-release specifiers), which is how sysupdate's
`MatchPattern=%M_@v` extracts/relabels the version. `_empty` + `NoAuto=1` marks a reserved slot the
gpt-auto-generator won't try to mount. Verity pairing at boot is automatic: `systemd-repart` sets
each verity partition's UUID to a half of the roothash, so the UKI's `roothash=` alone lets
`systemd-veritysetup` find the data+hash pair (verified: `/dev/mapper/root` built from partuuids
`fc4092a1…` + `a97e56da…`, the two roothash halves).

## Versioned build + split artifacts

`mkosi.conf` `[Output]` — **`ImageId=` must precede `Output=`**: mkosi expands the `%i` specifier only
from settings parsed earlier in the section, so `Output=%i_…` above `ImageId=` silently yields an
empty `%i` (caught via `mkosi summary`; order now matches particleos).

```ini
ImageId=shrek
Output=%i_%v_%a                    # → shrek_<version>_x86-64.raw
SplitArtifacts=partitions,uki
# [Content]:
UnifiedKernelImageFormat=%i_%v_%a  # → UKI shrek_<version>_x86-64.efi
```
`SplitName=%t.%U` on each verity partition puts the partition-type + UUID in the split filename so the
transfer's `@u` wildcard binds it. `Version` comes from `image/mkosi.version` (gitignored;
`build-in-container.sh <N>` writes it). A build emits, beside the disk:
```
shrek_<v>_x86-64.root-x86-64.<uuid>.raw          (root data — the [Source])
shrek_<v>_x86-64.root-x86-64-verity.<uuid>.raw   (root verity)
shrek_<v>_x86-64.efi                             (signed UKI)
```

## sysupdate transfers (`image/overlay/usr/lib/sysupdate.d/`)

Three transfers — root data, root verity, UKI — adapted from particleos (retargeted usr→root, no
verity-sig). They ship **inside the image** at `/usr/lib/sysupdate.d/`: `systemd-sysupdate --image=`
resolves `--definitions` relative to the dissected image root, so the OS carries its own update
policy (also what an on-appliance self-update reads). The external version pool comes in via
`--transfer-source` (host-side). `[Source]` reads the mkosi output dir (`PathRelativeTo=explicit`, `Path=/`); `[Target]`
is the inactive on-disk partition / ESP file. `ProtectVersion=%A` never overwrites the running slot;
`InstancesMax=2` keeps exactly A+B. The UKI target carries boot-counting (`TriesLeft=3`/`TriesDone=0`)
and a `MatchPattern` listing every counter-state variant (`+@l-@d`, `+@l`, bare) so sysupdate
recognizes the UKI whatever state systemd-boot renamed it to.

## Applying the update (offline)

The built v1 disk has slot A populated + slot B `_empty`. `scripts/update-in-container.sh` copies it
and applies v2 offline with `systemd-sysupdate --image=<copy> --definitions=image/mkosi.sysupdate
--transfer-source=out update 2` — the same operation an on-appliance `systemd-sysupdate` would do,
run against a disk copy so the built artifacts stay untouched. (On-appliance self-update from a
network source is a productization step: same transfers with a `Type=url-file` `[Source]`.)

## Boot assessment / rollback (sets up S8)

The v2 UKI installs as `shrek_2_x86-64+3.efi`; systemd-boot renames it `+2-1`, `+1-2`, `+0-3` on
successive boot attempts. `+0-N` sorts last ⇒ the loader falls back to the previous (last-good) UKI —
automatic rollback. A "good boot" = reaching `boot-complete.target`; `systemd-bless-boot` then strips
the counter. Wiring the success marker + a deliberately-broken update is **S8**.

## Verified (the S7 gate)

- **v1 build + boot**: on-disk layout exactly as tabled (slot A `shrek_1`, slot B `_empty`). Boots
  sealed — Secure Boot auto-enrolled + **enforcing**, Shrek key loaded, kernel lockdown,
  `roothash=fc4092a1…` → `/dev/mapper/root` slot-A verity pair, reached `login:`,
  `/etc/issue` reads `IMAGE_VERSION=1`.
- **Logind loop fixed**: `var.mount` (tmpfs) mounted, `systemd-logind` started once cleanly, no
  "start request repeated"; only journald's normal one-time initrd→system restart.
- **The A/B update** (`scripts/update-in-container.sh`, offline `systemd-sysupdate --image`):
  `list` showed `1 current` / `2 candidate`; `update 2` imported v2's root → partition 4 (the
  `_empty` slot), v2's verity → partition 5, and installed the UKI **boot-counted** as
  `shrek_2_x86-64+3-0.efi`. "Successfully installed update '2'."
- **Updated disk boots v2**: systemd-boot picked the newer UKI, cmdline `roothash=8d65807f…` (v2's,
  distinct from v1's), `systemd-veritysetup` built `/dev/mapper/root` from partuuid `8d65807f…` =
  **slot B**, `/etc/issue` reads `IMAGE_VERSION=2`, reached `login:`.

Gotchas burned (for the next gate): `ImageId` must precede `Output=` in `[Output]` or `%i` empties;
`--image` resolves `--definitions` inside the image (ship defs in `/usr/lib/sysupdate.d/`) while
`--transfer-source` stays host-side; **`--offline` suppresses local available-version discovery** on
257 (do not pass it); offline `--image` dissection needs the container to share the host `/dev`.

## Spike-only bits to strip before ship

- `image/mkosi.conf.d/90-vm-acceptance.conf` (`console=ttyS0`) and the `/etc/issue` version marker.
- `systemd.volatile=state` → replace with the persistent writable-state layer (sealed-`/usr` model).
- Throwaway `keys/` Secure Boot key; no verity-signature partition yet.
