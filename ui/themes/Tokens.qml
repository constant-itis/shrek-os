pragma Singleton
import QtQuick

// Shrek design tokens (Desktop Slice 1). A fixed identity palette — calm, protective, organic/layered,
// developer-oriented. NOT wallpaper-derived (that is a Caelestia/MD3 approach we reject). One source of
// truth for colour roles, type scale, spacing, rounding, and animation timing so surfaces never
// hard-code values inline. Kept deliberately small; grow only when a surface actually needs a token.
QtObject {
    // ── colour roles ──────────────────────────────────────────────────────────────────────────────
    // Near-black base with a faint cool tint (organic, not flat #000). Raised planes step lighter.
    readonly property color bg:           "#101014"   // base / root
    readonly property color surface:      "#17181c"   // bar, cards
    readonly property color surfaceAlt:   "#1f2127"   // raised row / hover
    readonly property color overlay:      "#22242b"   // drawers / panels floating over content
    readonly property color border:       "#2f333b"
    readonly property color borderStrong: "#3d424c"

    // Floating surfaces over the wallpaper — slightly translucent for depth (#AARRGGBB).
    readonly property color barBg:   "#ec1a1b20"   // floating bar
    readonly property color panelBg: "#f21d1f26"   // launcher / drawers
    readonly property color rowHi:   "#26311c"     // selected launcher row (accent-tinted)

    readonly property color text:      "#e8e8e6"       // primary
    readonly property color textDim:   "#9aa0a8"       // secondary
    readonly property color textFaint: "#6b7079"       // tertiary / disabled

    readonly property color accent:     "#5aa02c"      // swamp green — Shrek identity
    readonly property color accentDim:  "#47801f"
    readonly property color accentText: "#0c1206"      // text/icon drawn ON accent fills

    // Semantic — used sparingly. ATTENTION is amber (notice), not red (alarm); danger only for
    // genuinely destructive affordances (power off).
    readonly property color notice: "#d8a657"
    readonly property color danger: "#e06c75"
    readonly property color ok:     "#5aa02c"

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
