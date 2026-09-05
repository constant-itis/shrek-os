// shrekos-updates.iambu.dev — Shrek OS update front (Cloudflare Worker)
//
// A DUMB, owner-controlled, stateless front over the public constant-itis/shrek-os GitHub Releases.
// It holds NO key material and does NO dynamic signing. Authority is the Secure-Boot-signed UKI (carries
// the verity roothash) + the GPG-signed SHA256SUMS.gpg, so this front is untrusted plumbing: it cannot
// ship a bootable tampered image and it cannot forge the manifest signature. Its only jobs:
//
//   GET /<channel>/SHA256SUMS       -> the cumulative signed manifest (from the stable `manifest` release)
//   GET /<channel>/SHA256SUMS.gpg   -> its detached GPG signature
//   GET /<channel>/<asset>          -> the versioned release asset holding <asset>
//
// systemd-sysupdate (Type=url-file, Path=https://shrekos-updates.iambu.dev/stable/) fetches exactly these.
//
// Asset->release mapping is STATELESS: content-hash asset names begin with `shrek_<VERSION>_`, so the
// version (hence the `v<VERSION>` release tag) is parsed straight from the filename. No API calls, no
// per-file config. The cumulative manifest lives on ONE stable `manifest` release that the publish step
// re-signs (see scripts/sync-manifest.sh), because sysupdate wants a single SHA256SUMS listing every
// version at the base URL.

const REPO = "constant-itis/shrek-os";
const MANIFEST_TAG = "manifest"; // stable release carrying the cumulative SHA256SUMS(+.gpg)
const CHANNELS = new Set(["stable"]);
// shrek_<V>_<arch>...  e.g. shrek_1_x86-64.efi / shrek_1_x86-64.root-x86-64.<hash>.raw.zst
const ASSET_RE = /^shrek_(\d+)_[A-Za-z0-9][A-Za-z0-9._-]*$/;
const MANIFEST_FILES = new Set(["SHA256SUMS", "SHA256SUMS.gpg"]);

function ghDownload(tag, file) {
  return `https://github.com/${REPO}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(file)}`;
}

function notFound(msg) {
  return new Response((msg || "not found") + "\n", { status: 404, headers: { "content-type": "text/plain" } });
}

export default {
  async fetch(request) {
    const url = new URL(request.url);

    if (request.method !== "GET" && request.method !== "HEAD") {
      return new Response("method not allowed\n", { status: 405, headers: { "allow": "GET, HEAD" } });
    }

    const parts = url.pathname.split("/").filter(Boolean);

    // Root: a friendly liveness page, nothing sensitive.
    if (parts.length === 0) {
      return new Response("shrek-os update front — see /stable/SHA256SUMS\n",
        { status: 200, headers: { "content-type": "text/plain" } });
    }
    if (parts.length !== 2) return notFound("expected /<channel>/<file>");

    const [channel, file] = parts;
    if (!CHANNELS.has(channel)) return notFound("unknown channel");

    // Resolve which GitHub release tag serves this file.
    let tag;
    let isManifest = false;
    if (MANIFEST_FILES.has(file)) {
      tag = MANIFEST_TAG;
      isManifest = true;
    } else {
      const m = ASSET_RE.exec(file);
      if (!m) return notFound("unrecognized asset name");
      tag = "v" + m[1];
    }

    const upstream = ghDownload(tag, file);

    // Follow GitHub's redirect to its object CDN and stream the body straight back — the client never sees
    // the redirect (boring, predictable behavior). Manifest is small + changes each release (short cache);
    // assets are content-hash-immutable (long cache).
    let resp;
    try {
      resp = await fetch(upstream, {
        method: request.method,
        redirect: "follow",
        headers: { "user-agent": "shrek-os-update-front", "accept": "*/*" },
        cf: {
          cacheEverything: true,
          cacheTtl: isManifest ? 60 : 86400,
          // Never let a 404 from GitHub get cached as if it were real.
          cacheTtlByStatus: { "200-299": isManifest ? 60 : 86400, "300-599": 0 },
        },
      });
    } catch (e) {
      return new Response("upstream fetch failed\n", { status: 502, headers: { "content-type": "text/plain" } });
    }

    if (resp.status === 404) return notFound("no such release asset");
    if (resp.status >= 400) {
      return new Response("upstream error\n", { status: 502, headers: { "content-type": "text/plain" } });
    }

    const headers = new Headers(resp.headers);
    headers.delete("set-cookie");
    headers.set("x-shrek-upstream-tag", tag);
    if (isManifest) {
      headers.set("cache-control", "public, max-age=60");
      headers.set("content-type", file.endsWith(".gpg") ? "application/pgp-signature" : "text/plain");
    } else {
      headers.set("cache-control", "public, max-age=86400, immutable");
    }
    return new Response(resp.body, { status: resp.status, headers });
  },
};
