# Phase-2 spike — The Onion (signed sysext layers)

> Phase-2 milestone (roadmap): *Shrek's visible OS composes from independently managed, signed
> layers.* Spike thesis: a **signed, dm-verity-authenticated `systemd-sysext` layer merges into the
> sealed read-only `/usr`**, a **confext merges into `/etc`**, and an **unsigned or tampered layer is
> refused** — all on the Phase-1 sealed base, under enforcing Secure Boot, with the layers living on
> a writable store *outside* the sealed root. This proves "the Onion" with zero layering code of our
> own (per architecture.md §3: `systemd-sysext` does the layering; Shrek only orchestrates).

## What we are NOT building here

- **No oniond logic.** oniond stays a disabled stub (it is Phase 4). Its Phase-2 stand-in is
  `shrek-onion.service` — a single unit that mounts the store and calls the `systemd-sysext` binary
  under a fixed image policy. That is the entire "orchestration" surface the spike needs.
- **No real fat layers.** `graphics`/`desktop`/`dev`/`gaming`/`ai` are content-work and premature.
  The spike uses a trivial **marker layer** (`shrek-hello`: adds `/usr/lib/shrek/layers/hello` + a
  tiny `/usr/bin/shrek-hello`) and a marker **confext** (adds `/etc/shrek-layer.conf`). Proving the
  *mechanism* is the milestone; filling layers with real software is later.
- **No dependency/version solver.** sysext has none by design (upstream: "should not be
  misunderstood as a generic software packaging framework, as no dependency scheme is available").

## The mechanism (authoritative, systemd 257 / trixie)

- **Identity marker** (required or the image is ignored): sysext ships
  `/usr/lib/extension-release.d/extension-release.<NAME>`; confext ships
  `/etc/extension-release.d/extension-release.<NAME>`. Matching rule: `ID=` must equal the host's
  (or `_any`); then `SYSEXT_LEVEL=`/`CONFEXT_LEVEL=` if defined, else `VERSION_ID=`, must match;
  `ARCHITECTURE=` if present and not `_any`. We match the host base: `ID=debian`, `VERSION_ID=13`.
- **Merge = overlayfs.** The extension's `/usr` (+`/opt`) is combined with the host's via overlayfs
  and overmounted at `/usr`; unmerge reveals the original. The **sealed RO verity `/usr` is a
  read-only lower** — nothing is written into it; this is the sanctioned way to extend a sealed
  `/usr`. Confext overlays **`/etc` only** (merged `nosuid`,`noexec`).
- **DDI + verity + signature.** A layer ships as a `*.raw` GPT disk image (data + Verity hash +
  Verity **signature** partitions). The roothash PKCS#7 signature is checked against the kernel
  keyring (MOK/DB) and against on-disk certs at **`/usr/lib/verity.d/<name>.crt`** (vendor) /
  `/etc/verity.d/`. We reuse the throwaway **Shrek key** (`keys/secureboot.{key,crt}`) as the verity
  signing key and bake its cert at `/usr/lib/verity.d/shrek.crt` in the sealed root.
- **Policy enforcement.** `systemd-sysext --image-policy='usr=signed' merge` refuses anything not
  verity-authenticated-and-signed (`signed` implies verity + PKCS#7). This is the L3 refusal lever.
- **Search paths / store.** sysext scans `/etc/extensions`, `/run/extensions`, `/var/lib/extensions`,
  `/usr/lib/extensions` (confext: `/run/confexts`, `/var/lib/confexts`, `/usr/lib/confexts`). On our
  layout `/var` is volatile (tmpfs) and `/etc` is sealed RO, so the traditional writable drop-dirs
  are unusable *at runtime for persistence*. **Decision:** ship layers on a **separate writable
  "layer-store" disk** that `shrek-onion.service` mounts at `/var/lib/extensions` (and
  `/var/lib/confexts`) before merging. The store need not be trusted — each DDI is signed-verity and
  the image policy rejects tampering — which is exactly the real "independently-managed signed
  layers" property, and it lets us iterate layer variants without rebuilding the sealed root.

## Orchestration: `shrek-onion.service` (Phase-2 seed of oniond)

```
mask stock systemd-sysext.service + systemd-confext.service   (so only Shrek merges)
shrek-onion.service (baked + enabled in the sealed root):
  ExecStartPre  mkdir + mount the layer-store disk at /var/lib/extensions and /var/lib/confexts
  ExecStart     systemd-sysext  --image-policy='usr=signed' merge     (verdict → serial console)
  ExecStart     systemd-confext --image-policy='root=signed' merge
  ExecStartPost systemd-sysext status   (legible proof in vm-console.log)
```

The fixed `--image-policy` is **baked into the unit** (trusted), never read from the untrusted store.
When oniond is built (Phase 4) it takes over this decision (which layers, which versions, may-this-
user, roll-back-on-failure); the low-level merge stays `systemd-sysext`.

## Gates (each a go/no-go, VM-verified)

```
L1  merge plumbing   a SIGNED-verity marker sysext on the store merges into sealed /usr →
                     /usr/bin/shrek-hello runs, `systemd-sysext status` shows it merged.
                     (If it fails, `status`/journal disambiguates overlay-plumbing vs trust.)
L2  trust enforced   same layer, under --image-policy=usr=signed, still merges = the verity.d/
                     PKCS#7 trust path works on our image (the researcher's smoke-test risk).
L3  refusal          an UNSIGNED layer and a byte-TAMPERED layer on the store are REFUSED (not
                     merged); /usr/bin/shrek-hello absent, journal shows the policy rejection.  ← the gate
L4  confext /etc     a signed confext overlays sealed /etc → /etc/shrek-layer.conf appears.
```

**Milestone:** signed layers compose onto the sealed base from an external store; a bad layer is
refused. L1/L2 collapse into one build (a signed merge proves both plumbing and trust; tooling
disambiguates on failure); L3 and L4 are cheap store rebuilds against the same root image.

## Build economy

One **root** rebuild adds the Phase-2 machinery to the sealed image (shrek-onion.service + its
enable symlink, the masks, `/usr/lib/verity.d/shrek.crt`, the mount points). Thereafter each gate is
a **cheap layer-store rebuild** (`scripts/build-layers.sh` → sysext/confext DDIs via mkosi
`Format=sysext`/`confext` + `Verity=signed`, packed into an ext4 store with `mkfs.ext4 -d`), booted
with `STORE=out/layer-store.raw scripts/boot-vm.sh`. `scripts/onion-proof.sh` drives a gate end to
end and reads the verdict from the serial log.

## Deferred (not this sprint)

- Persistent-writable-store-as-product (vs the spike's second disk): folds in with the deferred
  persistent-`/var` / sealed-`/usr`+writable-`/` refinement from Phase 1.
- `.v/` versioned-directory layer selection (`systemd-vpick`, present in 257) — the version/rollback
  ergonomics oniond will use; smoke-test separately, not needed to prove merge/refusal.
- Real layer contents; layer dependency metadata; per-user activation; oniond itself (Phase 4).
