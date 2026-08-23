pragma Singleton
import QtQuick

// Shrek design tokens (Desktop Slice 1). THE stable semantic contract the shell consumes: colour roles,
// type scale, spacing, rounding, and animation timing, so surfaces never hard-code values inline. The
// COLOUR roles are now resolved by the theme SYSTEM (themes/Theme.qml -> Theme.c) rather than fixed here:
// the active values come from whichever mode is selected — dynamic wallpaper-derived (the default), curated
// shrek-dark / shrek-light, high-contrast, or user-custom — plus any user overrides. The role NAMES are the
// contract and stay stable; only their values change per mode. Surfaces must reference these role names
// ONLY (never a raw colour or a palette source), enforced by scripts/check-tokens.sh, so a mode switch
// repaints the whole shell through this one chokepoint. Type/spacing/rounding/timing remain fixed identity.
QtObject {
    // ── colour roles (resolved by Theme.c; see themes/Theme.qml + themes/Palettes.js) ───────────────
    readonly property color bg:           Theme.c.bg           // base / root
    readonly property color surface:      Theme.c.surface      // bar, cards
    readonly property color surfaceAlt:   Theme.c.surfaceAlt   // raised row / hover
    readonly property color overlay:      Theme.c.overlay      // drawers / panels floating over content
    readonly property color border:       Theme.c.border
    readonly property color borderStrong: Theme.c.borderStrong

    // Floating surfaces over the wallpaper — may be translucent for depth (#AARRGGBB).
    readonly property color barBg:   Theme.c.barBg     // floating bar
    readonly property color panelBg: Theme.c.panelBg   // launcher / drawers
    readonly property color rowHi:   Theme.c.rowHi     // selected launcher row (accent-tinted)

    readonly property color text:      Theme.c.text        // primary
    readonly property color textDim:   Theme.c.textDim     // secondary
    readonly property color textFaint: Theme.c.textFaint   // tertiary / disabled

    readonly property color accent:     Theme.c.accent       // swamp green — Shrek identity
    readonly property color accentDim:  Theme.c.accentDim
    readonly property color accentText: Theme.c.accentText   // text/icon drawn ON accent fills

    // Semantic — used sparingly. ATTENTION is amber (notice), not red (alarm); danger only for
    // genuinely destructive affordances (power off).
    readonly property color notice: Theme.c.notice
    readonly property color danger: Theme.c.danger
    readonly property color ok:     Theme.c.ok

    // Danger-row hover tint + modal scrim (previously hard-coded in surfaces; now first-class roles).
    readonly property color dangerHover: Theme.c.dangerHover
    readonly property color scrim:       Theme.c.scrim

    // ── typography ────────────────────────────────────────────────────────────────────────────────
    readonly property string fontFamily: "DejaVu Sans"       // shipped (fonts-dejavu-core)
    readonly property string fontMono:   "DejaVu Sans Mono"
    readonly property int fontCaption:  11
    readonly property int fontSmall:    12
    readonly property int fontBody:     13
    readonly property int fontTitle:    15
    readonly property int fontHeadline: 18
    readonly property int fontDisplay:  24

    // ── spacing scale ─────────────────────────────────────────────────────────────────────────────
    readonly property int spaceXs: 4
    readonly property int spaceSm: 8
    readonly property int spaceMd: 12
    readonly property int spaceLg: 16
    readonly property int spaceXl: 24

    // ── rounding ──────────────────────────────────────────────────────────────────────────────────
    readonly property int radiusSm:   4
    readonly property int radius:     8
    readonly property int radiusLg:   12
    readonly property int radiusFull: 999

    // ── surface sizing ────────────────────────────────────────────────────────────────────────────
    readonly property int barHeight:   34
    readonly property int drawerWidth: 340

    // ── animation timing (ms) ─────────────────────────────────────────────────────────────────────
    // One calm motion vocabulary; surfaces pick a duration via components/Anim.qml.
    readonly property int animFast: 120
    readonly property int animMed:  200
    readonly property int animSlow: 320

    // ── back-compat aliases (Bootstrap-0 surfaces used these; kept until those files are replaced) ──
    readonly property int gap: 8
}
