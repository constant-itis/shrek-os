# Omarchy → Shrek OS portability — what's worth taking

Companion to `docs/dms-functionality-catalog.md`. Both catalog the same question from
opposite ends: the DMS catalog = "what functionality does our shell already have";
this = "what does Omarchy have that's worth pulling in."

**Sources (read-only clones, MIT-licensed both):**
- Omarchy `basecamp/omarchy` @ `b86d450` — Arch + **Hyprland**, ships a Quickshell desktop
  under `shell/` (104 QML files, ~22 touch Hyprland). 22 themes, 437 `bin/` helpers.
- DMS `AvengeMedia/DankMaterialShell` @ `eadb8cf9` (our `dms=1.5.3db1`).

## Bottom line

DMS already covers ~80% of what Omarchy's shell does, usually **better** (launcher search,
clipboard, bar, notifications, plugin system, most panels). So the value is NOT "port
Omarchy" — it's a short list of **net-new capabilities DMS lacks**, plus **two reusable
patterns**. Everything else is duplication, Hyprland-only, or a sealed-OS / licensing
mismatch. Both repos MIT → literal code-lifting is legal (attribution appreciated).

Key intersection with the DMS catalog: several Omarchy pickups are gated on the **same
missing packages** the catalog already flagged (`wlsunset`, `cliphist`, `dgop`). Adopting
these and lighting up DMS's "bound-but-dead" backends are the *same package adds*, not
separate workstreams.

---

## Tier 1 — take these (net-new, low friction, high fit)

| Pickup | Path | Why | Effort |
|---|---|---|---|
| **AI-agent usage panel** | `shell/plugins/agents/` | Per-provider (Claude/Codex/Fireworks) usage dashboard, timer-driven collectors. **Zero Hypr coupling, no DMS equivalent.** Thematically perfect — Shrek is an agent-work OS; this is the desktop face of the CLI token ledger. **Highest fit-to-purpose pickup.** | Low — port + one `omarchy-agent-usage-*` collector |
| **Nested searchable command-menu engine** | `shell/plugins/menu/` (`Menu.qml`, `MenuModel.js`) + `default/omarchy/omarchy-menu.jsonc` | Declarative JSONC tree (dotted IDs → submenus), fuzzy-search flattens every leaf, per-row `when`/`checked`/`disabled` bash guards, submenus backed by live `provider` scripts. **DMS's DankLauncherV2 is a flat scored app-launcher — it has no browsable guarded command-tree.** Engine QML/JS is compositor-clean; only the menu *content* embeds Hypr/Arch action strings. | Med — reimplement against DMS IPC/providers; supply a Shrek-native `menu.jsonc` |
| **Night-light service** | `shell/plugins/services/nightlight/Service.qml` | DMS ships only a settings *tab* (`Settings/GammaControlTab.qml`), no service singleton — the one place DMS is genuinely thinner. Fills the catalog's night-mode hole. Swap `hyprctl hyprsunset temperature` (lines ~50-51,57) → **`wlsunset`**/`gammastep`. | Low |
| **Timed reminders** | `shell/plugins/reminders/` | Two-step input (minutes → message) → `omarchy-reminder` helper. DMS's Notepad is *persistent notes*, not timed reminders. Net-new, trivial. | Low |

## Tier 2 — take the pattern, rebuild the backend

- **Utility panels** — `shell/plugins/panels/{wifiqr,dropbox,speedtest,disk-speedtest}/`. No DMS
  equivalents. UI/parsing ports directly; each shells to `omarchy-*` helpers (nmcli/qrencode/
  stdlib-python, **not** hyprctl) that need thin reimplementations.
- **Wallpaper reveal-wipe** — `shell/plugins/background/Background.qml`. Pure `WlrLayershell`.
  DMS's wallpaper stack is otherwise richer; lift only the diagonal-wipe animation as an extra
  transition style.
- **Theming *mechanism* (not the data)** — `bin/omarchy-theme-set-templates` +
  `default/themed/*.tpl`. Zero-dependency bash+`sed`: one `colors.toml` → N app config formats
  via `{{ token }}` substitution, a resolver cascade (`mix()`, ANSI aliasing, luminance-based
  light/dark detect) so templates never hit an undefined key, and a **"hand-authored file
  always wins over generated"** rule + a `post_theme_commands` parallel retint list. This is a
  clean model for the piece **matugen/DMS does NOT solve**: propagating a palette out to
  *non-Quickshell* apps (foot, nvim, btop) on Sway. Relevant to sprint **S5**'s back half.
  (Omarchy uses **zero** matugen/pywal/wallust — 100% hand-curated palettes; it contributes
  nothing to the wallpaper→palette *generation* step, only to propagation.)

- **Terminal follows theme (foot) — a concrete S5 sub-task.** Shrek's `foot.ini`
  (`overlay/usr/share/shrek/xdg/foot/foot.ini`) is a **statically-baked swamp palette** — when
  S5/matugen recolors the shell, foot stays swamp-green unless propagation is added. Omarchy
  solves this with **two** paths worth lifting: (1) a `default/themed/foot.ini.tpl` colors
  template filled from the palette (for the on-disk file + **new** windows); (2)
  `bin/omarchy-theme-set-foot` + `omarchy-theme-osc` — generates **OSC escape sequences** from
  the palette and writes them straight to each running foot's child PTY (`/dev/pts/*`, via
  `pgrep -x foot` → child pids), retinting **already-open** windows **live, no restart, no lost
  scrollback**. OSC is a terminal standard, so it's foot-agnostic. Recommended shrek wiring,
  respecting sealed RO `/usr`: keep `foot.ini` in `/usr/share/shrek/xdg` but point its colors
  block at **`include=<writable path>`** on `/home` (e.g. `~/.local/state/shrek/foot-colors.ini`)
  that matugen writes on theme change (new windows), **plus** a ~30-line OSC-injection helper the
  theme-change hook calls (open windows). Wire this **as part of the S5 matugen slice** so foot
  isn't discovered swamp-locked later. (Omarchy's `INSTALLED_THEME_DENIED` list — stripping
  `foot.ini`/`alacritty.toml`/etc. from *cloned* themes — is a nice supply-chain guard to copy
  if Shrek ever allows user themes.)

## Tier 3 — inspiration only

- **22 curated palettes** (`themes/*/colors.toml`) — schema is flat-ANSI (16 terminal colors);
  DMS is Material-3 tonal roles (primary/surface/container tiers). No clean import — a port
  needs a lossy mapping layer. Use the hex choices as *seed values* for building DMS static
  named themes, nothing more.
- ⚠ **Do NOT bundle the wallpapers** — 92 images (~65MB), **no attribution/license file**;
  provenance unconfirmed. MIT covers the code, not necessarily the art. Verify with upstream
  before redistributing any.

## Skip entirely

- **Duplication (DMS equals or beats):** clipboard, emojis, image-picker, polkit, osd, bar
  chrome, plugin/widget registry, and panels for audio/bluetooth/clock/network/power/weather/
  monitor/tailscale.
- **Hyprland-only:** `plugins/bar/widgets/Workspaces.qml` (`hyprctl dispatch`),
  `KeyboardLayout.qml` (`hyprctl switchxkblayout`), `Ui/PopupCard.qml` (`HyprlandFocusGrab`),
  `Commons/Style.qml` (polls `hyprctl getoption decoration:rounding`/`gaps_out` — Sway has no
  equivalent getters), `plugins/services/idle/` (parses Hyprland raw window events — **use
  DMS's own clean `Services/IdleService.qml` instead**), `plugins/panels/monitor/` (`hyprctl
  keyword monitor` — DMS `DisplayService` is deeper).
- **Sealed-OS mismatch:** Omarchy's `install/` machinery, pacman hooks, and the 437 `bin/`
  helpers all assume a *mutable Arch* box with live package installs. The single-purpose
  helper-script *convention* is fine, but on Shrek they bake into the image overlay — do NOT
  copy the installer/live-mutation model.

---

## Recommended sequencing

1. **`agents/` panel first** — highest fit, zero blockers, distinctly Shrek. Quick win.
2. **Night-light + reminders** — small net-new pickups; night-light rides the same `wlsunset`
   add the DMS catalog already wants.
3. **Menu engine** — highest-effort/highest-delight; scope a proper feasibility pass on how it
   binds to DMS IPC/providers + what a Shrek `menu.jsonc` looks like before building.

---

## Appendix — Menu engine build plan (feasibility done)

Bringing Omarchy's nested-searchable command-menu (`shell/plugins/menu/` +
`default/omarchy/omarchy-menu.jsonc`) into DMS. **Verdict: GO.**

**Integration seam — a shrek-owned standalone Quickshell surface, NOT a DMS plugin.**
Evidence-backed, not a routing-around: DMS's plugin schema
(`PLUGINS/plugin-schema.json`) allows only `widget|daemon|launcher|desktop|composite` — no
`surface`/`popup` type, and **no plugin ever instantiates a top-level `PanelWindow`/
`WlrLayershell`**. The menu needs a centered, freely-sized, exclusive-keyboard-focus overlay,
which the plugin API structurally cannot host. A `daemon` plugin can own an `IpcHandler` but has
nowhere to render. Forking `Modals/DankLauncherV2/` (~9,300 lines, flat query→results model) is
rejected — permanent re-diff against every DMS release. So: a new QML entrypoint (own
`PanelWindow`+`WlrLayershell`) baked into `layers/shrek-desktop/overlay/usr/share/shrek/dms/`,
launched by Sway alongside `dms run`, reusing DMS's `qs.Common` `Theme`/`Color` for visual
parity, exposing its own `IpcHandler { target: "shrek-menu" }`. Only coupling is to
`qs.Common` — immune to plugin-API churn. (DMS's IPC registry is a closed hardcoded set in
`DMSShellIPC.qml`; the standalone surface declares its own handler, so it needs nothing from it.)

**Ports vs. rebuilds (both repos MIT):**
- `MenuModel.js` — **ports ~verbatim** (pure JS: tree flatten, fuzzy score, route resolve,
  guard-batch gen; zero Hypr coupling). Only edits: swap the hardcoded `omarchy-*` `GUARD_READERS`
  allowlist + rewrite `guardHelpers()` from `pacman -Q` → `command -v`/`dpkg -l`.
- `Menu.qml` — **ports with surgery**: UI/delegate/keyboard-nav/search lift; rewrite the
  plugin-lifecycle shim → standalone `ShellRoot`+`IpcHandler`, `Util.execDetached` →
  `Quickshell.execDetached`, the `providers` map, and app-library → DMS's app-entry service.
- `omarchy-menu.jsonc` — **full rewrite** (tree *shape* is a fine skeleton; every action is an
  Arch/Hypr CLI). Starter tree below.
- `BarWidget.qml` / `manifest.json` — don't port (optional bar pill, or just a keybind).

**Sealed-OS fit — clean.** Bake read-only `/usr/share/shrek/dms/menu/menu.jsonc`, merge an
optional `~/.config/shrek/menu.jsonc` from `/home` (Omarchy already does default+user per-key
merge; `FileView{watchChanges:true}` gives free live-reload of the writable file). `when`/
`checked`/`disabled` guards batch into **one subprocess per menu (re)load** (not per-row, not
per-keystroke; opens on cached eval so it never blocks) — safe on sealed, just re-derive
"package present" → **command/unit/`/home`-state present** since nothing installs post-boot.
**Security: `provider` must accept only a fixed baked-in string key, never a path/command** —
enforced by review, so a `/home`-writable override can *select* a vendor provider but never inject
a script. (Action strings are arbitrary shell, same trust model as Omarchy: only vendor-`/usr` +
the user's own `/home` file feed the tree.)

**Invocation:** `bindsym $mod+slash exec dms ipc call shrek-menu toggle` (same pattern as the
existing `dms ipc call spotlight toggle` binds in `sway.config`).

**Starter `menu.jsonc`** (uses only shrek's wired backends — S1 power/polkit, S2 network, S5
theming, grim/slurp, wpctl):

```jsonc
{
  "apps":    {"icon":"apps","label":"Apps","provider":"apps"},
  "system":  {"icon":"power_settings_new","label":"System","aliases":["power"]},
  "network": {"icon":"wifi","label":"Network"},
  "style":   {"icon":"palette","label":"Style"},
  "capture": {"icon":"photo_camera","label":"Capture"},
  "audio":   {"icon":"volume_up","label":"Audio"},

  "system.lock":     {"icon":"lock","label":"Lock","action":"loginctl lock-session"},
  "system.suspend":  {"icon":"bedtime","label":"Suspend","action":"systemctl suspend"},
  "system.reboot":   {"icon":"restart_alt","label":"Reboot","action":"dms ipc call powermenu open reboot"},
  "system.shutdown": {"icon":"power_settings_new","label":"Shutdown","action":"dms ipc call powermenu open shutdown"},

  "network.toggle-wifi": {"icon":"wifi_off","label":"Toggle Wi-Fi",
    "checked":"nmcli radio wifi | grep -q enabled",
    "action":"nmcli radio wifi $(nmcli radio wifi | grep -q enabled && echo off || echo on)"},

  "style.theme":     {"icon":"palette","label":"Cycle Theme","action":"dms ipc call theme cycle"},
  "style.wallpaper": {"icon":"wallpaper","label":"Wallpaper","provider":"wallpapers"},

  "capture.screenshot": {"icon":"screenshot","label":"Screenshot (select)",
    "action":"grim -g \"$(slurp)\" ~/Pictures/Screenshots/$(date +%Y%m%d-%H%M%S).png"},

  "audio.mute": {"icon":"volume_off","label":"Mute","when":"command -v wpctl",
    "checked":"wpctl get-volume @DEFAULT_AUDIO_SINK@ | grep -q MUTED",
    "action":"wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle"}
}
```
(`apps` provider → DMS app-entry service, native rows; `wallpapers` → small baked `/usr` script
enumerating the seeded wallpaper dir, `label\tvalue\tcurrent` rows.)

**Effort: M, ~3-5 focused days** given a bootable image to iterate against; ceiling is content/
guard authoring, not engine porting. **Top risks:** (1) no DMS precedent for a shrek-owned surface
sharing DMS's Theme/process — second `qs` process vs. `Loader` spliced into DMS's tree is an open
call; (2) guard reliability on sealed (re-derive to command/unit presence, hold the ~1s/open
budget); (3) provider-allowlist must be enforced, not just documented.

**FIRST STEP (retires the only estimate-invalidating unknown):** a one-day spike standing up the
*empty* surface — minimal `PanelWindow`/`WlrLayershell` that opens via `dms ipc call shrek-menu
toggle`, renders with `qs.Common` `Theme`/`Color` for DMS parity, closes on Escape. Prove the
shell works and themes correctly **before** porting any `MenuModel.js`.

---

*Compiled 2026-08-24 from Omarchy `b86d450` + DMS `eadb8cf9`, cross-checked against the Shrek
image package lists. Sonnet sub-agents (shell + theming + menu-engine feasibility) + direct
launcher/menu/foot/wallpaper analysis; tooling findings folded in from direct inspection.*
