# Shrek Shell — Theme System

The shell is themed through **one stable semantic-token contract** with **many interchangeable palette
sources**. Dynamic (wallpaper-derived) is the default/recommended experience; it is not the only one. A
user can pick a curated or custom mode and pin individual roles, and no component can bypass the contract.

## The contract

`ui/themes/Tokens.qml` is the singleton every surface consumes. Its **colour role NAMES are the stable
contract**; their VALUES are resolved per-mode by the theme system. Surfaces reference role names only
(`Tokens.accent`, `Tokens.surface`, …) — never a raw colour, never a palette source.

The semantic colour roles (`Palettes.js:SEMANTIC_KEYS`):

```
bg surface surfaceAlt overlay border borderStrong    — planes & structure
barBg panelBg rowHi                                   — floating surfaces (may be #AARRGGBB) + selection
text textDim textFaint                                — foreground tiers
accent accentDim accentText                           — identity accent + on-accent foreground
notice danger ok                                      — status
dangerHover scrim                                     — danger-row hover tint + modal scrim
```

Type scale, spacing, rounding, sizing and animation timing also live in `Tokens` but are **fixed identity**
— they are not colour-themed.

## Resolution flow

```
                 mode (theme.json)
                       │
      ┌────────────────┼───────────────────────────┐
 dynamic          curated / custom              (base source)
 Colours.scheme   palettes/<mode>.json                │
 (wallpaper)      custom.json                          │
      └────────────────┬───────────────────────────┘
                       ▼
        merge(base, user overrides)          ← overrides win, per semantic key
                       ▼
        complete(…, shrek-dark floor)        ← fill any missing key; contract is NEVER incomplete
                       ▼
                   Theme.c                    ← resolved full palette
                       ▼
                   Tokens.*                   ← the stable contract
                       ▼
                surfaces / shell              ← pure consumers
```

Files (`ui/themes/`):

| File | Role |
|------|------|
| `Tokens.qml`   | Stable semantic contract. Colour roles are passthroughs to `Theme.c`; everything else fixed. |
| `Theme.qml`    | Controller. Reads config, selects the base by mode, merges overrides, guarantees completeness, exposes `c`. Also pushes sway window-chrome. |
| `Colours.qml`  | Dynamic source. `FileView`-watches the wallpaper-derived scheme and republishes it as `scheme`. |
| `Palettes.js`  | `SEMANTIC_KEYS`, the compiled-in `BOOTSTRAP` floor (mirrors `shrek-dark.json`), and `merge`/`complete`. |
| `palettes/*.json` | Curated palettes — the single source of truth (also readable outside QML by tooling). |

## Modes

| Mode | Base source |
|------|-------------|
| `dynamic` (default) | `~/.local/state/shrek/colours.json` (matugen, wallpaper-derived — Phase B). Absent ⇒ falls back to the shrek-dark floor, so dynamic is always safe. |
| `shrek-dark`   | `palettes/shrek-dark.json` — the fixed swamp identity. |
| `shrek-light`  | `palettes/shrek-light.json` — light swamp. |
| `high-contrast`| `palettes/high-contrast.json` — WCAG-AAA, opaque surfaces, white borders. |
| `custom`       | `~/.config/shrek/custom.json` — a full or partial palette of the same shape; gaps fall back to the floor. |

## Config

`~/.config/shrek/theme.json`:

```json
{ "mode": "dynamic", "overrides": { "accent": "#7cb518" } }
```

- `mode` selects the base palette source.
- `overrides` is a **semantic-keyed partial that merges over ANY mode**. This is how override is preserved
  *without* bypass: a user changes the value bound to a role name; the component still only ever sees
  `Tokens.<role>`. `mode: "custom"` is simply the heavy-override case over a user base.

`SHREK_THEME_CONFIG` overrides the config path (used by the preview harness to render each mode).

### `shrek-theme` CLI (staged to `/usr/bin`)

```
shrek-theme set <mode> [key=#hex ...]   # e.g. shrek-theme set dynamic accent=#7cb518
shrek-theme show
shrek-theme apply
```

Writes `theme.json`; the running shell watches it, repaints live, and re-syncs sway chrome.

## No-bypass enforcement

`scripts/check-tokens.sh` (wired into `scripts/qml-check.sh` as a fail-fast pre-step) fails the build if
anything under `ui/surfaces/**` or `ui/shell/**`:

1. contains a raw colour hex literal, or
2. references a palette source directly (`Theme.` / `Colours.` / `Palettes.`).

Only `ui/themes/**` may hold raw colour or touch the sources. The chokepoint is mechanical, not a
convention.

## Sway window chrome

Window borders live in sway, outside QML. Rather than a config `include` on the read-only sealed `/usr`,
`Theme.qml` pushes the active palette to sway at runtime via `swaymsg client.*` (the shell inherits
`SWAYSOCK` from the session that spawned it), on every palette change and at startup. `sway.config` keeps a
static shrek-dark default for first paint before the shell is up.

## Dev loop

```
scripts/check-tokens.sh      # enforcement gate (ms)
scripts/qml-check.sh         # headless load (runs check-tokens first)
scripts/theme-preview.sh     # render + screenshot EVERY mode → out/preview/theme-<mode>-{bar,launcher}.png
```

## Phase B (next slice): the dynamic engine

Stage **matugen** (Rust, single static binary) into the desktop layer like the quickshell binary. A
`shrek-theme`-adjacent hook runs matugen against the active wallpaper → writes `colours.json` (semantic-keyed)
→ `Colours.qml` republishes → the shell (in `dynamic` mode) repaints. No QML change required; the consumer
seam already exists. matugen can also template a sway include if the runtime-push approach is ever retired.
