# Menu engine — a shrek-owned command surface

Bringing Omarchy's nested-searchable command-menu into Shrek OS as a **shrek-owned standalone
Quickshell surface** launched alongside DMS. Feasibility (GO) and the port breakdown are in
`docs/omarchy-portability.md` (Appendix); this doc records the **resolved architecture** after the
spike and is the build reference going forward.

## Status

- **Spike: DONE, verified live.** An empty surface opens via IPC, renders centered over the desktop,
  grabs keyboard focus, and closes on Escape — confirmed on the real image (packaged quickshell 0.3.0),
  dogfood still 60/0. This retired the only estimate-invalidating unknown.
- **Theme parity: DONE, verified live.** The surface reads DMS's `dms-colors.json` and follows theme
  changes in place — proven on the 0.3.0 image by atomically rewriting the file under a running
  instance and watching the card recolor with no relaunch, dogfood still 60/0. Schema resolved (below).
- **Next:** port the engine (`MenuModel.js` ~verbatim, `Menu.qml` with surgery) + write the shrek
  `menu.jsonc`.

## Architecture (three unknowns, resolved from source)

The surface is a **second `qs` process**, not a DMS plugin and not spliced into DMS's instance. Why,
with evidence:

1. **IPC routing — a separate process, addressed by config path.** Quickshell IPC target names are
   scoped to a single instance (`quickshell` `command.cpp::selectInstance` selects by `--pid`/`--id`/
   config-path hash *before* target lookup), and `dms ipc` is hard-pinned to DMS's own pid/config
   (DankMaterialShell `core/cmd/dms/shell.go::buildQsIPCBaseArgs`). **`dms ipc call shrek-menu` cannot
   reach another process.** So the menu is toggled with:
   ```
   qs -p /usr/share/shrek/dms/shrek-menu/shell.qml ipc call shrek-menu toggle
   ```
   addressing *this* instance by its config path. (A DMS plugin can't host it anyway: the plugin
   schema has no surface/popup type and no plugin instantiates a top-level `PanelWindow`.)

2. **Theme parity — read the colors file, don't import DMS's singletons.** Cross-instance
   `import qs.Common` is **blackholed** by quickshell's `qs://` URL interceptor
   (`src/core/qsintercept.cpp` resolves `qs.*` under *this* instance's config root only; anything
   outside is sent to `qrc:/qs-blackhole`). So a shrek-owned surface cannot borrow DMS's `Theme`/
   `Color`. Parity instead comes from reading `~/.cache/DankMaterialShell/dms-colors.json` (DMS's
   matugen output). **Schema (verified live against `dms=1.5.3db1`):**
   `{ colors: { dark|light: { <M3 semantic key>: "#rrggbb" } }, dank16: {...} }` — flat hex values, the
   50 standard matugen M3 keys (`surface_container_high`, `on_surface`, `primary`, `outline_variant`, …).
   Matugen writes it atomically (`.tmp`+rename), so a watched read never sees a partial file. The
   surface mirrors DMS's own `dynamicColorsFileView` (`Common/Theme.qml`):
   `FileView{ watchChanges:true; onFileChanged: reload(); onLoaded: reparse() }`. **Use `onLoaded`, not
   `onLoadedChanged`** — `loaded` transitions `false→true` only on the first load, so `onLoadedChanged`
   never re-fires on subsequent reloads and the card would freeze at the launch palette. A baked
   swamp-green palette is the fallback when the file/key is absent (first boot, pre-matugen).

3. **Launch + window.** Sway autostarts it alongside `dms run`; it stays hidden until toggled:
   ```
   exec_always qs -p /usr/share/shrek/dms/shrek-menu/shell.qml -n
   ```
   The window is a `PanelWindow` with **no anchors** (wlr-layer-shell centers it), `visible:false`,
   `color:"transparent"`, `WlrLayershell.layer: WlrLayer.Overlay`,
   `WlrLayershell.keyboardFocus: WlrKeyboardFocus.Exclusive`, `namespace:"shrek-menu"`, and an
   `IpcHandler{ target:"shrek-menu"; function toggle():void ... }`. `IpcHandler` is in module
   **`Quickshell.Io`**; `WlrLayershell`/`WlrLayer`/`WlrKeyboardFocus` in `Quickshell.Wayland`.

## Files

- `layers/shrek-desktop/overlay/usr/share/shrek/dms/shrek-menu/shell.qml` — the surface (desktop
  sysext; `/usr` only, so it lives in the desktop layer, not the base image).
- `layers/shrek-desktop/overlay/usr/share/shrek/desktop/sway.config` — the `exec_always` autostart
  and the `$mod+slash` / `Mod1+slash` toggle binds (Alt fallback because a host compositor can eat
  Super before the guest sees it).

## Validating changes

Load the QML headless with the cached from-source binary before baking (catches import/type errors):
run `out/qs-cache/quickshell -p <shell.qml>` under a headless Sway in `debian:trixie`
(`WLR_BACKENDS=headless WLR_RENDERER=pixman`, `QT_QPA_PLATFORM=wayland QT_QUICK_BACKEND=software`,
`LC_ALL=C.UTF-8`), and grep for `Failed to load configuration` (fail) vs `Configuration Loaded`
(pass). The cached binary is 0.3.1; the image ships 0.3.0 — core windowing API matches, but always
**verify layershell render live** on the graphical domain.

## Port plan (next)

- **`MenuModel.js`** — port ~verbatim (pure JS: tree flatten, fuzzy score, route resolve, guard-batch
  gen). Only edits: swap the `omarchy-*` `GUARD_READERS` allowlist and rewrite `guardHelpers()` from
  `pacman -Q` → `command -v`/`dpkg -l` (re-derive "package present" → command/unit/`/home`-state
  present, since nothing installs post-boot).
- **`Menu.qml`** — port with surgery: keep UI/delegate/keyboard-nav/search; rewrite the plugin
  lifecycle → the standalone `ShellRoot`+`IpcHandler` already in `shell.qml`, `Util.execDetached` →
  `Quickshell.execDetached`, the `providers` map, and app-library → DMS's app-entry service.
- **`menu.jsonc`** — full rewrite; the starter tree (power/lock/network/theme/capture/audio on
  shrek's wired backends) is in `docs/omarchy-portability.md`. Bake read-only under
  `/usr/share/shrek/dms/shrek-menu/`, merge an optional `~/.config/shrek/menu.jsonc` from `/home`.
- **Security:** a `provider` must accept only a fixed, baked-in string key — never a path or command
  — so a `/home`-writable override can *select* a vendor provider but never inject a script.
