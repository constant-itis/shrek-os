# DMS Functionality Catalog — what DankMaterialShell ships vs. what Shrek OS wires

DMS = **DankMaterialShell** `dms=1.5.3db1` (AvengeMedia OBS), pinned upstream commit
`eadb8cf9`. It is the *running* shell (`sway.config: exec dms run`); the old `ui/`
Quickshell tree is vestigial. Scope of DMS: **~45 IPC targets, ~22 UI modules, ~62 backend
services.** The sprint's 7 slices touch maybe a third of it. This catalogs the whole surface
so we can decide what to wire, what to disable, and what to leave alone.

## How it's driven

- **`dms run`** launches the Quickshell config. Everything is triggered by
  **`dms ipc call <target> <method> [args]`** (Quickshell `IpcHandler`s). The full target
  list is in `quickshell/DMSShellIPC.qml` plus per-module handlers.
- **Config seams** (first-run-seeded, then persistent in `~/.local/state/DankMaterialShell/`):
  - `default-session.json` → wallpaper (path / per-monitor / per-mode / cycling / transitions),
    light-dark auto mode.
  - `default-settings.json` → theme name+scheme, **matugen** toggle, blur, elevation, corner
    radius, animation speeds, audio visualizer, media album-art accent, **dashTabs**
    (overview/media/wallpaper/weather/settings — each toggleable).
- **Bound in shrek today** (`sway.config`): `spotlight`, `clipboard`, `processlist`,
  `settings`, `notifications`, `audio` (vol keys), `brightness` (bright keys). Everything
  else below is **reachable via IPC but unbound**, or surfaced in the bar/control-center UI.

## Status legend

- ✅ **works** — backend present in the image, surface functional.
- 🔑 **bound-but-dead** — keybind/UI exists, backend missing → clicks nothing.
- 🧩 **available-unwired** — DMS ships it, no keybind, backend may/may not be present.
- 🛠 **sprint** — tracked in `docs/desktop-functionality-sprint.md`.
- 🚫 **compositor-gated** — DMS feature that targets niri/Hyprland; Sway won't honor it.

---

## 1. Core launchers & search  ✅ mostly working

| Surface | IPC target | Backend service | Dep | Status |
|---|---|---|---|---|
| Spotlight / app launcher | `spotlight`, `launcher`, `dash` | AppSearchService, DSearchService | desktop entries | ✅ bound `$mod+space` |
| Clipboard history | `clipboard` | ClipboardService | **`cliphist`** (+ wl-clipboard ✅) | 🔑 bound `$mod+v` — **cliphist NOT in image → only live copy, no history** |
| Process list / task mgr | `processlist` | DgopService | **`dgop`** | 🔑 bound `$mod+m` — **dgop NOT in image → dead** |
| Settings panel | `settings` | (many) | — | ✅ bound `$mod+comma` |
| Notifications center | `notifications` | NotificationService | Quickshell native | ✅ bound `$mod+n` |
| Spotlight-bar (cmd-palette toasts) | `spotlight-bar`, `toast` | ToastService | native | 🧩 unwired |

## 2. Power / session  🛠 S1 DONE

| Surface | IPC | Service | Dep | Status |
|---|---|---|---|---|
| Shutdown / reboot / suspend | `powermenu` | SessionService (loginctl/systemctl) | **polkit** ✅ | ✅ **S1 fixed** — Off/Reboot/Suspend work |
| Logout / session switch | `sessions` | SessionsService (loginctl) | logind | ✅ present (autologin single-user) |
| Idle inhibit | `inhibit` | IdleService | logind | 🧩 unwired |
| Login greeter | — | GreeterService | **`dms-greeter`/greetd** | 🧩 DMS can *be* the login screen — not used (shrek autologins) |

## 3. Network  🛠 S2 DONE (+ extras DMS ships)

| Surface | IPC | Service | Dep | Status |
|---|---|---|---|---|
| Wi-Fi / ethernet center | (control-center) | NetworkService, DMSNetworkService | **NetworkManager** ✅ (base) | ✅ **S2 done** — list/toggle/connect + persist |
| VPN | — | VPNService | NM VPN plugins | 🧩 unwired — no VPN plugin pkgs |
| **Tailscale** | — | TailscaleService | **`tailscale`** | 🧩 DMS has native Tailscale UI — **not in image** (notable: shrek infra *is* tailnet-based) |

## 4. Lock + idle  🛠 S3 IN PROGRESS

| Surface | IPC | Service | Dep | Status |
|---|---|---|---|---|
| Lock screen | `lock` (lock/unlock/demo/status) | Lock module | Quickshell **SESSION_LOCK** backend + **PAM** | 🛠 S3 — verify QS built with SESSION_LOCK; PAM auth is the knot |
| Auto-lock on idle | — | IdleService | **`swayidle`** + logind idle | 🛠 S3 — swayidle NOT in image |
| Lock-on-suspend | — | IdleService/SessionService | logind | 🛠 S3 |

## 5. Storage  🛠 S4 DONE

| Surface | IPC | Service | Dep | Status |
|---|---|---|---|---|
| Mount attached disks / USB | (file manager) | (no DMS core svc — udisks path) | **udisks2** ✅ (base) + polkit | ✅ **S4 done** — attach→mount→open |
| Trash | — | TrashService | **`trash-cli`**/gio + a file mgr | 🧩 unwired — not in image |

## 6. Audio / media  ✅ working, one gap

| Surface | IPC | Service | Dep | Status |
|---|---|---|---|---|
| Volume / mute / output cycle | `audio` | AudioService | PipeWire/WirePlumber ✅ | ✅ bound (vol keys) |
| Mic | `mic` | AudioService | PipeWire ✅ | 🧩 unwired keys |
| Media transport (play/pause/next) | `mpris` | MprisController | native MPRIS | ✅ Media dash tab; keys unwired |
| Album-art accent | — | MediaAccentService, TrackArtService | native | 🧩 off in settings |
| **Audio visualizer** | — | CavaService | **`cava`** | 🧩 off in settings — cava NOT in image |

## 7. Display / theming

| Surface | IPC | Service | Dep | Status |
|---|---|---|---|---|
| Brightness | `brightness` | DisplayService | **`brightnessctl`** (+ `ddcutil` for ext. monitors) | 🔑 keys bound — **brightnessctl NOT in image** (🛠 S6, real-HW only) |
| Night mode / color temp | `night` | DisplayService | **`wlsunset`/`gammastep`** | 🔑 UI present — **no temp tool in image → dead** |
| **Dynamic theming (matugen)** | `theme`, `color-picker` | Theme, DisplayService | **`matugen`** binary | 🛠 **S5** — `runUserMatugenTemplates:false`, binary not staged |
| Light/dark toggle | `theme` (light/dark/toggle) | ThemeAutoService | native | ✅ works (static schemes) |
| Wallpaper picker + cycling | `wallpaper` (get/set/next/screenshot) | WallpaperCyclingService | native | ✅ set/cycle work; recolor needs S5 |
| Power profiles | `powerprofile` | PowerProfileWatcher | **`power-profiles-daemon`** | 🔑 UI present — **not in image** (🛠 S6) |

## 8. Productivity / desktop widgets  ✅ mostly pure-QML (work as-is)

| Surface | IPC | Dep | Status |
|---|---|---|---|
| Notepad (persistent notes) | `notepad`, `dash openFile` | NotepadStorageService (native) | ✅ likely works |
| Do-Not-Disturb + scheduling | (notifications handler) | native | ✅ works |
| Calendar | — (CalendarService) | **`khal`** for events | 🧩 grid works; **no khal → no events** |
| Weather tab | — (WeatherService) | **`curl` + network egress** | 🛠 **S7** — needs sealed-egress decision |
| Printing | `systemupdater`?/Cups | **`cups`** | 🧩 UI present — **cups NOT in image** |
| Color picker | `color-picker` | native | ✅ |
| Screenshot (screen/window/region) | `wallpaper screenshot*` | grim ✅/slurp ✅ | ✅ |
| Desktop widgets / welcome overlay | `desktopWidget`, `welcome` | native | 🧩 unwired |
| Dank Island (dynamic-island notch) | `island` | native | 🧩 unwired, pure-QML |
| Dock | `dock` | native | 🧩 unwired, pure-QML |

## 9. System management  ⚠ evaluate for a sealed immutable OS

| Surface | IPC | Dep | Status / note |
|---|---|---|---|
| **System updater** | `systemupdater` | distro pkg mgr | ⚠ **conceptually wrong for Shrek** — sealed image updates A/B via image, not apt. Disable/hide. |
| **Plugin system** (3rd-party widgets) | `plugins`, `widget`, `plugin-scan`, `file` | fetches/loads external QML | ⚠ **security surface** on a sealed OS — decide policy (disable?) |
| Users management | — (UsersService, UserInfoService) | accountsservice | 🧩 single dev user; low value |
| Tray (StatusNotifier) | `tray` | SNI apps | ✅ backend present |
| Bluetooth | (control-center) | BluetoothService | **bluez** ✅ + wpexec | ✅ works |
| Battery | — | BatteryService | **upower** ✅ | ✅ works |

## 10. Compositor-gated  🚫 (Shrek runs Sway, not niri/Hyprland)

DMS ships HyprlandService, NiriService, LabwcService, MangoService + CompositorService.
These IPC targets/features assume niri or Hyprland and **won't work on Sway**:

- `hypr wallpaper`, `niri open/close/toggle` — compositor-specific.
- `window-rules` (profiles/cycle/auto) — niri/hypr window-rule engines.
- `workspace-rename`, `keybinds toggleOverview` / WorkspaceOverlays — overview is niri/hypr.
- Per-window rounding / compositor blur / true Material-You live blur — Hyprland-only
  (already flagged to owner in earlier checkpoints). `blurEnabled:false` in settings is correct.

Verify DMS's Sway support level in CompositorService — bar/dash/control-center are
compositor-agnostic and confirmed working; the above are the known dead zones.

---

## Rollup — the "lot of functionality" to decide on

**Quick wins (add one package, feature lights up):**

| Feature | Add | Effort |
|---|---|---|
| Clipboard *history* (not just live copy) | `cliphist` | trivial — key already bound |
| Process list / system monitor | `dgop` | trivial — key already bound (may need OBS/AvengeMedia pkg) |
| Night mode / eye-comfort | `wlsunset` | trivial |
| Calendar events | `khal` | small (also needs a calendar source) |
| Trash support | `trash-cli` | trivial |
| Audio visualizer | `cava` | trivial (then flip `audioVisualizerEnabled`) |

**Already sprint-tracked:** matugen theming (S5), brightness + power-profiles (S6, real-HW),
weather egress (S7), lock+idle/swayidle (S3, in progress).

**Decisions needed (not just packaging):**

- **System updater** — hide/disable; sealed A/B image, not a package-managed distro.
- **Plugin system** — external-QML load on a sealed OS is a security-model question. Default off?
- **Weather / Tailscale / VPN** — all need *shell egress* through the sealed network plane
  (same decision as S7). Tailscale is interesting given shrek's own tailnet infra.
- **Greeter** — DMS could serve the login screen (`dms-greeter`/greetd); shrek autologins today.

**Leave alone:** compositor-gated niri/Hyprland features (§10) unless shrek ever switches
compositor; per-window rounding/blur are Hyprland-only.

---
*Source: DMS `eadb8cf9`, cross-checked against `image/mkosi.conf` +
`layers/shrek-desktop/mkosi.conf` + `sway.config` on branch `installer-0`, 2026-08-24.*
