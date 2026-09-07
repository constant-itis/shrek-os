# ADR-009 — The desktop egress capability layer: data-driven manifests, the Network Access panel, and the delivery bridge

Status: **ACCEPTED (owner, 2026-09-06)** — design A+, after a Fable adversarial pass. Supersedes the
2026-09-06 draft. Amends **ADR-007** (§11 S7 — the deferred client-side delivery) and **ADR-008**
(§ hosts composition — widened here, but *only* for sealed-source pins; see §4).
Author: officialbubies <thegambinogold@gmail.com>
Branch: installer-0 (== master). Rust changes → system-index bump.

> ## AUTHORIZED DESTINATION ≠ AUTHENTICATED CLICK
> Sealed policy and the console ceremony **authorize** what may *ever* become reachable. The root daemon
> **enforces** it. The panel only makes that state legible and operable. A one-click toggle is **not**
> proof of live human intent — it means "this destination was previously authorized," never "the owner
> personally clicked this just now." Nothing in this document, code, or UI may describe it otherwise.

## 1. Context — verified live on installed metal (2026-09-06)

The ADR-007 egress plane enforces correctly on the 2012 MBP install, but **nothing network-touching in the
DMS shell works and there is no UI to fix it** — both owner-confirmed:

- Weather: `No Weather Data — dial tcp: lookup api.open-meteo.com on 127.0.0.53:53: … operation not permitted`.
- Typing a location by name does nothing (the geocode lookup is blocked the same way).
- There are no reachable OS controls to grant egress or set a working location.

**Mechanism (dissected, mycelium #3188/#3189):** DMS's QML makes no net calls; the forecast, the
location-name geocode, and night-mode auto-location are performed by the compiled **Go `dms` backend**
via glibc `getaddrinfo`. The ADR-007 firewall drops uid-1000 DNS to `127.0.0.53:53` by design, so any
`getaddrinfo` fetch dies even after `weather` is blessed. The hosts are baked into an **un-patchable Go
binary that does not shell curl** — so ADR-007 §11 S7's assumed `curl --resolve` widget cannot exist.

**Owner steer (2026-09-06):** *"Stop hardcoding egress in Rust and reflashing every time something needs
the net. The UI should prompt the user to grant egress for the shell/UI things that need it."* This ADR is
the answer: a **data-driven, prompt-driven capability layer** that adds no security hole.

## 2. The core decision — A+ (prompts are discoverability, not a boundary)

On this desktop **prompts can always be spoofed**: every shell process is uid 1000 in one trust domain and
any Quickshell client can draw an Overlay layer-shell surface over anything (the shipped shrek-menu proves
it). There is no attestable "which app is asking." Therefore:

> **The prompt is never the boundary. The boundary is (sealed/ceremony tier policy) + (the root daemon's
> authorization). Prompts are discoverability.**

- **B (any process may raise "allow destination X?") is rejected — structurally unsound.** It makes the
  prompt the boundary, makes the destination and the prompt *text* attacker-authored, and rebuilds the hole
  ADR-007 §3 closed ("the supervisor cannot tell the Settings panel from malware; both are uid 1000 on a
  socket").
- **A+ is ACCEPTED.** The *requestable* egress vocabulary is a set of **root-authored capability manifests**
  (data, not compiled). A spoofed one-click prompt can at most cause `egressd ask bless <sealed-name>` — the
  outcome ADR-007's tier-B criterion already declared acceptable if it happens silently. A spoofed
  ceremony-tier prompt grants nothing (only the SAK/VT console, outside the compositor, can). Plus a
  **closed-token request inbox**: any uid-1000 process may file "capability `<existing-token>` is wanted"
  (a catalog token — never free text), surfaced as a pending card. That buys B's discoverability with zero
  new authority and zero attacker-authored text.

## 3. DMS egress surface map & rulings

| Feature | Hosts | Ruling (owner-confirmed) |
|---|---|---|
| Weather forecast | `api.open-meteo.com` :443 | **GRANT** — sealed `weather` capability; delivered via §4 bridge. |
| Location-name geocode | `geocoding-api.open-meteo.com` :443 | **GRANT** — same `weather` capability. Fallbacks (nominatim/photon/bigdatacloud) **excluded** (single-vendor); their attempts die fast at the nft drop. |
| Night-mode auto-location | `ip-api.com` :80 **plain HTTP** | **LOCAL, NEVER GRANT** — no TLS (delivery floor absent), fails the one-click 443 invariant, and is a geolocation leak. Use local `suncalc.js` + seeded coordinates + GeoClue2. |
| DMS self-update | github / raw.githubusercontent / void.danklinux | **BLOCKED, PERMANENTLY** — self-mutating shell contradicts the sealed A/B image; world-writable storage = a general exfil/ingress channel. **UI hidden (OQ-4).** |
| Plugin marketplace | api/plugins.danklinux.com | **BLOCKED** — fetching third-party QML = code exec in the session. Plugin system stays default-off. **UI hidden (OQ-4).** |
| Media now-playing art | apple / youtube :443 | **NOT SHIPPED in v1 (OQ-2).** Capabilities are data — add later via a normal sealed update if the feature is ever wanted. Do not enlarge the shipped vocabulary before something needs it. |
| Tailscale / calendar / VPN | — | Out of scope. Tailscale, if ever shipped, is a system daemon under its own uid (a baseline/signed-image decision), never a uid-1000 capability. |

Night-mode + location seed already landed: `nightModeUseIPLocation:false`, `weatherLocation` "Lake Mary,
FL", `weatherCoordinates` "28.7589,-81.3178" in the DMS session seed.

## 4. The capability/manifest model

### 4.1 What is data, what stays compiled
**Moves to data:** the entire pinned, blessable capability space the owner iterates on (`weather` is the
first manifest). **Stays compiled in `shrek-policy`:** the two baseline profiles (`desktop-ntp` literal-IP
clock bootstrap, anti-drift-locked; `desktop-updates` signed-image trust) and `web-browsing` (an
enforcement *mechanism* — the cgroup accept-pair — not a destination list). Moving these to data buys
nothing and loses guarantees.

### 4.2 Two authoring sources — no writable search path, ever
```
/usr/lib/shrek/egress-capabilities/<name>.capability     # SEALED: dm-verity, shipped in image
/home/.shrek-system/egress/manifests/<name>.capability   # OWNER: root:root 0700; written ONLY by egressd
                                                         #        on a confirmed SAK-console ceremony verb
```
Sealed data under verity has identical integrity to a compiled Rust literal — "data, not Rust" costs no
trust. Owner manifests carry the same authority a raw triple already has (ADR-007 S4 `confirmed_add_raw`).
**Nothing else authors a manifest** — no socket verb creates or edits one; uid 1000 can only *name*
capabilities and file closed-token requests.

### 4.3 Schema (ADR-004 legible flat style, one file per capability, closed keys)
```
schema shrek-egress-capability/1
name weather
title Weather
purpose Local forecast and location search
feature dms:weather
tier one-click
deliver hosts
host api.open-meteo.com tcp 443
host geocoding-api.open-meteo.com tcp 443
```
- `title`/`purpose`/`feature` — the card text. Because the file is root-authored, this text is trustworthy
  for the panel to render (the property B could never have).
- `tier` — `one-click` | `ceremony`. **Rust-enforced invariant:** `one-click` requires every rule be
  `tcp:443` (names, not IP literals). Plaintext :80, odd ports, UDP → refused into one-click at parse time.
  This is what makes `ip-api.com:80` structurally unclickable.
- `deliver` — `hosts` | `none`: whether granted pins are lifted into the §5 `/etc/hosts` composition.
- `host` lines reuse the sealed raw-host grammar verbatim (`valid_raw_host`) — same argv/line-injection
  defenses, because the host lands in the world-readable `/run` state.
- **Loader is fail-closed:** unknown schema/key ⇒ whole file rejected + legible fault (never partial); a
  name collision ⇒ **sealed always wins, the owner file faults** (an owner manifest can never shadow a
  sealed one); exactly the two dirs above are read; a rejected manifest ⇒ capability absent ⇒ no bless
  possible. `shrek-policy` keeps the types/grammar/tier-invariants; `egressd` loads the catalog at boot and
  after a ceremony install.

### 4.4 The owner-pin ↔ root-resolution isolation invariant (OQ-3, strengthened by owner)
The delivery bridge (§5) lifts pins into the **host-wide** `/etc/hosts`, which root daemons also read.
The owner's ruling: *"I do not want 'safe when installed, becomes root-steerable six updates later' to
exist."* Therefore, three layers, strongest first:

1. **STRUCTURAL (primary): the `/etc/hosts` composition ingests ONLY sealed-source, non-baseline capability
   pins. Owner-manifest pins are NEVER lifted into host-wide resolution — by construction, permanently.**
   Root/system resolution can never consume an owner-supplied host pin, regardless of any future hostname
   collision, because owner pins never enter the file root reads. Owner capabilities are still enforced by
   the nft `@cap_pinned` allow; their delivery is scoped (the consumer reaches the pinned IP directly / via
   `--resolve` / the `/run` map) — never via host-wide `getaddrinfo`. (Consequence, stated plainly: an
   owner manifest with `deliver hosts` for a getaddrinfo-only consumer is refused at install with a legible
   reason; `deliver hosts` is a sealed-only affordance.)
2. **INSTALL-TIME (belt): hard-refuse** any owner manifest naming a host currently reserved/consumed by
   sealed or root system machinery — enumerated in `shrek-policy` (the agent egress tables, the
   provider-bind alias set, sealed capability hosts, baseline hosts). Not a warning — a refusal.
3. **UPDATE-TIME (suspenders): preflight collision quarantine.** When an OS update introduces a new
   system-reserved hostname that collides with an existing owner manifest host, the update/preflight
   **detects and quarantines** that owner capability (disabled + legible fault), never silently allows it.

### 4.5 Enforcement — one generalized nft set, then zero-nft extensibility
The nft table is static/baked; the supervisor is an element-only writer and must never add rules. So
per-capability sets are impossible for runtime capabilities. **Generalize the shipped `@raw_pinned` concat
set** (`{ ipv4 . proto . port }`, one baked match rule) into **`@cap_pinned`**: one baked set + one baked
rule serving all catalog capabilities; a grant's pins land as tuples; reconcile recomputes the set as the
**union** across all granted capabilities (never per-entry deletes, so two capabilities sharing a host
can't tear each other down). Migrate `weather` off `@weather_pinned` onto `@cap_pinned`. **One final image
bake**, after which unlimited future capabilities need zero nft changes.

### 4.6 Grants, persistence, delivery
Grants live in the existing blessed store keyed by capability name, survive A/B, keep intent-first bless +
boot-reconcile self-heal. The `/run/shrek/egress/state` projection grows closed tokens
`purpose=`/`feature=`/`source=sealed|owner`. Delivery (`read_egress_pins` in `hosts.rs`) consults the
loaded catalog's **sealed-source, `deliver hosts`, non-baseline** hosts (§4.4); both composers (daemon
`reproject`, boot oneshot) load the same root-owned catalog; a variant with no manifests composes
baseline-only (fail-closed). A removed capability while granted ⇒ legible fault, tuples withdrawn, hosts
lines gone at next compose (deny-direction, no ceremony).

## 5. Delivery bridge (ADR-007 §11 S7 — DONE)
Blessed **sealed-source** capability pins flow into the ADR-008 `compose_hosts` output
(`/run/shrek/hosts` ← `/etc/hosts`), so the uid-1000 Go backend resolves the sealed weather names to their
pinned IPs via NSS `files`, past the DNS drop; TLS name verification stays intact. `reproject()` recomposes
on every bless/unbless/reconcile/repin under the hosts lock (store→hosts order, no cycle). **Built +
unit-tested** (`hosts.rs` `read_egress_pins`/`compose_hosts`; `supervisor.rs` `reproject`); the v2 change
is the sealed-source restriction (§4.4) and the `@cap_pinned` retarget.

## 6. The config UI — "Network Access"

- **User-facing name: Network Access.** Implementation: `shrek-connectivity`. **Keybind: Super+Shift+N.**
- **Where it lives:** a **standalone** trimmed Quickshell surface (the shipped shrek-menu pattern — a second
  `qs` process, Overlay layer, own IPC, running *alongside* DMS), wrapping the render-proven
  `ConnectivityPage.qml` + `Egress.qml`. **Not** `ui-v2` baked wholesale (it is a whole competing shell),
  **not** a DMS plugin (forbidden by §3), **not** DMS Settings (unpatchable). When shell-v2 matures the
  page slots into its Panel unchanged.
- **Scope discipline (OQ-5):** this panel is **root security policy** only. The model-provider binds
  (`shrek-connect`) are *agent configuration* and stay OUT — they may sit near this in shell-v2 later, but
  this surface must not become miscellaneous "things involving connections."
- **Sections:** (1) **System baseline** — status-only (time sync / updates), revoke-is-console. (2)
  **Features** — one card per catalog capability from the `/run` state (root-authored text = untrickable):
  title, purpose, feature badge, `source` badge (owner-installed visibly distinct), status chip
  (Off / Active / "Blessed — waiting for network" / "Needs attention"), pinned IPs, last-refresh, and the
  control (one-click toggle, busy-locked, no optimistic flip — or "Set up at console" for ceremony tier).
  **The grant card IS the "this wants the net — allow?" surface.** (3) **Pending needs** — the closed-token
  request inbox, rate-shared, ages out. (4) **Advanced** — the raw editor + owner-manifest
  install/remove ceremony launchers + the events tail.
- **Entry points:** first-run onboarding step (widened from "weather?" to the catalog; the primary consent
  surface, shown once) + the keybind/menu + non-modal toasts from the watcher (never a modal ambush).
- **The watcher/shim:** DMS can't declare needs, so the shrek-side manifest *is* the shim; a small
  unprivileged uid-1000 watcher in the panel's `qs` process observes DMS intent signals (weather tab
  flipping on; an MPRIS player appearing) and files a **closed-token request** into the inbox. It can only
  file tokens — never a trust component.
- **Location sequencing:** the location field lives in DMS Settings (separate process). Sequence by state:
  seeded default (so the tab is never dead) → first-run weather bless offered before the user meets the DMS
  panel → the Active weather card says "Set your location in Settings → Time & Weather." The dead-typing
  symptom becomes unreachable.
- **Refusals (normative):** the panel never flips on an action's reply (the `/run` file is sole display
  truth); never renders uid-1000-authored free text; and **NOTHING rendered by the compositor ever
  authorizes above one-click tier** — ceremony tier is SAK/VT console only. A future "streamline the browser
  toggle" PR must trip over this line.

## 7. Owner decisions (2026-09-06)

- **OQ-1 — Owner-manifest toggle = one-click after the SAK ceremony admits it.** The ceremony is the
  authorization event for changing the capability *vocabulary*; requiring ceremony per enable is empty
  ritual. **But (normative):** the toggle is **not** proof of live human intent — see the banner. A
  compromised session can silently re-enable a paused owner capability; the ceremony text must say so.
- **OQ-2 — No `media-art` capability in v1.** Cosmetic; leaks listening behavior. Add later via a sealed
  update only if wanted.
- **OQ-3 — Strengthened; see §4.4.** Owner pins never touch root/system resolution (structural), + install
  hard-refuse of every system-reserved host, + update-time collision quarantine.
- **OQ-4 — Hide the DMS self-update + plugin-store UI now.** Dead controls for permanently-forbidden
  operations are dishonest UI.
- **OQ-5 — Name "Network Access", impl `shrek-connectivity`, Super+Shift+N; model-provider binds stay out.**

## 8. Security review (Fable pass, condensed)
- **Consent fatigue:** bounded — decisions ≈ catalog size, once each, grants persist across A/B; first-run
  batches them; inbox coalesces.
- **Prompt spoofing / Wayland clickjacking:** conceded and priced — max harvest is a one-click bless of a
  root-vetted capability, which a rogue could get via the socket with no UI theater. Ceremony tier is
  immune (outside the compositor).
- **`/etc/hosts` widening:** resolution-only (nft gates packets independently); root daemons were never
  gated by this table; IPs are root-DoT-resolved (real A records, at worst stale; TLS name-verify defeats a
  reassigned IP). **Owner pins excluded structurally (§4.4)** — the residual Fable flagged is closed, not
  merely warned.
- **Self-update/plugin hosts as exfil:** never shipped sealed; ceremony text warns on any world-writable
  storage host; open-meteo-class (read-only, no storage) vs github-class (bidirectional) is the review-culture line.
- **Agent netns:** unchanged — sandboxes live in private netns where this table/these hosts don't exist;
  gatekeeperd composes per-sandbox hosts itself and resolves public pins over DoT, never NSS files. The
  desktop layer cannot widen the agent plane.

## 9. Slices (each with its oracle, house style; owner-split feat/test/docs; no AI refs)
- **S1 — policy (Rust; system-index bump):** manifest grammar + parser + tier/port invariants + merged
  catalog type in `shrek-policy`; catalog-backed `bless_tier`/`admits_socket_bless`/`is_blessable_desktop_host`
  with the **sealed-source** restriction; the §4.4 install-refuse host set. Tests: malformed ⇒ absent;
  :80 one-click refused; sealed-shadows-owner; owner-source never lifted to hosts; baseline/broad stay compiled.
- **S2 — egressd + nft (Rust; one image bake) — DONE (2026-09-06).** Catalog loader (two dirs,
  `egressd::catalog`, fail-closed per file); the generalized `@cap_pinned` concat set + one baked rule
  (baked in `desktop-egress.nft`), `@weather_pinned` retired, `weather` migrated onto it; the union
  reconcile (`confirmed::desired_cap_union`/`reconcile_cap` — weather's pins fold in with raw as
  `<ip>.proto.port` tuples, MF-5 whole-set recompute); the state view gains `source`/`feature` + the
  `title`/`purpose`/`capfault` card lines; the hosts bridge is catalog-backed + sealed-source only
  (`is_sealed_deliverable_host`, §4.4); `confirmed-manifest-{install,remove}` root-peer verbs (staged
  candidate in `/run/shrek/egress-manifest-staging`, egressd is the sole writer of the live owner dir, the
  §4.4 install-refuses enforced) + the `want <catalog-token>` inbox verb. **One image bake** = the
  `desktop-egress.nft` set + the sealed `weather.capability` (baked here, not S5 — the catalog-backed hosts
  bridge needs it or weather delivery regresses). Oracle: `desktop-egress-s2-proof.sh` 16/16 (real nft,
  `@cap_pinned`) + `desktop-egress-adr009-s2-proof.sh` 8/8 (catalog, delivery, owner-host isolation,
  install-via-relay, §4.4 refusal, want-inbox, remove). **Scoped for S2 (carries to the panel/ceremony
  slices):** an *installed owner capability* is display-only — not yet one-click-blessable over the socket
  (catalog-backed `admits_socket_bless` + catalog-validated pin storage land with the S4 panel toggle);
  the vestigial `.applied` marker is retired (the union reconciles against live nft truth). The stale
  ADR-007 S4 `desktop-egress-s4-proof.sh` (drives the removed `egressd confirmed-*` CLI) and the S6 dogfood
  probe's `@weather_pinned`/`@raw_pinned` legs are noted for S6-rework; the confirmed-verb behavior is
  covered by `supervisor::tests`.
- **S3 — ceremony UX:** gatekeeperd `manifest-install/remove` rendering the full card + storage-host
  warning + the "toggle ≠ live intent" line; the update-time collision quarantine (§4.4 layer 3).
- **S4 — Network Access panel (no Rust):** standalone `shrek-connectivity` overlay baked (shrek-menu
  pattern), Super+Shift+N + menu entry, onboarding widened, watcher + inbox UI. Render proof extends
  `desktop-connectivity-proof.sh`.
- **S5 — content:** night-mode seeds + default location (done); hide DMS updater/plugin UI (OQ-4). No
  `media-art` (OQ-2).
- **S6 — sealed-VM dogfood + reflash + metal:** grant weather in-panel → forecast + location search
  populate; `ip-api.com` drops with weather granted; geocode fallbacks fail fast; owner manifest via real
  SAK ceremony → pins → revoke; owner pin absent from `/etc/hosts`; daemon-death fail-closed over `@cap_pinned`.

## 10. Changelog
- 2026-09-06 S2 built — egressd catalog loader + `@cap_pinned` generalization (weather off `@weather_pinned`,
  union reconcile) + catalog-backed state view (`source`/`feature`/card text) + sealed-source `/etc/hosts`
  bridge + owner-manifest ceremony verbs (staging in `/run/shrek/egress-manifest-staging`; §4.4 install-
  refuses) + `want` inbox. `weather.capability` baked into the sealed dir (one image bake, with the nft set).
  Owner-capability one-click bless deferred to the panel slice; `.applied` marker retired. Oracles green
  (s2-proof 16/16 real-nft, adr009-s2-proof 8/8); shrek-policy 88 + egressd 80 unit tests green.
- 2026-09-06 v2 — ACCEPTED. Fable adversarial pass folded; owner ruled OQ-1..5. A+ (manifests-as-data +
  request inbox); `@cap_pinned` generalization; owner-pin↔root-resolution structural isolation (§4.4);
  Network Access standalone panel; the two normative banners (destination≠click, toggle≠live-intent).
  Supersedes the draft's §2 D4 (do not bake `ui-v2` wholesale).
- 2026-09-06 draft — delivery bridge (built) + first cut.
