pragma Singleton
import QtQuick

// Tokens — THE Shrek-owned semantic design contract for shell-v2. Every surface expresses colour ONLY
// through these role names (never a raw hex, never a palette source), enforced by scripts/check-tokens.sh,
// so one mode switch repaints the whole shell through this single chokepoint.
//
// Colour roles resolve through the ported theme system (theme/Theme.qml -> Theme.c, theme/Palettes.js).
// The active values come from the selected MODE while the role NAMES stay fixed:
//   dynamic (REQUESTED) -> falls through to shrek-dark (the EFFECTIVE floor) until a wallpaper scheme is
//     written (plumbing only — no matugen, no extraction shipped this slice),
//   shrek-dark / shrek-light / high-contrast (curated), or a user CUSTOM contract (~/.config/shrek/custom.json).
// Because dynamic has no scheme source yet, shrek-dark is the effective default everywhere.
//
// Type / spacing / rounding / motion are fixed identity (not themed).
QtObject {
    // ── surfaces ──────────────────────────────────────────────────────────────────────────────────
    readonly property color background:    Theme.c.bg           // root / desktop base
    readonly property color surface:       Theme.c.surface      // cards, rail, bar
    readonly property color surfaceRaised: Theme.c.surfaceAlt   // raised row / hover
    readonly property color overlay:       Theme.c.overlay      // drawers floating over content
    readonly property color panelBg:       Theme.c.panelBg      // floating drawer fill (may be translucent)
    readonly property color scrim:         Theme.c.scrim        // modal dim

    // ── text ──────────────────────────────────────────────────────────────────────────────────────
    readonly property color textPrimary:   Theme.c.text
    readonly property color textSecondary: Theme.c.textDim
    readonly property color muted:         Theme.c.textFaint

    // ── accent (Shrek identity) ─────────────────────────────────────────────────────────────────────
    readonly property color accent:        Theme.c.accent
    readonly property color accentDim:     Theme.c.accentDim
    readonly property color accentText:    Theme.c.accentText   // text/icon drawn ON an accent fill

    // ── structure ───────────────────────────────────────────────────────────────────────────────────
    readonly property color outline:       Theme.c.border
    readonly property color outlineStrong: Theme.c.borderStrong

    // ── status ──────────────────────────────────────────────────────────────────────────────────────
    readonly property color success:       Theme.c.ok
    readonly property color warning:       Theme.c.notice
    readonly property color danger:        Theme.c.danger

    // ── Work / Attention roles (as already defined): Work == Shrek accent identity; Attention == amber
    //    notice (a NOTICE, not an alarm). These name the hero surface's authority-viz colours directly. ──
    readonly property color work:          Theme.c.accent
    readonly property color attention:     Theme.c.notice

    // ── typography (fixed identity) ─────────────────────────────────────────────────────────────────
    readonly property string fontFamily: "DejaVu Sans"        // shipped (fonts-dejavu-core)
    readonly property string fontMono:   "DejaVu Sans Mono"
    readonly property int fontCaption:  11
    readonly property int fontSmall:    12
    readonly property int fontBody:     13
    readonly property int fontTitle:    15
    readonly property int fontHeadline: 18
    readonly property int fontDisplay:  24

    // ── spacing (fixed identity) ────────────────────────────────────────────────────────────────────
    readonly property int spaceXs: 4
    readonly property int spaceSm: 8
    readonly property int spaceMd: 12
    readonly property int spaceLg: 16
    readonly property int spaceXl: 24

    // ── component metrics (fixed identity) ──────────────────────────────────────────────────────────
    readonly property int controlHeightSm: 30
    readonly property int controlHeight:   36
    readonly property int iconButtonSize:  36
    readonly property int rowHeight:       52
    readonly property int panelPadding:    16
    readonly property int toggleWidth:     48
    readonly property int toggleHeight:    26
    readonly property int sliderHeight:    28
    readonly property int sliderTrack:      8

    // ── rounding (fixed identity) ───────────────────────────────────────────────────────────────────
    readonly property int radiusSm:   4
    readonly property int radius:     8
    readonly property int radiusLg:   12
    readonly property int radiusFull: 999

    // ── motion, ms (fixed identity) ─────────────────────────────────────────────────────────────────
    readonly property int animFast: 120
    readonly property int animMed:  200
    readonly property int animSlow: 320
}
