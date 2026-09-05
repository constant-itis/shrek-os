# Networked A/B updates over GitHub (ADR-007 Q6b productization / "updates on GitHub")

Status: DESIGN PROVEN (contract validated against real systemd-sysupdate 257), NOT yet wired into the
sealed image. This is Chunk 2 of "updates on GitHub" — Chunk 1 (public repo + first release v1 +
`scripts/publish-release.sh`) is DONE (https://github.com/constant-itis/shrek-os/releases/tag/v1).

## The model

GitHub is the transparency authority: every versioned build publishes its systemd-sysupdate payload
(zstd-compressed split root partition, dm-verity hash partition, Secure-Boot-signed UKI) + a manifest as a
public, immutable Release. The OS fetches over a stable owner-controlled hostname
`updates.shrekos.iambu.dev/<channel>/` (a thin front over the public releases), checksum- and
signature-verifies, and A/B installs with boot-counted rollback (the S7/S8 engine, already proven).

Authority = the SB-signed UKI (carries the verity roothash) + the GPG-signed manifest. The front is
untrusted plumbing — it cannot ship a bootable tampered image, and it cannot forge the manifest signature.

## Proven contract (systemd-sysupdate 257, verified 2026-09-05 against the real v1 artifacts)

`Type=url-file` `[Source]`:
- `Path=` = base URL. sysupdate GETs `<Path>/SHA256SUMS` AND `<Path>/SHA256SUMS.gpg`.
- **The manifest signature is MANDATORY.** With no `SHA256SUMS.gpg` the tool exits:
  `Failed to retrieve signature file, cannot verify. (Try --verify=no?)`. `Verify=` is NOT a transfer key;
  verification is only disabled by the CLI `--verify=no` (not usable for the unattended service path).
- `SHA256SUMS` is standard `sha256sum` format (`<hex>  <filename>`), and must list ALL files for ALL
  versions offered (one cumulative manifest at the base URL).
- Compression is transparent (.gz/.xz/.zst). `MatchPattern` INCLUDES the compression suffix
  (`...raw.zst`). The checksum in the manifest is over the COMPRESSED bytes — which is what
  `publish-release.sh` already generates.
- Proof (metadata level, no target partitions): a mock front serving the real v1 `.zst` assets + their
  `SHA256SUMS`, with `--verify=no`, yielded `VERSION 1 ... AVAILABLE ✓ candidate` from a real
  `/usr/lib/systemd/systemd-sysupdate list`. (Harness: scratchpad/sysupdate-contract-proof.sh.)

## Consequences for the pieces

1. **Manifest must be signed, cumulatively.** sysupdate wants ONE `SHA256SUMS` (all versions) + ONE
   `.gpg` over it. Releases publish independently, so a per-release manifest can't be the source of truth.
   `publish-release.sh` must maintain a CUMULATIVE `SHA256SUMS` (append the new version's lines to the
   running set), GPG-sign it → `SHA256SUMS.gpg` (private key on the build host, NEVER in the edge front),
   and publish both to a stable location the front serves for every version.

2. **Front is dumb.** It serves the cumulative `SHA256SUMS` + `SHA256SUMS.gpg` + proxies each asset to its
   GitHub release-asset URL. No key, no dynamic signing. Candidate mechanisms: a Cloudflare Worker on
   `updates.shrekos.iambu.dev` (serverless, no prod-host risk — preferred), or the existing `iambu-stats`
   cloudflared tunnel on claude-remote → a tiny proxy (touches prod infra; less preferred). DNS: the
   iambu.dev zone token at ~/vault/iambu-dev-CF-token.txt is DNS-scoped; a Worker deploy additionally
   needs a Workers-scoped token.

3. **Image bake (signed-image change, so pick everything before baking):**
   - Bake the update-signing GPG PUBLIC key into the sealed image's systemd import keyring
     (`/usr/lib/systemd/import-pubring.gpg`) so sysupdate trusts the manifest signature.
   - Flip `image/overlay/usr/lib/sysupdate.d/*.transfer` `[Source]` from `Type=regular-file` `Path=/`
     to `Type=url-file` `Path=https://updates.shrekos.iambu.dev/stable/`, and change the root/verity
     `MatchPattern` to the `...raw.zst` names (UKI stays uncompressed).
   - Keep the S7 OFFLINE A/B proof working: it injects a local source via `--transfer-source` (host-side),
     so the baked url-file `[Source]` does not break it — VERIFY this override still applies after the flip.

4. **Sealed egress bless.** `crates/shrek-policy/src/desktop_egress.rs` `DESKTOP_UPDATES` is an empty
   fail-closed stub. Add the one host `updates.shrekos.iambu.dev`, resolved via sealed DoT (ADR-008, like
   `weather`/open-meteo) — one hostname, not GitHub's rotating CDN IPs. Rust change → system-index bump.

5. **Networked A/B dogfood.** The sealed-VM dogfood is offline by design; a networked-pull proof needs a
   VM with egress reaching the live front (or the front mocked in-VM). New harness — the remaining gate
   after the above.

## OPEN DECISION (owner) — the update-signing trust root

The GPG key that signs `SHA256SUMS` is the cryptographic root of every future update. Decide before baking:
- Which key (a new dedicated Shrek update-signing key vs. reuse of an existing one), where the private key
  lives (e.g. keys/ gitignored like the throwaway Secure-Boot key, or ~/vault, or an offline/HSM setup),
  and the rotation story. The public half gets baked into the sealed image (irreversible until the next
  signed-image rollout), so this is a bake-once-decide-carefully choice.

## Go-live checklist (after the decision)

1. Generate the update-signing keypair; store the private key securely; export the public key.
2. `publish-release.sh`: cumulative `SHA256SUMS` + `SHA256SUMS.gpg`; re-cut v1 with the signed manifest.
3. Stand up the front (Worker) + DNS `updates.shrekos.iambu.dev`; verify `sysupdate list` (verify ON) against it.
4. Bake: import-pubring.gpg + transfer defs url-file/.raw.zst + egress bless; system-index bump.
5. Networked A/B dogfood (fresh install → pull v-next → boot → rollback proof).
6. Owner-split commits + dual-gh push.
