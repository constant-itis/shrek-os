pragma Singleton
import QtQuick

// Palette — placeholder colours for FIRST ACCEPTANCE (swamp greens). Superseded by a port of the real
// 5-mode Colours/Palettes theme system in a later slice; raw hex is fine here because this IS the theme
// layer, not a surface consuming it.
QtObject {
    readonly property color bg:      "#0f1410"
    readonly property color surface: "#1b241c"
    readonly property color border:  "#3a4a3c"
    readonly property color accent:  "#7cc67a"
    readonly property color text:    "#e7f0e6"
    readonly property color textDim: "#9db29c"
}
