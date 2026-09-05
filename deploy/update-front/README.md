# shrekos-updates.iambu.dev — update front (Cloudflare Worker)

A dumb, owner-controlled, stateless front over the public `constant-itis/shrek-os` GitHub Releases. It is
the boring, independently-operable update origin the sealed image bakes in — deliberately **not** the
`claude-remote` cloudflared tunnel (that touches prod and is a network-dogfood convenience, never the
permanent trust anchor). See `worker.js` for the routing contract and `../../docs/update-network.md` for the
model. This front holds **no key material**; authority is the SB-signed UKI + the GPG-signed manifest.

## One-time prerequisites (owner actions)

### 1. A narrowly-scoped Cloudflare API token

The `~/vault/iambu-dev-CF-token.txt` token is **DNS-only** and cannot deploy a Worker. Create a second,
Workers-scoped token — scope it as tightly as possible (least privilege for exactly this deploy):

Cloudflare dashboard → **My Profile → API Tokens → Create Token → Create Custom Token**:

- **Permissions:**
  - `Account` → `Workers Scripts` → **Edit**
  - `Zone` → `Workers Routes` → **Edit**
  - (DNS: prefer creating the `updates` record with the existing DNS token below and **omit** DNS here. Only
    add `Zone` → `DNS` → **Edit** if you want wrangler to manage the record too.)
- **Account Resources:** Include → your account only.
- **Zone Resources:** Include → Specific zone → **iambu.dev** only.
- **Client IP / TTL:** optionally pin to your IP and a short expiry — this token only needs to live long
  enough to deploy.

Save it to the vault (do not paste it into the repo or shell history):

```
printf '%s' 'PASTE_WORKERS_TOKEN' > ~/vault/iambu-dev-CF-workers-token.txt
chmod 600 ~/vault/iambu-dev-CF-workers-token.txt
```

### 2. A proxied `updates` DNS record

Workers routes only fire on **proxied (orange-cloud)** hostnames. If `shrekos-updates.iambu.dev` doesn't
resolve through Cloudflare yet, create a proxied record (target is irrelevant — the Worker route intercepts;
a placeholder like `192.0.2.1` or a CNAME to the zone apex is fine) using the DNS-scoped vault token. The
`shrekos` label under `iambu.dev` must exist too (as a proxied A/AAAA/CNAME) so the FQDN is in-zone.

## Deploy

```
cd deploy/update-front

# Publish/refresh the cumulative signed manifest to the stable `manifest` release first, so the front has
# something to serve at /stable/SHA256SUMS. (Signs with the vault key; see ../../scripts/sync-manifest.sh.)
../../scripts/sync-manifest.sh

# Deploy the Worker with the Workers-scoped token.
CLOUDFLARE_API_TOKEN="$(cat ~/vault/iambu-dev-CF-workers-token.txt)" npx wrangler deploy
```

`npx wrangler` pulls wrangler on demand (Node 24 is installed; no global install needed).

## Prove it BEFORE baking anything

Run the external proof harness from a fresh client (nothing baked, nothing trusted yet):

```
./prove-front.sh
```

It exercises, against the live host: manifest fetch, GPG verification against the repo public key, asset
fetch + checksum, a missing-file 404, redirect/cache behavior, and a **bad-signature negative case** (a
tampered manifest must fail verification). Only after this is green do we bake the pubkey + URL + egress
bless as one trust-policy change and run the networked A/B dogfood.

## Operating notes

- **Rollback:** `npx wrangler rollback` (or delete the Worker) — the front is stateless, so reverting is
  safe and instant. GitHub Releases remain the source of truth regardless.
- **New release:** `scripts/publish-release.sh <V>` then `scripts/sync-manifest.sh` — the front needs no
  redeploy (it resolves assets by filename and serves the refreshed `manifest` release).
- **Independence:** this front depends only on Cloudflare + public GitHub. No homelab host is on the update
  path. That is the point.
