pragma Singleton
import QtQuick

// Tokens — the installer's semantic design contract. The live installer runs with NO user theme config
// (no ~/.config/shrek/theme.json, no wallpaper), so unlike the desktop shell it does not resolve a mode
// through Theme/Colours. It uses the curated shrek-dark floor as a FIXED contract — mirrored here verbatim
// from ui-v2/theme/palettes/shrek-dark.json, exactly as Palettes.js BOOTSTRAP mirrors it (kept redundant
// so the contract holds with zero I/O dependency). Installer surfaces express colour ONLY through these
// role names — never a raw hex — so a later swap to the shared theme is a one-file change.
QtObject {
    // ── surfaces (shrek-dark) ──────────────────────────────
    readonly property color background:    "#101014"
    readonly property color surface:       "#17181c"
    readonly property color surfaceRaised: "#1f2127"
    readonly property color overlay:       "#22242b"
    readonly property color footerBg:      "#0d0e11"

    // ── structure ──────────────────────────────────────────
    readonly property color outline:       "#2f333b"
    readonly property color outlineStrong: "#3d424c"
    readonly property color rowHighlight:  "#26311c"

    // ── text ───────────────────────────────────────────────
    readonly property color textPrimary:   "#e8e8e6"
    readonly property color textSecondary: "#9aa0a8"
    readonly property color muted:         "#6b7079"

    // ── accent (Shrek identity) ────────────────────────────
    readonly property color accent:        "#5aa02c"
    readonly property color accentDim:     "#47801f"
    readonly property color accentText:    "#0c1206"
    readonly property color accentGlow:    "#335aa02c"

    // ── status + tinted surfaces ───────────────────────────
    readonly property color notice:        "#d8a657"
    readonly property color noticeSurface: "#1d1810"
    readonly property color noticeOutline: "#4a3c1c"
    readonly property color danger:        "#e06c75"
    readonly property color dangerText:    "#1a0d0f"
    readonly property color dangerSurface: "#1a1113"
    readonly property color dangerOutline: "#52242a"
    readonly property color ok:            "#5aa02c"
    readonly property color sealSurface:   "#141b0f"
    readonly property color sealOutline:   "#2c3a1c"

    // ── typography (fixed identity) ────────────────────────
    readonly property string fontFamily: "DejaVu Sans"
    readonly property string fontMono:   "DejaVu Sans Mono"
    readonly property int fontCaption:  11
    readonly property int fontSmall:    12
    readonly property int fontBody:     13
    readonly property int fontTitle:    15
    readonly property int fontHeadline: 18
    readonly property int fontDisplay:  24
    readonly property int fontHero:     31   // installer hero heading — owner-approved, not in the DMS scale

    // ── spacing (fixed identity) ───────────────────────────
    readonly property int spaceXs: 4
    readonly property int spaceSm: 8
    readonly property int spaceMd: 12
    readonly property int spaceLg: 16
    readonly property int spaceXl: 24
    readonly property int stagePad: 56

    // ── rounding ───────────────────────────────────────────
    readonly property int radiusSm:   4
    readonly property int radius:     8
    readonly property int radiusLg:   12
    readonly property int radiusFull: 999

    // ── component metrics ──────────────────────────────────
    readonly property int chromeHeight:  56
    readonly property int actionHeight:   72
    readonly property int controlHeight:  44
    readonly property int buttonHeight:   40

    // ── motion, ms ─────────────────────────────────────────
    readonly property int animFast: 120
    readonly property int animMed:  200
    readonly property int animSlow: 320
}
