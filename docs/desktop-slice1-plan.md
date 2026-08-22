# Desktop Slice 1 — the first operable Shrek shell

> **Status:** ACCEPTED (2026-08-22), building on branch `desktop-slice1`. Sub-track of Dogfood-0:
> M0/M1 boot the sealed image to a persistent Sway + Quickshell desktop, but the Quickshell surface is
> still the Bootstrap-0 skeleton (static bar + launcher placeholder + Work drawer). Owner dogfood
> result: *"I can't really do anything on the screen."* Slice 1 turns that skeleton into a desktop a
> human can operate end-to-end from the graphical session — no serial, no SSH, no dev escape hatch.
>
> Caelestia (`caelestia-dots/shell`) is a **design reference only** (see §2). It is GPLv3 and built for
> Hyprland; we adopt *concepts* clean-room, port its Hyprland integration to Sway, and keep Shrek's
> architecture and identity. This is **not** "Caelestia with a Shrek theme".

## Acceptance line
From a fresh graphical boot, a normal user operates Shrek entirely from the GUI: open a terminal, open
the launcher, launch installed apps, close/switch windows, move between workspaces, access
network/audio/Bluetooth/system controls, receive notifications/OSD, open/close the Work drawer, and
inspect live Shrek workload state — with **no** serial console, SSH, or developer-only escape hatch.

## Non-goals (explicitly deferred until the session is operable end-to-end)
PLACES (files/projects/storage), lock screen, media/MPRIS, system tray, Wi-Fi picker + NetworkManager,
brightness on real hardware, weather/perf/wallpaper theater, **any** authority-mutation UX (Work stays
read-only), M2 `shrek-dev`, SWAMP-5, SAK/VT, and cosmetic polish before the surfaces function.

---

## 1. Starting state (verified)
The shell is ~231 lines of QML across 7 files; only `SessionProvider.qml` is real.

- `ui/shell.qml` — config-folder root shim (loads `shell/Shell.qml`). **Keep.**
- `ui/shell/Shell.qml` — `ShellRoot`; instantiates Bar + Launcher + WorkDrawer, wires SessionProvider.
- `ui/shell/Bar.qml` — **placeholder**: 3 static rects + the word "shrek". No clock/workspaces/status.
- `ui/shell/Launcher.qml` — **placeholder**: a centered label. No apps, search, or launch.
- `ui/shell/WorkDrawer.qml` — real-ish: always-on 320px right panel, renders `provider.sessions`
  generically, read-only. Not toggleable.
- `ui/providers/SessionProvider.qml` — **real, read-only, keep unchanged**: polls
  `$SHREK_SESSION_DIR/*.json` (default `/run/shrek/session`) every 2 s, fail-closed, schema
  `shrek-session/1`. This is the second consumer of the exact record `shrek session status` reads.
- `ui/themes/Tokens.qml` — ~6 colors (accent `#5aa02c`), a few sizes.

`sway.config` binds only `Super+Return→foot` and `Super+Shift+e→exit`. No workspace/window/focus
binds and no output config — this, not the empty QML, is the immediate reason the box feels inert.

**Load-bearing fact:** Quickshell (v0.3.1, source-built in `scripts/build-desktop-layer.sh`) is compiled
with every backend/service OFF (`-DI3=OFF -DWAYLAND_TOPLEVEL_MANAGEMENT=OFF -DSERVICE_PIPEWIRE=OFF
-DSERVICE_NOTIFICATIONS=OFF -DSERVICE_UPOWER=OFF -DBLUETOOTH=OFF …`). Only `WAYLAND` + `WLR
layer-shell` are on. Slice 1's enabling move is to flip the right subset back on — and per the
stable/fast/secure rule those backends link libraries **already shipped in the desktop layer**
(`pipewire`, `bluez`, `dbus`, Sway IPC), so this adds native event-driven consumers of daemons we
already trust, **not** a new dependency closure.

**Networking reality:** the image runs `systemd-networkd` (wired DHCP), not NetworkManager. Slice 1
shows honest **read-only** link/connectivity state from networkd and **defers** NetworkManager + the
Wi-Fi picker until real-hardware dogfooding creates that requirement.

---

## 2. Caelestia study — concept map (REUSE CONCEPT / ADAPT / DEFER / REJECT)
**License gate:** Caelestia is **GPLv3-only**; fonts are OFL. We copy **no** Caelestia QML verbatim
(that would pull the whole `shrek-desktop` layer into GPLv3) — we clean-room reimplement from concepts,
which is the right call anyway since we want Shrek identity, not a clone.

| Caelestia concept | Shrek adaptation | Verdict |
|---|---|---|
| Thin `shell.qml` root loads singleton services + surfaces | Same shape in `shell/Shell.qml` | REUSE CONCEPT |
| Services (`/services/*` singletons) vs views (`/modules/*`), reactive binding, no polling | `ui/services/` wrap **Quickshell native backends**; `ui/surfaces/` render them | REUSE CONCEPT |
| Per-monitor via `Variants` over screens + per-screen state | Same; 1 output in VM, built multi-monitor-ready | ADAPT |
| `Anim.qml` token-typed animation (type → duration+easing) | Shrek `Anim.qml` + a small set of duration/easing tokens (calmer) | ADAPT |
| Bar (workspaces, status cluster, popouts) | Sway workspaces via `Quickshell.I3`; popout focus via layer-shell keyboard + click-catcher | ADAPT |
| Launcher (slide-up, fuzzy app list) | `Quickshell.DesktopEntries` + JS fuzzy; no fish/action-prefix yet | ADAPT |
| Notifications = D-Bus `org.freedesktop.Notifications` server | `-DSERVICE_NOTIFICATIONS=ON` → Quickshell *is* the server; one audited surface | REUSE CONCEPT |
| OSD (transient volume/brightness) | Native Pipewire + `brightnessctl` (hw-gated) | ADAPT |
| Dashboard/Nexus (settings, weather, perf, m3 blobs) | Minimal Sway-backed SYSTEM quick-controls only | DEFER |
| Lock (`WlSessionLock` + PAM) | Deferred; `-DWAYLAND_SESSION_LOCK` stays OFF | DEFER |
| Sidebar / Utilities / VPN / recording / toasts-zoo | Not on the usability path | DEFER |
| `Shortcuts.qml` (Hyprland global-shortcut IPC) | **Sway `bindsym` → `quickshell ipc call`** | ADAPT |
| Visualiser (`libcava`+`aubio`), `libqalculate`, `ddcutil`, `caelestia-cli`, `fish`, `m3shapes`, MD3-palette-from-wallpaper | none of it | REJECT |
| Dynamic color from wallpaper | Fixed Shrek identity palette in Tokens | REJECT (their approach) |

**Hyprland coupling is concentrated and cleanly replaceable.** Almost all of it lives in
`/services/Hypr.qml` (workspaces/toplevels/monitors/dispatch), plus `hyprctl` keyboard-layout,
`HyprlandFocusGrab`, `cursorpos`, "special workspaces". Quickshell ships a native **`I3`** backend
(i3/Sway IPC) — we replace the whole `Hypr.qml`, not port its internals. The one genuine gap with no
Sway equivalent is `HyprlandFocusGrab` (dismiss-on-focus-loss) → we implement a transparent
full-screen click-catcher (`Scrim`) + layer-shell keyboard-interactivity while a drawer is open.

---

## 3. Information architecture
The **Bar** is the always-present spine. The four product zones are surfaces invoked from the bar or a
keybind. Shrek already knows most state — zones are *display + ordinary actions*, never approval
prompts; no Windows-UAC-style interruption.

```
 BAR (layer top, exclusive)
  [1][2][3] workspaces      focused-window title        audio net bt   WORK-N   clock
   Quickshell.I3            ToplevelManager             StatusCluster   pill

    Super+D            Super+W            Super+S            (passive)
   LAUNCHER           WORK drawer        SYSTEM drawer      ATTENTION
   apps/actions       agent workloads    audio/net/bt/pwr   notif center + toasts
   (PLACES later)     + windows (RO)     quick controls     OSD = vol/brightness
```

- **PLACES** — DEFER to slice 2.
- **WORK** — existing `SessionProvider` stays the source; becomes toggleable; rows enriched with the
  tier/trust/state the provider already carries. **Read-only.**
- **ATTENTION** — Quickshell notification server → right-stack center + transient toasts. Real
  notifications only; no security nagging.
- **SYSTEM** — audio (Pipewire), Bluetooth (BlueZ), network **state** (networkd, read-only), power menu
  (logout/reboot/poweroff via `loginctl`/`systemctl`), brightness if hardware. Quick controls, not a
  settings app.

Interaction rules: one drawer open at a time (shared `ShellState` mutual-exclude); Escape or
click-outside closes; drawers are layer-shell `Top`, non-exclusive, animate in from their edge.

---

## 4. Target `ui/` layout (monolith kept; organized so surfaces can split later)
No swappable-units refactor. `shell.qml` stays the config-folder root; everything lives under `ui/`.

```
ui/
  shell.qml                         root shim (unchanged)
  shell/Shell.qml                   composition root: load services, Variants per screen, ShellState, mount surfaces
  services/
    Sway.qml            Quickshell.I3 -> workspaces, focusedWorkspace, focused window
    Applications.qml    Quickshell.DesktopEntries + JS fuzzy filter
    Audio.qml           Quickshell.Services.Pipewire -> default sink vol/mute
    Bluetooth.qml       Quickshell.Bluetooth -> adapter/devices (empty-OK in VM)
    Power.qml           Quickshell.Services.UPower -> battery (hw-gated, absent in VM)
    Network.qml         networkd link state (read-only; NM/Wi-Fi deferred)
    Notifications.qml   Quickshell.Services.Notifications (server)
    Session.qml         <- providers/SessionProvider.qml (unchanged logic, relocated)
    Ipc.qml             IpcHandler: toggleLauncher/Work/System, power
  surfaces/
    bar/         Bar.qml + Workspaces.qml WindowTitle.qml StatusCluster.qml WorkPill.qml Clock.qml
    launcher/    Launcher.qml + SearchField.qml AppList.qml AppRow.qml
    work/        WorkDrawer.qml (relocated, toggleable, enriched RO rows)
    system/      SystemDrawer.qml + VolumeControl.qml NetRow.qml BtRow.qml PowerMenu.qml
    notifications/ Center.qml Toast.qml
    osd/         Osd.qml
  components/    Panel.qml Anim.qml Pill.qml Icon.qml Scrim.qml
  themes/        Tokens.qml (expanded) qmldir
```

Build loop unchanged: edit `ui/` -> `build-desktop-layer.sh` -> `DOGFOOD=1 build-in-container.sh 1` ->
`build-layers.sh desktop` -> boot. The `layers/shrek-desktop/overlay/**/ui/` copies are gitignored
build artifacts — never edit those.

---

## 5. Package / runtime delta
**No new apt packages for slice 1** — `sway foot pipewire wireplumber bluez xdg-desktop-portal* qt6-*
fonts-dejavu-core` are already in the layer. The delta is Quickshell cmake flags (flip in
`scripts/build-desktop-layer.sh`, mirror in `scripts/desktop-smoke.sh`), enabled **incrementally** per
build phase:

| Flag | Slice 1 | Rationale |
|---|---|---|
| `I3` | ON | native Sway/i3 IPC; workspaces+focus, event-driven, no text parsing |
| `X11` | ON | **required for `I3`** — the i3/Sway IPC module lives under `src/x11` and only builds when `X11=ON` (verified in Quickshell v0.3.1 `src/CMakeLists.txt`). Links `libxcb` (build: `libxcb1-dev`, runtime: `libxcb1`) but runs **no X server** — Xwayland stays disabled; dormant client linkage only. Chosen over shelling out to `swaymsg` text (native = stable/fast/secure). |
| `WAYLAND_TOPLEVEL_MANAGEMENT` | ON | window titles/app-ids via wlr-foreign-toplevel (compositor-agnostic) |
| `SERVICE_PIPEWIRE` | ON | audio direct to libpipewire; lib already shipped |
| `SERVICE_NOTIFICATIONS` | ON | Quickshell *is* the notif server; one audited surface |
| `BLUETOOTH` | ON | native BlueZ D-Bus; `bluez` already shipped; empty-adapter-OK |
| `SERVICE_UPOWER` | ON | battery; cheap, hardware-gated |
| `SESSION_LOCK` `STATUS_NOTIFIER` `MPRIS` `NETWORK` `PAM` `POLKIT` `GREETD` `SCREENCOPY` `HYPRLAND` | OFF | deferred / not on path / networkd-not-NM; keep closure lean |

Shell-out survives only for `brightnessctl` (no backend; hw-gated). Flag names are `# VERIFY`-at-build
against the pinned Quickshell tag. Optional identity UI font is a later, non-blocking add.

---

## 6. Keybindings (added to `sway.config`)
```
set $mod Mod4
bindsym $mod+Return       exec foot
bindsym $mod+d            exec quickshell ipc call launcher toggle
bindsym $mod+w            exec quickshell ipc call work toggle
bindsym $mod+s            exec quickshell ipc call system toggle
bindsym $mod+q            kill
bindsym $mod+f            fullscreen
bindsym $mod+Shift+space  floating toggle
# focus + move: $mod+{Left,Down,Up,Right} / +h,j,k,l ; $mod+Shift+... to move
# workspaces: $mod+1..9 switch ; $mod+Shift+1..9 move-to
# media/brightness keys -> wpctl / brightnessctl (drive the OSD)
bindsym $mod+Shift+e      exec quickshell ipc call system power   # was raw `exit`
output * resolution ... position 0,0
```

---

## 7. Acceptance oracle (extends the existing 3-tier harness; same `SHREK_GATE` idiom)
Each surface logs a greppable marker on instantiate (as `Shell.qml` already does).

- **Tier 1 — `scripts/desktop-smoke.sh` (container headless, deterministic):** mirror the new cmake
  flags; assert Quickshell loads with **zero** QML errors (proves the enabled modules resolve). New
  markers: `bar-ready launcher-ready system-ready work-ready notif-server-registered osd-ready`. Live
  asserts against the real headless Sway: spawn `foot` -> bar `WindowTitle` marker updates; `swaymsg
  workspace 2` -> `Workspaces` marker reflects active; `notify-send` -> notification server logs
  receipt; `quickshell ipc call launcher toggle` -> visibility marker flips.
- **Tier 2 — `scripts/desktop-sealed-proof.sh` (KVM gate):** the updated sysext merges onto sealed
  dm-verity `/usr` and all new surfaces instantiate in a real boot (extend the `Pn-desktop-*` gates).
- **Tier 3 — `scripts/dogfood-vm.sh` (graphical screendump oracle):** boot graphically, toggle each
  drawer + launch an app, `screendump` PNGs = human-visible "you can operate it" evidence.

QML has no unit-test rig here, so gates are marker-grep + `swaymsg -t get_tree` assertions +
notify-send round-trip — all deterministic, matching the current pattern. No new test infra invented.

---

## 8. Build order (owner-split commits; provider-neutral tree; no Co-Authored-By)
Incremental, each phase independently bootable and gated:

1. **Sway operability + I3 bar** — real keybinds + output config; flip `I3` +
   `WAYLAND_TOPLEVEL_MANAGEMENT` ON; `services/Sway.qml`; Bar workspaces + window title + clock;
   `Ipc.qml` seam + `ShellState`. Fastest path to "it feels usable".
2. **Real launcher** — `services/Applications.qml` (DesktopEntries + fuzzy); `surfaces/launcher/*`;
   keyboard nav + launch; `Scrim` dismiss.
3. **SYSTEM + WORK** — flip `SERVICE_PIPEWIRE` `BLUETOOTH` `SERVICE_UPOWER` ON; `services/{Audio,
   Bluetooth,Power,Network}.qml`; `surfaces/system/*`; relocate + enrich the read-only Work drawer,
   make it toggleable; StatusCluster + WorkPill wire into the bar.
4. **Notifications + OSD** — flip `SERVICE_NOTIFICATIONS` ON; `services/Notifications.qml`;
   `surfaces/notifications/*` + `surfaces/osd/*`; bind media/brightness keys.

Each phase: build -> `desktop-smoke.sh` green -> commit. Sealed/KVM + graphical screendump proofs at
phase boundaries. Push to `constant-itis` on owner GO. `system-index refresh --graphify` + graph
baseline bump per landing.
