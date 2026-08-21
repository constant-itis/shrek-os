pragma Singleton
import QtQuick

// Bootstrap-0 design tokens. Intentionally minimal — NO theming system, NO polish.
// Just enough shared constants so the surfaces are not hard-coding colours inline.
QtObject {
    readonly property color bg:      "#1a1a1a"
    readonly property color surface: "#242424"
    readonly property color border:  "#3a3a3a"
    readonly property color text:    "#e0e0e0"
    readonly property color textDim: "#8a8a8a"
    readonly property color accent:  "#5aa02c"   // muted swamp green

    readonly property int barHeight:   32
    readonly property int gap:         8
    readonly property int radius:      6
    readonly property int drawerWidth: 320
}
