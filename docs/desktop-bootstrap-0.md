# Desktop Bootstrap 0 — the concrete slab

Status: **bounded foundation slice.** Goal: resolve the *mechanical* questions of delivering a real
Wayland desktop runtime into the sealed image, so the Work-drawer product slice can start tomorrow
against an actually-bootable shell. This is **not** the Work-drawer product slice and does **not**
touch `agentd`, gatekeeperd records, or the session data contract.

## Acceptance line
The real desktop runtime is **reproducibly installed and bootable**, **Quickshell owns the initial
shell surfaces**, and the future Work drawer has a **mock-provider seam** without prematurely
defining its product contract.

## In scope
- Determine + document how Sway, Quickshell, QML/Qt6, portals, PipeWire, and session services enter
  the immutable image / desktop sysext (this doc, §Delivery).
- Pin required source/package versions through the existing build system (§Pins).
- Sway session definition + minimum env to launch it (§Session).
- Quickshell autostarted from the Sway session (§Session).
- Initial `ui/` directory structure (§UI skeleton).
- Render only: a minimal bar, workspace indicator, launcher **placeholder**, empty Work-drawer host.
- A replaceable `MockSessionProvider` seam — **without** defining the final session schema.
- A desktop smoke test: Sway starts, Quickshell loads its config, shell surfaces instantiate, clean
  logout (§Smoke).

## Explicitly OUT of scope
Live `shrek-desktopd` · D-Bus session schema · reading gatekeeperd records · authority visualization
· Grant/Stop/promotion actions · Bluetooth, full quick-settings, update UI, theming, animations,
visual polish · **any** change to Shrek policy or isolation behavior. No fake T2 badges, no fake
authority, no demo session pretending the backend contract exists.

## Relationship to `shell-architecture.md` (divergence noted, not resolved here)
`shell-architecture.md` (Phase-10 design-of-record) specifies the Rung-2 default as **swappable
systemd-user units** (Waybar-class bar, Fuzzel launcher) plus a **Rust** trusted-path `policy/agent-UI`.
Desktop Bootstrap 0 instead hosts the shell surfaces in a **single Quickshell (QML) process** — the
monolithic-consolidation approach `shell-architecture.md`/#2544 flagged. This is a deliberate owner
direction for the Quickshell/Desktop + Work-drawer track. It does **not** breach that doc's one
load-bearing rule (the trusted-path `policy/agent-UI` must be un-shadowable): Bootstrap 0 performs
**no** authority rendering and **no** grant actions, so the trusted path is simply absent here, not
subverted. Reconciling the role-contract table with a Quickshell host is later product-slice work.

---

## Delivery — how the desktop enters the sealed image

**Decision: a signed dm-verity `shrek-desktop` sysext Onion layer** (`layers/shrek-desktop/`), built
exactly like `layers/shrek-hello` (`Format=sysext`, `Packages=`, `ExtraTrees=overlay`), assembled
into the external layer store by `scripts/build-layers.sh`, and merged onto the sealed `/usr` by
`shrek-onion.service`. Rationale:

- **Keeps the sealed base headless** — `shell-architecture.md`: the shell is an *optional* Onion
  layer; the base stays server-capable with no shell. Putting Qt6/Sway in the base bloats the sealed
  root and every reader of it.
- **Matches the Onion mechanism already shipped** — nothing new to invent; the layer is a
  package-carrying sysext (unlike the empty marker `shrek-hello`, this one sets `Packages=`, which
  mkosi installs into the extension's `/usr`).
- **Swappable / rung-selectable** — enabling the desktop = enabling a layer, per the three-rung ladder.

**Package-carried vs source-built vs staged-binary** — three delivery classes in this one layer:

| Class | Components | How |
|---|---|---|
| trixie packages | `sway`, `qt6-declarative` (QML runtime), `qt6-wayland`, `foot` (terminal), `xdg-desktop-portal` + `xdg-desktop-portal-wlr`, `pipewire` + `wireplumber`, `libgl1-mesa-dri`/`mesa-vulkan-drivers` (llvmpipe software GL), `fonts-dejavu-core` | `Packages=` in the layer `mkosi.conf` |
| trixie-**backports** pin | `uwsm` (0.26.4; not in stable — `shell-architecture.md` caveat) | backports apt line in the layer build (§Pins) |
| **source-built** | **Quickshell** (unpackaged in Debian) | compiled in the ephemeral debian:trixie build container, staged into the layer overlay `/usr/bin/quickshell`; its Qt6 runtime is satisfied by the layer's `qt6-*` packages. Mirrors how release binaries are staged into `image/overlay` before mkosi. |

**Why staged-binary for Quickshell (not a supply-pin like `image/supply/gvisor.pin`):** gVisor ships
an official portable release binary; Quickshell does not — it is Qt-runtime-coupled and must be built
against the same Qt6 it will load. So Bootstrap 0 builds it from a pinned source tag in the same
container that has the Qt6 dev headers, then stages the resulting binary. The build recipe +
version pin live in `scripts/build-desktop-layer.sh` (§Pins). A future hardening step can move this to
an in-`mkosi` build script or a reproducible supply-pin once a trusted prebuilt exists.

## Pins (reproducibility)
Recorded in `image/supply/desktop.pins` (new, mirrors `gvisor.pin` intent) and consumed by
`scripts/build-desktop-layer.sh`:
- Distribution: debian **trixie** (matches base + existing layers).
- Quickshell: pinned git tag (recorded in `desktop.pins`; `# VERIFY` the exact tag resolves at first
  build — the pin file is the single source of truth, never `main`/`latest`).
- UWSM: `trixie-backports` version pin (recorded; not a stable-suite guarantee — `shell-architecture.md`).
- All apt packages: resolved from the trixie snapshot the build container sees; the layer build logs
  the exact `dpkg -l` set for provenance (same discipline as the base `# VERIFY` comments).

## Session — how Sway boots and starts Quickshell
Bootstrap-0-minimal launch (no greetd/UWSM session wiring yet — that is a later slice; here we prove
the runtime, not the login stack):
- `/usr/bin/shrek-desktop` — a wrapper that sets the minimum Wayland env and execs
  `sway -c /usr/share/shrek/desktop/sway.config`.
- `sway.config` — a minimal config that sets the environment for a **software/headless** path and
  `exec`s `quickshell -p /usr/share/shrek/ui/shell.qml`. No keybinds beyond a logout binding and a
  terminal spawn; no bars from Sway itself (Quickshell owns all surfaces).
- **Config-root entry (`ui/shell.qml`):** Quickshell uses the entry file's directory as its "config
  folder" and forbids QML imports that escape it. So the entry is a thin `ui/shell.qml` (loads
  `shell/Shell.qml`) at the `ui/` root — that makes `ui/` the config folder, so the shell's sibling
  imports (`providers/`, `themes/`) resolve legally. Loading `shell/Shell.qml` directly would reject
  `import "../providers"`.
- Env for the deterministic/headless path (also what the smoke test uses):
  `WLR_BACKENDS=headless`, `WLR_RENDERER=pixman` (Sway needs no GPU), `WLR_LIBINPUT_NO_DEVICES=1`,
  `QT_QUICK_BACKEND=software` (Qt Quick renders QML with no GPU/EGL). Together these give a fully
  software, GPU-free shell — the basis of a deterministic smoke test.

## UI skeleton (`ui/` — canonical source; staged into the layer overlay at build)
```
ui/
  shell.qml    config-root entry (loads shell/Shell.qml) — see §Session
  shell/       Shell.qml  Bar.qml  Launcher.qml  WorkDrawer.qml
  components/  (empty — future shared widgets)
  providers/   SessionProvider.qml  MockSessionProvider.qml
  mocks/       (empty — future mock fixtures)
  themes/      Tokens.qml  qmldir (Tokens singleton)
```
`ui/` is the source of record ("where shell code lives"); `scripts/build-desktop-layer.sh` copies it
to the layer overlay at `/usr/share/shrek/ui/` (binaries-staged pattern). Rendered result is
intentionally boring: a bar (workspace indicators left, system status right), a centered launcher
**placeholder**, and a Work-drawer host that reads the `MockSessionProvider` and shows *"Nothing
running."* No authority, no badges.

**The provider seam:** `WorkDrawer` binds to a `SessionProvider` interface; Bootstrap 0 wires
`MockSessionProvider` (returns an empty session list). The final data contract — the versioned read
model that will mirror the Phase-8 Slice-1 session-view record — is **intentionally not defined here**.
Swapping the mock for a real provider is a one-line change at the `WorkDrawer` binding site.

## Smoke — `scripts/desktop-smoke.sh`
Runs the desktop layer's actual stack headless in an ephemeral `--privileged debian:trixie` container
(fast iteration; same container idiom as the layer build) and asserts, emitting `SHREK_GATE`-style
PASS/FAIL lines:
1. **Sway starts** — `sway` comes up on `WLR_BACKENDS=headless` (`swaymsg -t get_version` succeeds).
2. **Quickshell loads config** — `quickshell` starts against `Shell.qml` and logs config load with no
   QML error.
3. **Surfaces instantiate** — the shell surfaces are created (Bar + Launcher + WorkDrawer objects
   report ready via a log line; and/or `swaymsg -t get_tree` shows the layer-shell surfaces).
4. **Clean logout** — `swaymsg exit` tears the session down with rc 0 and no orphaned processes.

Sealed-VM boot integration of the desktop layer (merging `shrek-desktop` in the KVM gate) is done in
the **sealed close-out** below (`scripts/desktop-sealed-proof.sh`), after the container smoke is green.

### Bring-up result — VERIFIED GREEN (2026-08-21)
`scripts/desktop-smoke.sh` → **`PASS=4 FAIL=0`**: `DB0-sway` (Sway starts headless) · `DB0-qs-load`
(Quickshell loads the config, no QML error) · `DB0-surfaces` (shell surfaces instantiate — the
`SHREK-DESKTOP shell surfaces instantiated` marker fires) · `DB0-logout` (clean teardown). The
acceptance line is met. What the bring-up nailed down (the whole point of the slab):
- **Quickshell** resolves to **v0.3.1**; source-built with the feature set in
  `scripts/build-desktop-layer.sh` (WAYLAND + WLR layer-shell ON; X11/services/etc OFF).
- **Verified build deps** (trixie): `qt6-base-dev qt6-base-private-dev qt6-declarative-dev
  qt6-declarative-private-dev qt6-wayland-dev qt6-wayland-private-dev qt6-shadertools-dev
  libwayland-dev libwayland-bin wayland-protocols libcli11-dev libdrm-dev` + cmake/ninja/build-essential.
- **wayland-protocols gap**: trixie lacks the `ext-background-effect` staging protocol Quickshell
  references unconditionally → the build overlays newer XMLs (tag 1.49) into pkgdatadir. Recorded in
  `image/supply/desktop.pins`.
- **Headless/software path proven**: Sway `WLR_BACKENDS=headless WLR_RENDERER=pixman` + Quickshell as
  a Wayland client (`QT_QPA_PLATFORM=wayland`, `QT_QUICK_BACKEND=software`) → GPU-free, deterministic.
- **Config-folder rule**: the `ui/shell.qml` root entry is required (see §Session).

## Sealed close-out — Pn-desktop (the KVM gate) — VERIFIED GREEN (2026-08-21)
The container smoke proves the *stack*; this proves it **merges onto the sealed dm-verity `/usr` and
instantiates in the real boot**. `scripts/desktop-sealed-proof.sh` runs the pipeline — build the signed
DDI → bake the sealed root → assemble the store → boot the KVM gate → assert — and reads the verdict off
`out/vm-console.log`:
```
oniond: shrek-desktop (sysext) -> merged      # broker merged the signed layer onto sealed /usr
SHREK_GATE: PASS Pn-desktop-merge / -sway / -qs-load / -surfaces / -logout   (DESKTOP PASS=5 FAIL=0)
  ds| INFO: Launching config: "/usr/share/shrek/ui/shell.qml"
  ds| DEBUG qml: SHREK-DESKTOP shell surfaces instantiated
  ds| INFO: Configuration Loaded
```
Wiring: `image/overlay/usr/lib/shrek/onion-policy` gains `enable shrek-desktop`; the new baked
`shrek-desktop-gate.service` runs the real session (`/usr/bin/shrek-desktop` → Sway → Quickshell)
headless after the Onion merge and mirrors the verdict to the serial console (it never fails the boot).

**Integration unknowns this close-out actually resolved** (the mechanical work, not turnkey):
- **Packaged sysext needs a base tree.** mkosi 25.3 refuses `Packages=` in an extension without one
  (`Cannot install packages in extension images without a base tree`). `scripts/build-desktop-layer.sh`
  builds a throwaway base tree of the sealed-base runtime closure and builds the layer with
  `--base-tree … --overlay`, so only the new desktop files land in the DDI.
- **overlayfs-on-overlay2.** mkosi assembles the overlay under `/var/tmp`, which is docker's overlay2 →
  `mount(overlay) EINVAL`. The build bind-mounts a host ext4 dir over `/var/tmp` so every mkosi
  workspace lands on real ext4 (the disk-format base image build sidesteps this via systemd-repart).
- **Runtime lib the dev-smoke masked.** Quickshell links `libQt6Widgets.so.6`; the dev-package smoke
  pulled it transitively, the runtime layer did not — added `libqt6widgets6` to the layer `Packages=`.
- **Poweroff ordering.** `shrek-mount-gate.service` owns `poweroff-force`; it is now ordered
  `After=shrek-desktop-gate.service` so the desktop surface proof completes before the VM powers off.
- **Shared-log error scoping.** The sealed gate runs Sway+Quickshell into one log, so the qs-load error
  scan is scoped to Quickshell/QML failures (Sway's benign `[ERROR]`/Xwayland lines must not count);
  `xwayland disable` in `sway.config` drops the X dependency and the noise. (Benign remainder: a
  Fontconfig `Cannot load default config` warning — surfaces instantiate regardless.)

## Build plan (owner-split commits; no Co-Authored-By)
1. **feat(ui)** — `ui/` QML skeleton + provider seam + tokens.
2. **build** — `layers/shrek-desktop/` (`mkosi.conf` + overlay: `shrek-desktop` wrapper, `sway.config`,
   `quickshell` config), `image/supply/desktop.pins`, `scripts/build-desktop-layer.sh`
   (backports + Quickshell-from-source + stage `ui/`), `build-layers.sh` hook.
3. **test** — `scripts/desktop-smoke.sh` (container headless smoke).
4. **docs** — this file.
