# Networked A/B updates over GitHub (ADR-007 Q6b productization / "updates on GitHub")

Status: DESIGN PROVEN (contract validated against real systemd-sysupdate 257), NOT yet wired into the
sealed image. This is Chunk 2 of "updates on GitHub" — Chunk 1 (public repo + first release v1 +
`scripts/publish-release.sh`) is DONE (https://github.com/constant-itis/shrek-os/releases/tag/v1).

## The model

GitHub is the transparency authority: every versioned build publishes its systemd-sysupdate payload
(zstd-compressed split root partition, dm-verity hash partition, Secure-Boot-signed UKI) + a manifest as a
public, immutable Release. The OS fetches over a stable owner-controlled hostname
`shrekos-updates.iambu.dev/<channel>/` (a thin front over the public releases), checksum- and
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
   `shrekos-updates.iambu.dev` (serverless, no prod-host risk — preferred), or the existing `iambu-stats`
   cloudflared tunnel on claude-remote → a tiny proxy (touches prod infra; less preferred). DNS: the
   iambu.dev zone token at ~/vault/iambu-dev-CF-token.txt is DNS-scoped; a Worker deploy additionally
   needs a Workers-scoped token.

3. **Image bake (signed-image change, so pick everything before baking):**
   - Bake the update-signing GPG PUBLIC key into the sealed image's systemd import keyring
     (`/usr/lib/systemd/import-pubring.gpg`) so sysupdate trusts the manifest signature.
   - Flip `image/overlay/usr/lib/sysupdate.d/*.transfer` `[Source]` from `Type=regular-file` `Path=/`
     to `Type=url-file` `Path=https://shrekos-updates.iambu.dev/stable/`, and change the root/verity
     `MatchPattern` to the `...raw.zst` names (UKI stays uncompressed).
   - Keep the S7 OFFLINE A/B proof working: it injects a local source via `--transfer-source` (host-side),
     so the baked url-file `[Source]` does not break it — VERIFY this override still applies after the flip.

4. **Sealed egress bless.** `crates/shrek-policy/src/desktop_egress.rs` `DESKTOP_UPDATES` is an empty
   fail-closed stub. Add the one host `shrekos-updates.iambu.dev`, resolved via sealed DoT (ADR-008, like
   `weather`/open-meteo) — one hostname, not GitHub's rotating CDN IPs. Rust change → system-index bump.

5. **Networked A/B dogfood.** The sealed-VM dogfood is offline by design; a networked-pull proof needs a
   VM with egress reaching the live front (or the front mocked in-VM). New harness — the remaining gate
   after the above.

## DECISION (owner, resolved 2026-09-05) — the update-signing trust root

The GPG key that signs `SHA256SUMS` is the cryptographic root of every future update. Resolved:
- **Which key:** a dedicated Shrek update-signing key (RSA-3072, keyid 28143FEC30F15C8C), separate from the
  Secure-Boot key — two independent roots by design.
- **Where the private key lives:** the encrypted vault at `~/vault/shrek-os/update-signing/gnupg`, OUTSIDE
  the checkout, with an armored offline backup at `~/vault/shrek-os/update-signing/backup/`. It is NOT under
  `keys/` — gitignore protects against accidental commits, not filesystem loss or an over-broad tool. The
  repo carries only the PUBLIC half (`keys/shrek-update-pub.gpg`, now tracked) + the signing contract.
- **Rotation:** the keyring baked into the image is a SET, so a future image can trust old+new keys for one
  transition release, then drop the old one. Full bootstrap + rotation procedure: **docs/update-key-rotation.md**.

The public half gets baked into the sealed image (irreversible until the next signed-image rollout), so bake
the trusted key + front URL + egress bless as ONE trust-policy change.

## Go-live checklist (after the decision)

1. [DONE] Update-signing keypair generated — keyid 28143FEC30F15C8C; private keyring now at
   `~/vault/shrek-os/update-signing/gnupg` (relocated out of the repo 2026-09-05, armored offline backup +
   README in `backup/`); public key `keys/shrek-update-pub.gpg` (tracked). Signed-manifest path PROVEN
   end-to-end: a GPG-signed SHA256SUMS.gpg + the pubkey in /etc/systemd/import-pubring.gpg made real
   systemd-sysupdate 257 verify ("Good signature") and list v1 as an available candidate with verification
   ON. (scratchpad/signed-path-proof.sh.)
2. [DONE] `publish-release.sh` signs the manifest (SHA256SUMS.gpg) using an explicit `--signing-key DIR`
   (default: the vault keyring, override via `$UPDATE_GNUPGHOME`), FAILS CLOSED if the key is absent, and
   self-verifies before publishing; v1 re-cut and now carries the signed manifest.
3. [DONE 2026-09-05] Front stood up + PROVEN externally. `scripts/sync-manifest.sh` aggregates all v<N>
   manifests into one cumulative signed SHA256SUMS on a stable `manifest` release (for v1, cumulative ==
   per-release). Stateless Cloudflare Worker (`deploy/update-front/`) serves `/stable/SHA256SUMS(.gpg)` from
   that release and proxies assets to the versioned release parsed from the filename — no key material, no
   dynamic signing. LIVE at https://shrekos-updates.iambu.dev.
   HOST DECISION: a 1-LEVEL host under iambu.dev (not `updates.shrekos.iambu.dev`) — Cloudflare free
   Universal SSL covers `*.iambu.dev` (one level) only; the 2-level name fails the TLS handshake without
   Advanced Certificate Manager. `shrekos-updates` keeps the identity + matches the `*-stats.iambu.dev`
   sidecar convention. THIS is the host to bake.
   PROOF: `deploy/update-front/prove-front.sh` (fresh-client trust = repo pubkey only) = 15/15 against the
   live edge — manifest fetch, GPG verify, real root+verity+UKI checksum through the proxy (FULL=1),
   client-visible 200 (no leaked redirect), cache + provenance headers, path-traversal/bad-name/unknown-
   channel 404s, and the bad-signature negative case (tampered manifest + bogus sig both REJECTED).
   STILL TODO as the FIRST step of the bake run: a real `systemd-sysupdate list` (verify ON) against this
   live host, using the actual transfer def + import keyring about to be baked (validate-then-bake).
4. [TODO] Bake (signed-image change): image/overlay/usr/lib/systemd/import-pubring.gpg (from
   keys/shrek-update-pub.gpg) + transfer defs → url-file/.raw.zst @ shrekos-updates.iambu.dev/stable/ +
   egress bless shrekos-updates.iambu.dev via sealed DoT; system-index bump. Keep the S7 offline proof via
   --transfer-source override.
5. [TODO] Networked A/B dogfood (fresh install → pull v-next → boot → rollback proof).
6. [TODO] Owner-split commits + dual-gh push.
