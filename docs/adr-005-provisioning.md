# ADR-005 — Install-time provisioning & the intent→credential trust boundary

**Status:** ✅ **Accepted** — owner sign-off **GO** (2026-09-02); two non-blocking nits
folded (§6 dependency-vs-enablement wording; §5b VT adapter is layout-only for M1, variants
deferred to a future `XKBVARIANT` schema key). Design frozen. Fable-reviewed **GO-WITH-FIXES** (2026-09-02, round 1: all 7
must-fixes + nice-to-haves folded). **Owner round-2 (2026-09-02):** four corrections folded
— (1) three-state seed/deliver with a gate-completion sentinel so a transient gate crash
cannot permanently default-lock a valid manifest; (2) display name is inert plain-text in
Quickshell only, **never** `PS1` (removes an injection vector the owner-provision slice
shipped); (3) FDE backed out to its own future ADR, ADR-005 records only the
handoff-semantics constraint; (4) keymap package assumption corrected — verified `kbd`
ships no keymap data under `--no-install-recommends`; validate/deliver against **XKB**
(`xkb-data`, already shipped) via `ckbcomp`, not `/usr/share/keymaps`. **Round-3
(2026-09-02, Fable re-review GO-WITH-FIXES — four round-2 changes confirmed sound, 2
residual keymap-delivery must-fixes folded):** (MF1) `kbd`+`console-setup`
+`keyboard-configuration`+`xkb-data` pinned to the **base** image (not the late-merging
desktop sysext) so the early VT keymap applier isn't a silent no-op (the #2904/#2795 timing
class); (MF2) compositor XKB delivery mechanism named — single source `/etc/vconsole.conf`
`XKBLAYOUT=`, with `zz-shrek-desktop.sh` exporting `XKB_DEFAULT_LAYOUT` before
`exec shrek-desktop` (sway reads the env, not `vconsole.conf`). **Fable round-3 re-review
= GO (clean, 2026-09-02)** — both residual must-fixes confirmed closed, no new holes
(shipped `sway.config` verified to carry no `xkb_layout` override, so the env path is
authoritative). **Round-4 (2026-09-02, owner):** one keymap correction — `systemd-vconsole-setup`
does **not** compile XKB→console, so the VT path is now an explicit Shrek adapter
(`ckbcomp "$XKBLAYOUT" | loadkeys -`, normative §5b) rather than an implicit conversion;
this is credential-critical (the first-boot passphrase VT must match the selected layout).
Fable round-4 GO-WITH-FIXES confirmed the correction + adapter and found one residual — a
udev re-trigger of `systemd-vconsole-setup` could revert the VT to baked `us` mid-wizard;
closed with the §5b clobber closure (`getty@tty1` `ExecStartPre=` re-assert + neutralized
post-boot re-trigger) + a §11 proof that the layout survives a simulated re-trigger while
the wizard is live. Fable round-5 GO-WITH-FIXES confirmed the closure sound and refined
guard (ii): neutralize **only the keymap re-application** of the udev re-trigger via a
`90-vconsole.rules` drop-in (keep `setfont`; keymap is kernel-global so it never needs
re-applying), **never** mask the service (that would kill the early-sysinit font+keymap
run); §11 also asserts console font survives the re-trigger. **Round-6 = GO-WITH-FIXES,
last item was a phrasing-only correction Fable pre-approved the end-state of + dictated:
guard (ii) reworded to *replace* the device-add action with a direct font-only `setfont`
(rather than "filter" the monolithic `systemd-vconsole-setup`, which has no font-only
mode). Folded — design is Fable-converged. **OWNER SIGNED OFF GO (2026-09-02)** — accepted;
next is Quickshell-flow static-screening, then code. Builds on ADR-001 (Deployment / A-B), ADR-002 (environment
vocabulary), ADR-004 (File-legible canonical state), and the shipped owner-provisioning
slice (#3014, `docs/owner-provisioning.md`). Scope of *this* ADR is the **provisioning
data plane and its trust boundary** — not the Quickshell UI, modelled separately after sign-off.

---

## 1. Context

Shrek installs by writing a **sealed dm-verity whole-disk image** to the target (the
Silverblue/SteamOS model), not by running a package installer into a fresh root. Two
consequences drive this ADR:

1. **The image is immutable.** We cannot write locale/keymap/timezone/name into the
   target's `/etc` at install time — `/etc` is runtime-sealed RO. Any install-time
   choice must be *applied later*, from the writable `/home` plane, by a first-boot
   service (exactly how `shrek-owner-provision` bind-mounts a `/home`-backed shadow
   over the sealed `/etc/shadow`).
2. **The installer collects almost nothing today.** `shrek-install-calamares` collects
   one fact — the target disk — via a zenity picker; Calamares is stripped to
   `summary → exec[shrekdeploy] → finished`. A mature distro is a collect→apply funnel
   (locale, keymap, timezone, identity). We add that funnel **without** breaking
   immutability and **without** weakening the identity trust model.

The install arc moves to one interaction language (coherence direction, memory #3017):
`rEFInd → Plymouth → Quickshell installer → reboot → Quickshell first-run → desktop`,
with `shrekdeploy` retained as the privileged headless writer. This ADR defines what
data crosses that arc and under what trust.

## 2. Decision (summary)

- The **live installer collects only non-secret *installation intent*** (locale,
  keymap, timezone, owner display name, and the install-time-only target-disk choice).
- **No credential material — no passphrase, no hash — ever crosses the live→target
  handoff.** The installed sealed system establishes owner credentials itself, at first
  boot, in the Quickshell first-run / credential-enrollment surface (reusing the proven
  `shrek-owner-provision` oneshot, unchanged except that it may *pre-fill and show for
  confirmation* the display name from the transplanted manifest and require only the
  passphrase).
- Intent is carried as a **file-legible manifest** (ADR-004) in a **supervisor-owned
  store** `/home/.shrek-system/provisioning`, `root:root 0700`, separate from user state.
- The **transplant across the live→target boundary is an explicit trust boundary**: the
  manifest is schema-validated on the untrusted live side *and re-validated on the
  trusted target side* before any value touches sealed configuration.
- **Two failure postures, structurally separated.** Non-secret applies **fail
  open-to-safe-default** (baked `/etc` placeholder) and never pull anything that can
  cascade to emergency mode. Credential establishment (separate, first-run) **fails
  closed** (getty `Requires=`), exactly as today.

## 3. The trust boundary (the load-bearing part)

The live/installer environment is **untrusted** relative to the sealed installed system:
it boots off removable media, has no persistent identity, and (in M1) runs `dev` with
`sudo -n` for installer actions. The sealed target is the trusted plane.

| Property | Rule |
|---|---|
| **What may cross** | Non-secret *intent* only. Bounded enums + a sanitized display name. |
| **What may NOT cross** | Any secret: passphrase, password hash, key material. Established target-side, first boot. |
| **Direction of trust** | The target trusts *nothing* from the manifest until it re-validates. The live side never sees a credential. |
| **Failure posture** | Non-secret apply → open-to-safe-default (en_US / us / UTC). Credential → closed. |

The dividing line is not secrecy alone but **authority**: the display name is *inert
rendered data*; the passphrase is *authority*. Splitting them lets the non-secret path
be low-assurance (a corrupt locale can neither brick nor escalate) while the credential
path stays high-assurance (unchanged from #3011/#3014). A single mixed manifest would
drag everything up to credential-grade handling for no benefit and put a secret on the
live medium.

## 4. The provisioning store

- **Path:** `/home/.shrek-system/provisioning/` on the persistent `shrek-data` (`/home`).
  Layout: `manifest` (the transplanted intent), `state/` (the bind-source config files
  materialized from intent — `locale.conf`, `vconsole.conf`, `localtime`), `.applied/`
  (per-domain *seed* stamps), `fault` (legible per-key rejection reasons),
  `manifest.rejected` (root-only audit copy of a manifest that failed re-validation).
- **Ownership/mode:** `root:root 0700` throughout — supervisor-owned, **never** chowned
  to uid 1000. Mirrors the existing `/home/.shrek-system/NetworkManager` keyfile store
  (`root:root 0700`, `tmpfiles.d/shrek-home.conf`) and is deliberately *unlike*
  `hosts-seed` (which chowns its store to the owner). Anything under `state/` **becomes
  trusted config the moment it is bound over sealed `/etc`**, so uid 1000 must have no
  write there — this is the `shrek-owner-provision` must-fix-1 lesson applied verbatim.
- **Format:** plain legible files, per ADR-004 — `manifest` is `key=value`, LF, sorted
  keys. Human-auditable, diffable, greppable; no binary, no DB.
- **Separation from user state:** user-facing state lives in `/home/dev`; the supervisor
  store is a sibling under `/home/.shrek-system`. Distinct trust domains, distinct owners.

## 5. Manifest schema

Two intent classes — do not conflate:

- **Install-time intent** — consumed by the writer *in the live env*, never transplanted:
  `target_disk` (whole-disk path, already validated by `shrek-list-disks`). It is the
  install action, not first-boot config. (The vestigial `--username/--fullname/--hostname`
  plumbing in `main.py`→`shrek-install-target`, echo-only today, is **removed** —
  `owner_display_name` supersedes it; carrying both would open a second unvalidated name
  path across the boundary.)
- **First-boot intent** — the transplanted `manifest`:

| Key | Domain / validation | Applier | Consumer + kick | Default on fail |
|---|---|---|---|---|
| `locale` | member of the image's installed locale set | `shrek-locale-seed` | `10-shrek-env.sh` sources `/etc/locale.conf`; unit ordered `Before=getty@tty1` and before the first graphical session (§9) | `en_US.UTF-8` |
| `keymap` | member of the shipped **XKB layout** set (`xkb-data`, already present — §5a) | `shrek-keymap-seed` | **single canonical source = `XKBLAYOUT` (XKB layout name, e.g. `de`), in `/etc/vconsole.conf` bound from `state/`. Two explicit adapters (§5b) — Shrek converts; nothing implicitly does.** VT: `ckbcomp "$XKBLAYOUT" \| loadkeys -`. Compositor: `zz-shrek-desktop.sh` reads `XKBLAYOUT` → exports `XKB_DEFAULT_LAYOUT` → `exec shrek-desktop`. Both delivered before the first graphical session **and before `getty@tty1`** (§9) | `us` |
| `timezone` | path resolves beneath `/usr/share/zoneinfo` (tzdata) | `shrek-timezone-seed` *(fast-follow)* | glibc re-reads `/etc/localtime` per `tzset`; pre-bind services cache TZ until restart (acceptable) | `UTC` |
| `owner_display_name` | UTF-8; sanitize: strip control/CSI/ESC, cap length, no `:` | first-run `shrek-owner-provision` (pre-fill + confirm only) | **plain-text render in Quickshell/first-run ONLY** — never interpolated into a shell prompt or any command context (see note) | empty → prompt |
| `schema_version` | exact-match known version | gate | — | reject whole manifest → all defaults |

Notes:
- Every enum is validated against a set that **exists in the sealed image** (see §5a) —
  the target can always check membership offline.
- `owner_display_name` is non-secret intent and may be collected in the installer, but
  the **passphrase is never here**. First-run re-sanitizes the name, shows it for
  confirmation, requires the passphrase, then writes `/home/.shrek-identity/owner`.
- **Display name never reaches a shell.** Control/CSI stripping alone is *not* sufficient
  to make the name safe for a shell prompt — bash `PS1` performs command substitution
  (`promptvars` default-on), so a name containing `$(...)`, backticks, or `$(( ))` would
  execute. Therefore the name is rendered **only** as inert plain text by Quickshell /
  the first-run surface; it is never interpolated into `PS1` or any command context.
  This ADR **removes** the display-name-in-`PS1` behavior the owner-provision slice
  shipped (`profile.d/50-shrek-owner.sh`) — that is an injection vector, not just a
  cosmetic. (Sanitization is retained as defense-in-depth, but shell-safety no longer
  depends on it because the name never touches a shell.)
- Network (SSID/PSK) is **out of scope for M1** — M2 (wifi needs on-metal b43 association
  proof + removal of the firmware-staging footgun, memory #3016). Its PSK is a secret and
  gets its own tightly-permissioned path, **not** this manifest.

### 5b. Keymap adapter contract (normative — nothing implicitly converts)

The one canonical, user-facing namespace is the **XKB layout name** (`keymap=de`). There
is exactly one source-of-truth field, `XKBLAYOUT` in `/etc/vconsole.conf` (bound from
`state/`), and **two explicit Shrek-owned adapters convert it** to each consumer's
required form. This is normative, not "confirm during implementation," because a wrong
console layout during first-boot passphrase creation followed by the correct desktop
layout is a **credential UX/security failure** (the owner enrolls their credential on the
VT with a keyboard that doesn't match what they'll use).

**Correction (the bug this closes):** `systemd-vconsole-setup` does **not** compile XKB
→ console. It `loadkeys` a `KEYMAP=` name and treats the `XKB*` fields as the *X11/
graphical* side; it never derives the console map from `XKBLAYOUT`. Relying on it would
leave the VT on baked `us`. So:

- **VT adapter** (`shrek-keymap-seed`, root oneshot, ordered `Before=getty@tty1`):
  `ckbcomp "$XKBLAYOUT" | loadkeys -`. **M1 is layout-only:** the schema has no
  `XKBVARIANT` field, so the adapter takes no variant input — passing an unspecified/unbounded
  variant is not accepted. Variants are a later addition: add an `XKBVARIANT` schema key
  (validated against the shipped XKB variant set) *first*, then extend the invocation to
  `ckbcomp "$XKBLAYOUT" "$XKBVARIANT"`. `ckbcomp` (from `console-setup`,
  base-pinned §5a) compiles the XKB layout into a console keymap on stdout; `loadkeys -`
  (from `kbd`) loads it into the **kernel** keyboard translation table, which is global
  across VTs. No generated keymap file is placed on the sealed FS; the compile is in-memory,
  piped. (Alternative, not chosen: precompile a `KEYMAP=` artifact and drive
  `systemd-vconsole-setup` — needs a writable place for the generated map on a sealed
  system and re-introduces the console-keymap namespace we're avoiding.)
  - **★ Clobber closure (credential-critical, behavior-independent).** A single early
    `loadkeys` is **not** sufficient: `systemd-vconsole-setup` is also **udev-retriggered**
    on late console/`vtconsole`/input-device enumeration, and it re-applies the *baked*
    `KEYMAP=us`, silently reverting the VT to `us` at or during the passphrase window. Two
    normative guards close this, neither depending on unverified `vconsole-setup`
    internals: **(i) re-assert at the credential boundary** — a `getty@tty1` drop-in
    `ExecStartPre=` runs the VT adapter immediately before the login/wizard spawns, so the
    *last* keymap write before the prompt is always Shrek's intended layout; **(ii)
    stop the post-boot re-trigger from re-applying the keymap.** `systemd-vconsole-setup`
    is **monolithic** — one invocation applies font **and** keymap from `/etc/vconsole.conf`
    with no font-only mode — so you cannot "filter" it to font-only; you must *replace* the
    device-add action. Ship a product drop-in that overrides the shipped `90-vconsole.rules`
    so a later `vtconsole` device-add does **not** re-invoke `systemd-vconsole-setup`, and
    instead re-applies **font only** via a direct `setfont` (or a small Shrek helper).
    This is correct because of an asymmetry: **font is per-console** (a freshly-added VT
    needs `setfont` re-run), whereas **the keymap is the kernel-global translation table**
    (one table shared by all VTs — it already persists across VT add, never needs
    re-applying, and re-applying is exactly what reverts to baked `us`). Keymap is thus left
    entirely to the intact one-time early-`sysinit` `systemd-vconsole-setup` run **+** the
    getty `ExecStartPre` re-assert (i). **Do NOT mask `systemd-vconsole-setup.service`** —
    that kills the early-sysinit run too, so the VT boots with no baked font/keymap at all
    (worse than the clobber). (Optional defense-in-depth: the delivered `state/vconsole.conf`
    may also carry a `KEYMAP=` that resolves to the `ckbcomp`-generated map so any surviving
    `vconsole-setup` run is idempotent-to-intent — but (i)+(ii) are the load-bearing
    guarantee.)
- **Compositor adapter** (`zz-shrek-desktop.sh`): parse `XKBLAYOUT` from
  `/etc/vconsole.conf` (grep/param-expand, **never** `source` it) → `export
  XKB_DEFAULT_LAYOUT="$XKBLAYOUT"` → `exec shrek-desktop`. `sway`/`libxkbcommon` reads the
  env at context creation; the shipped `sway.config` has no `xkb_layout`/`input` override,
  so the env is authoritative.

The exact `ckbcomp` flag set for model/variant edge layouts is validated by the applier's
proof (§11), but the **invocation shape above is fixed by this ADR** — the VT layout is
not left to implementation discretion.

#### 5b-correction — the clobber source is console-setup, not `systemd-vconsole-setup` (verified at build)

The §5b text above names `systemd-vconsole-setup` as the monolithic font+keymap
retrigger to neutralize. That is **wrong for our actual image**, verified against
`debian:trixie` with the §5a package set (systemd 257.13):

- **`systemd-vconsole-setup.service` is absent** — trixie's `systemd` ships no such unit
  at all. It is not the actor and there is no `90-vconsole.rules` to override.
- The console is owned by **console-setup** (installed for `ckbcomp`, §5a). Its keymap is
  applied by two *services* reading **`/etc/default/keyboard`** (baked `XKBLAYOUT=us`) —
  **not** our bound `/etc/vconsole.conf`:
  - `keyboard-setup.service` (enabled, `sysinit`, `Before=local-fs-pre.target`) → early `loadkeys us`;
  - `console-setup.service` (enabled, `WantedBy=multi-user.target`) → `setupcon --save`,
    which re-applies `us` in the **same phase** as our applier — **this is the real clobber.**
- The vtconsole udev rule is console-setup's `90-console-setup.rules`, whose device-add
  action (`cached_setup_font.sh`) is **already font-only**. So guard (ii) as written (ship
  a font-only `90-vconsole.rules` override) is **moot** — there is nothing to override, and
  we deliberately ship **no** udev rule.

**Corrected closure (implemented):**
1. **Guard (i) — credential-boundary re-assert (load-bearing, behavior-independent):**
   `/usr/lib/shrek/shrek-provision-kick` runs `ckbcomp "$XKBLAYOUT" | loadkeys -` as an
   `ExecStartPre=-` on **`shrek-owner-provision.service`** (the first-boot passphrase wizard —
   the true credential-entry VT, which runs *before* `getty@tty1`) **and** on `getty@tty1`.
   It writes the intended layout *last*, so it does not depend on suppressing the clobber.
2. **Guard (ii) — win the race instead of overriding udev:** `shrek-keymap-seed` is ordered
   `After=console-setup.service keyboard-setup.service` and `Before=shrek-owner-provision.service`.
3. Because console-setup reads `/etc/default/keyboard` and ignores our bound
   `/etc/vconsole.conf`, the `ckbcomp | loadkeys` kick is the **only** thing that puts the
   provisioned layout on the VT; the bound `vconsole.conf` remains the canonical `XKBLAYOUT`
   source that the kick and the compositor adapter read. (Rejected alternative: also binding
   `/etc/default/keyboard` to make console-setup an ally — it violates the single-canonical-
   source rule and adds a second bind target for no gain over the re-assert.)

The §3/§6 trust-boundary and the seed/deliver three-state model are unchanged; only the
keymap *consumer-kick* mechanism is corrected here.

### 5a. Base-image prerequisites (land in this slice)

The enum sets and bind targets the schema depends on do **not** all exist in the base
today (`image/mkosi.conf` ships no `locales`/`tzdata`; `Locale=C.UTF-8` bakes only
`/etc/locale.conf`). Without these the appliers fail and the declared defaults are
unattainable. This slice adds to the **base image** (config/overlay only, zero Rust → no
system-index bump, but it *is* a base change). **Every addition below was verified
empirically under `--no-install-recommends` (2026-09-02), because that flag silently
drops data packages — the class of bug that killed wifi (wpasupplicant) and that this
must-fix caught for keymaps:**

- **Locale:** add `locales` + a **curated pre-generated locale set** (not `locales-all`
  — size). Verified: `locales` populates `/usr/share/i18n` (`en_US.UTF-8` present in
  `SUPPORTED`). Without it the target's `locale -a` is only `{C, C.utf8, POSIX}` and the
  declared `en_US.UTF-8` default is itself unattainable.
- **Keymap — the assumption was WRONG.** `kbd` provides only the *utilities*
  (`loadkeys`, `setfont`, `systemd-vconsole-setup`); under `--no-install-recommends` it
  ships **no** `/usr/share/keymaps` (verified: 0 keymap files). The keymap **data** comes
  from either `console-data` (216 static `/usr/share/keymaps/*.kmap.gz`) *or*
  `console-setup`+`keyboard-configuration` (which provide `ckbcomp`, compiling a console
  map from XKB data on the fly). **Decision: validate + deliver against XKB, not
  `/usr/share/keymaps`.** The real product surface is the Wayland compositor, which uses
  **XKB layouts** from `xkb-data` — and `xkb-data` is **already shipped** in the desktop
  layer (`layers/shrek-desktop/mkosi.conf:33`, 979 layouts incl. `us`). So the `keymap`
  intent is an XKB layout name; the graphical session consumes it directly; the tty1
  first-run VT gets the *same* layout via the explicit `ckbcomp | loadkeys -` adapter in
  §5b (**not** via `systemd-vconsole-setup`, which does not compile XKB→console). Base add
  for this slice = `kbd` (`loadkeys`) + `console-setup`+`keyboard-configuration` (for
  `ckbcomp`); **no `console-data`** and **no second keymap namespace**.
  - **★ These MUST land in the BASE image (`image/mkosi.conf` Packages=), NOT a sysext.**
    Today `console-setup` ships only in the **installer** layer
    (`layers/shrek-installer/mkosi.conf`), which is not merged onto the INSTALLABLE
    product, and `xkb-data` ships only in the **desktop** sysext
    (`layers/shrek-desktop/mkosi.conf:33`), which is a **late-merging** partition. The VT
    keymap applier runs early (before `getty@tty1`, §9), so any dependency living in a
    late-merging sysext is **invisible when systemd computes the boot transaction** — the
    exact base-vs-sysext timing bug this project already hit (#2904/#2795), and it would
    make the VT keymap path a silent no-op that falls back to `us`. **Invariant:** every
    keymap-applier dependency — `kbd`, `console-setup`/`ckbcomp`, **and `xkb-data`** —
    must be resolvable at the applier's ordering point, i.e. present in the base image.
    `xkb-data` is therefore added to **base** as well (it also stays in the desktop sysext
    for the compositor; the sysext overlay of identical data is harmless). Size-checked
    with the other adds. (The keymap-applier slice must still confirm the concrete
    `ckbcomp` VT invocation on the real merged image — do not assume the incantation.)
- **Timezone:** add `tzdata`. Verified: populates `/usr/share/zoneinfo` (`UTC` present).
- Size-check all adds against the fixed 2G root slot (~1G headroom per `image/mkosi.conf`).
- **Baked placeholders (so every bind target pre-exists and §7 fallback is real):**
  `/etc/locale.conf` (already, `en_US.UTF-8` once `locales` ships), `/etc/vconsole.conf`
  (`KEYMAP=us` / `XKBLAYOUT=us`), and `/etc/localtime` as a **regular-file copy** of
  `zoneinfo/UTC` — **never a symlink** (binding over the symlink resolves to and shadows
  the sealed `/usr/share/zoneinfo/UTC` for all consumers).

## 6. Lifecycle (each step: actor · trust · failure mode)

```
collect → schema-validate(live) → root staging → deploy → transplant(→target)
        → target re-validates(gate) → domain seed/deliver → mark seeded
```

1. **collect** — *Quickshell installer, live env (untrusted).* First-boot intent +
   the install-time target disk.
2. **schema-validate (live)** — *root helper.* Rejects malformed intent before it is
   persisted; bounds every enum; sanitizes the display name. Bad input corrected/refused
   here, with UI feedback.
3. **root staging** — *root helper, live env.* Writes the validated manifest to a
   live-side root-owned staging path (`/run/shrek/provisioning/manifest`, tmpfs,
   `root:root 0600`). Never world-readable; contains no secret regardless.
4. **deploy** — *`shrek-install-target` (root), via `main.py`.* Writes the sealed base
   image to `target_disk`; partitions and `mkfs`'s `shrek-data` (target `/home`).
   Unchanged writer path; proven by `install0-writer-proof` (+ the `SHREK_INSTALL_ALLOW_LOOP`
   loop-device harness).
5. **transplant** — *inside `shrek-install-target`, immediately after the `shrek-data`
   `mkfs` (root).* Same root context that just formatted the partition — atomic with the
   format, no second mount cycle, no inter-unit partial-state window; `main.py` stays a
   thin arg-marshaller and passes the staged manifest path. Write discipline (ADR-004):
   mount at a fresh `root:root 0700` dir under `/run` → `mkdir -m 0700` the store →
   write `manifest.tmp` → `fsync` → `rename` → `fsync` dir → `umount` (the final `sync`
   already exists in `shrek-install-target`). **This is the trust-boundary crossing** —
   afterward the file sits on the target disk, outside the live env's control.
6. **target re-validates** — *first-boot gate `shrek-provision-validate.service`,
   `After=home.mount`.* Re-runs full validation on the trusted side (defense against a
   tampered/corrupt disk). Read hardening: `O_NOFOLLOW`, require regular file + root-owned
   + size-cap before parse, strict key whitelist, reject duplicate keys (closes the
   crafted-offline-disk class, e.g. manifest-as-symlink to the shadow store). **Output
   contract (the structural guarantee):** the gate **always exits 0** and emits, to
   `/run/shrek/provisioning/validated/` (ephemeral, exempt from ADR-004 as runtime state):
   (a) one *validated per-key file* per accepted key; (b) a rejected key is omitted and
   its reason written to the store `fault` file (+ `manifest.rejected` for audit); and
   **(c) a completion sentinel `/run/shrek/provisioning/validated/.gate-complete`, written
   last**, that means "validation ran to completion." The sentinel is what lets an applier
   distinguish *"the gate ran and produced no key for me"* (an intentional default) from
   *"the gate never completed"* (transient crash — must not be treated as a default). A
   gate crash writes neither the sentinel nor any key file.
7. **domain seed/deliver** — *`shrek-{locale,keymap,timezone}-seed`,
   `After=shrek-provision-validate`.* Each applier consumes **only** the gate's `/run`
   output and resolves exactly one of **three states** (never conflate them):
   - **gate-complete + valid key present** → *seed* (materialize the value into
     `store/state/<file>`) **if not already stamped**, write `.applied/<domain>`, then
     *deliver*.
   - **gate-complete + key rejected/missing** → intentional **default policy**: this
     install legitimately has no value for the key, so stamp `.applied/<domain>` as a
     *terminal* default decision and deliver the baked default. (Terminal is correct — the
     manifest is written once at install; no better value is ever coming.)
   - **sentinel absent (gate crashed / never ran)** → **do NOT seed, do NOT stamp.**
     Deliver the existing persistent `store/state/<file>` if one was seeded on a prior
     boot; otherwise leave the baked default exposed. **Retry next boot.** This is the
     bug this must-fix closes: a transient first-boot gate crash must never permanently
     stamp a default over a valid manifest.

   The **seed** step is `.applied/`-gated (so a later manual user change is never
   re-clobbered); the **deliver** step (bind `store/state/<file>` over sealed
   `/etc/<file>` nosuid,nodev, then kick the consumer per §5) runs **unconditionally every
   boot when a persistent value exists** — bind mounts are not persistent, so
   deliver-once would silently revert to baked defaults on boot 2 (the
   `shrek-owner-provision` seed/deliver split, applied here). Applier units carry **no
   mount-namespace isolation** (`PrivateMounts`/`MountFlags=slave` would hide the bind
   from PAM/PID1 consumers — the sibling's must-fix, made a requirement here).
8. **mark seeded** — see the three-state rule above: a stamp is written **only** when the
   gate completed (valid value → seed+stamp; rejected/missing → terminal-default stamp).
   A crashed/absent gate never stamps, so the valid manifest is reconsidered next boot.

**No failure-propagating / inter-applier dependency in the non-secret plane.** The gate
and appliers carry no `Requires=`/`Wants=` on each other (or on anything whose failure
could cascade to them): `After=` is ordering only, so no unit's failure can drag another
into `failed`/emergency. This — not hope — is what makes "never emergency mode" a
structural property. (This is a *dependency* constraint, not an *enablement* one: the units
are still enabled and are legitimately pulled into the boot transaction the normal way,
via a target's `Wants=` — being scheduled to run is fine; what's forbidden is one non-secret
unit *requiring* another such that a failure propagates.) (Explicit contrast: the credential
path *does* use getty `Requires=` to fail closed.)

## 7. A/B sysupdate safety

The store lives on `shrek-data` (`/home`), surviving base A/B swaps by construction.
*Seeding* is once (`.applied/` stamps) so a new base never re-clobbers a value the owner
later changed; *delivery* re-binds every boot so the value actually holds across reboots
and base swaps. The baked sealed `/etc/{locale,vconsole}.conf` + regular-file
`/etc/localtime` (§5a) are the genuine lower/fallback layer beneath the binds — if a
bind is absent the system is still valid (identical to the shadow bind falling back to
the baked placeholder).

## 8. Variant gating

Four-way, mirroring `home.mount`/`dev-nopasswd`/`shrek-owner-provision` in
`build-in-container.sh` ("ship units unconditionally, stage gitignored enablement
per-variant"):

- **plain / CI** — appliers OFF, no writable `/home`.
- **LIVE_INSTALLER** — appliers OFF (live medium has no persistent `/home`); the
  installer *produces* the manifest.
- **INSTALLABLE (product)** — appliers ON; gate ON; first-run credential enrollment
  blocking before desktop.
- **DOGFOOD** — appliers ON with a **non-interactive baked test manifest**: a gitignored
  `provisioning.env` (mirroring `owner-provision.env`) points the gate at a baked
  `/etc/shrek/test-manifest`, which the gate seeds into the store — because DOGFOOD never
  runs the installer that would normally populate it.

## 9. Scope & delivery ordering

- **M1:** welcome → locale/keymap → disk → (reboot) → owner credential enrollment →
  desktop. Manifest carries locale, keymap, display name. **Base prereqs §5a
  (locales + curated set / kbd+console-setup / tzdata + baked placeholders incl.
  regular-file `/etc/localtime`) land now**, even though the timezone *applier* is a
  fast-follow — otherwise the fast-follow forces a second base bump.
- **Delivery ordering (normative):** locale and keymap delivery **must complete before
  the first product graphical session is instantiated** — otherwise the session comes up
  in the baked default locale/layout and only corrects on a later re-login. Appliers are
  ordered before both `getty@tty1` (the first-run wizard VT) and the graphical session
  target. The §11 dogfood already asserts the correct oracle: session `LANG` and active-VT
  keymap behavior, not just tool output.
- **Deferred (right cut):** timezone applier (fast-follow); network/wifi (M2, #3016, own
  secret path); in-installer graphical passphrase (rejected for M1 to keep secrets off the
  live medium).
- **FDE — out of scope for ADR-005 (constraint only).** FDE/encrypted-home keyed to the
  owner credential is a **separate FDE ADR**, not designed here. Prematurely locking a
  crypto lifecycle (placeholder-key custody, first-boot unlock, per-install uniqueness,
  keyslot add/remove ordering, interruption behavior, proof-of-removal, recovery) just to
  avoid a later transplant change is the wrong trade. **The only thing ADR-005 records is
  the constraint the FDE ADR must honor:** *future FDE must preserve the validated-intent
  handoff semantics of §3/§6* (non-secret intent still schema-validated live, transplanted,
  and re-validated target-side before it touches sealed config). How FDE reconciles that
  with an encrypted `shrek-data` — landing spot, unlock/rekey/recovery lifecycle — is the
  FDE ADR's job.

## 10. Resolved design decisions (were §10 open questions; Fable-answered)

1. **Transplant actor:** in **`shrek-install-target`** (not the Calamares Python), right
   after the `shrek-data` `mkfs` — atomic with the format, one root context, exercised by
   the existing loop-device harness for free.
2. **Validate-gate granularity:** single gate — but only with the §6.6 output contract
   (always exit 0, emit validated per-key to `/run`, appliers consume only that). Without
   it, single-gate is *worse* than self-validating appliers (false checkpoint that `After=`
   doesn't enforce).
3. **Timezone vs `timedated`:** the bind makes `timedated` inert, which is acceptable if
   documented. `/etc/localtime` **must** be a regular file (§5a); `timedatectl
   set-timezone` fails cleanly (RO atomic-symlink swap impossible); `timedatectl status`
   misreports the zone (it `readlink`s `/etc/localtime`) — so proofs verify via `date +%Z`
   / file compare, **not** `timedatectl`. Consider masking `systemd-timedated` on the
   product so the dead knob isn't discoverable.
4. **Display-name handoff:** safe to cross (inert data, not authority) given target-side
   re-sanitization + pre-fill-and-confirm-only + string-only parsing — all now normative
   in §3/§5.
5. **Fault surfacing:** both layers — a legible `fault` record in the store (schema
   change, §4) *and* a one-line notice on the first-run credential screen ("Some install
   settings couldn't be applied — using defaults; change them in Settings"), free since
   first-run already blocks before desktop. Journal is corroborating, not the surface.

## 11. Proof plan

Mirrors the owner-provisioning proofs (#3014):
- **Host oracle** `scripts/provision-manifest-proof.sh` (no root/no VM): schema
  accept/reject matrix, enum bounding (keymap validated against the **XKB** set, not
  `/usr/share/keymaps`), display-name sanitization (incl. CSI/ESC strip), both-sides
  re-validation, gate read-hardening (symlink/dup-key/oversize refused), **three-state
  seed/deliver logic** (valid→seed+stamp; rejected/missing→terminal-default stamp;
  no-sentinel→neither), store mode/owner (`root:root 0700`, never chown-1000), variant
  matrix, negatives (uid-1000 cannot write the store; no `Requires=` cascade).
- **Sealed-VM dogfood** stage in `dogfood-persist-probe` + `dogfood-vm.sh` scoring, asserting
  **consumer-visible** effects, not just tool output (only the merged sealed boot can catch
  the base-vs-sysext class): baked test manifest with a **non-`us`** keymap (e.g. `de`) →
  session `LANG` reflects locale; **★ VT keymap credential check — while `getty@tty1`/the
  first-run wizard is LIVE (not merely right after the seed unit), a keycode that differs
  between `us` and the test layout produces the *test-layout* character on the VT**, i.e.
  the console the owner types their passphrase on matches the layout they selected;
  **and it survives a simulated `systemd-vconsole-setup` udev re-trigger** during the wizard
  window (proves the clobber closure §5b: the `getty` `ExecStartPre=` re-assert + the
  keymap-only-neutralized re-trigger keep the VT on intent, not baked `us`); **and console
  *font* is still correctly set after that simulated re-trigger** (catches a
  mask-the-service / drop-the-whole-rule regression — guard (ii) must preserve `setfont`);
  **compositor layout = `XKB_DEFAULT_LAYOUT` reflects the same intent** (not baked `us`), so
  VT and desktop agree; `date +%Z` reflects
  tz (via `date`, **not** `timedatectl`); **the keymap-applier deps (`kbd`, `ckbcomp`/`console-setup`, `xkb-data`)
  are present on the merged product `/usr` at the applier's early ordering point** (guards
  MF1 — the host oracle cannot, no merge); **binds still present after reboot ≥2** (catches
  the deliver-once bug); a deliberately **corrupt manifest degrades to defaults without
  emergency mode**; **★ gate-crash retry: a simulated first-boot gate crash leaves no stamp
  and exposes the baked default, and on the next boot (gate recovers) the VALID manifest
  value is applied** — proving a transient crash never permanently default-locks; display
  name renders in Quickshell only and **never** in `PS1`; credential enrollment still
  fails-closed and resolves uid 1000 (zero regression vs #3014).

Zero Rust (shell/config/systemd/docs; the §5a package adds are base config) → no
system-index bump, consistent with the owner-provisioning slice. Owner-split feat/test/docs,
author officialbubies, no AI refs, `master==installer-0`, push on owner "push it".
