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
- **Model port (`MenuModel.js`): DONE, validated.** Ported ~verbatim from Omarchy; the only changes are
  the guard batch (`GUARD_READERS` emptied, `guardHelpers()` rewritten pacman→sealed-Debian — see
  *Guard vocabulary* below). Validated standalone under `node` (`tests/menu-model.test.js`, 22 checks):
  jsonc parse / merge / alias-route / fuzzy-search all pass, and the generated guard batch is valid
  bash that runs on the build host with the correct `<id>:<w|c|d>:<0|1>` contract. No QML runtime needed
  — the file keeps its `module.exports` block, so the exact runtime file is the test target.
- **Content (`menu.jsonc`): DONE, validated.** The baked tree
  (`.../shrek-menu/menu.jsonc`) is written on verified-baked backends: `apps` (provider) · `system`
  (DMS `lock lock` + logind suspend/reboot/poweroff + `swaymsg exit`) · `network` (nmcli Wi-Fi toggle,
  `nmtui`) · `style` (DMS `theme toggle`, wallpaper provider, settings) · `capture` (grim/slurp) ·
  `audio` (DMS `audio mute`/`micmute`, state read with wpctl). Validated via `tests/menu-jsonc.test.js`:
  parses, nests with no orphans, providers stay bare baked keys (`apps`, `wallpapers` — the security
  invariant), aliases route, and the guard batch is valid bash that runs with the right contract. IPC
  verbs were taken from DMS source, not guessed (the appendix's `theme cycle` / `powermenu open reboot`
  were wrong — real verbs are `theme toggle` / `lock lock` / `audio mute`).
- **Next:** port `Menu.qml` with surgery into `shell.qml`'s existing `ShellRoot`+`IpcHandler` — this
  wires `MenuModel.js` + `menu.jsonc` into a rendered, searchable, keyboard-driven surface, honors the
  `apps`/`wallpapers` providers, and merges an optional `~/.config/shrek/menu.jsonc` from `/home`.

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

## Guard vocabulary (`when` / `checked` / `disabled`)

Every `when`/`checked`/`disabled` expression in `menu.jsonc` is a bash test evaluated once per menu
(re)load in a **single** subprocess (`MenuModel.guardScript()`), never per-row or per-keystroke — the
menu opens on the last evaluation's cached answers and never blocks. Expressions are arbitrary bash, so
anything that exits 0/nonzero works, but the batch preloads these Shrek helpers (rewritten from
Omarchy's pacman-based ones for a **sealed Debian** image — nothing installs post-boot, so "present" is a
fixed fact derived from dpkg / `command -v` / systemd):

| Helper | True when | Backed by |
|---|---|---|
| `shrek-pkg-present PKG…` | every PKG is installed (Provides-aware) | one `dpkg-query -W` load, hash lookup |
| `shrek-pkg-missing PKG…` | any PKG is absent | same |
| `shrek-cmd-present CMD…` | every CMD is on `PATH` | `command -v` |
| `shrek-cmd-missing CMD…` | any CMD is absent | `command -v` |
| `shrek-unit-active UNIT…` | every UNIT is active | `systemctl is-active` |
| `shrek-unit-enabled UNIT…` | every UNIT is enabled | `systemctl is-enabled` |

Package presence is loaded once into an associative array (same optimization Omarchy got from
`pacman -Qq`), so a menu with many `shrek-pkg-present` guards still forks only once. Inline commands
(`nmcli radio wifi | grep -q enabled`, `command -v wpctl`) are fine too and run directly.
`GUARD_READERS` (Omarchy's memoized `omarchy-default-*` readers) is **empty** — Shrek ships none yet;
append to that array in `MenuModel.js` to add a fast-path reader later.

## Port plan (remaining)

Done: `MenuModel.js` (engine) and `menu.jsonc` (content) — see Status. The one remaining slice:

- **`Menu.qml`** — port with surgery: keep UI/delegate/keyboard-nav/search; rewrite the plugin
  lifecycle → the standalone `ShellRoot`+`IpcHandler` already in `shell.qml`, `Util.execDetached` →
  `Quickshell.execDetached`, the `providers` map (honoring the `apps`/`wallpapers` keys menu.jsonc
  already declares), and app-library → DMS's app-entry service. This slice wires `MenuModel.js` +
  `menu.jsonc` into a live surface and adds the optional `~/.config/shrek/menu.jsonc` `/home` merge.
- **Security (holds through the port):** a `provider` must resolve only a fixed, baked-in string key —
  never a path or command — so a `/home`-writable override can *select* a vendor provider but never
  inject a script. The `providers` map in `Menu.qml` is where this is enforced.
