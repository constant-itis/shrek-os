import Quickshell
import Quickshell.Wayland
import QtQuick
import "../themes"

// Launcher.qml — Bootstrap-0 PLACEHOLDER ONLY. No search, no app list, no launching.
// A transparent, non-exclusive overlay that just proves the surface can host and
// centre content. Later slices replace the body with a real launcher.
PanelWindow {
    id: launcher
    anchors { top: true; bottom: true; left: true; right: true }
    color: "transparent"
    exclusiveZone: 0

    Text {
        anchors.centerIn: parent
        text: "Launcher placeholder"
        color: Tokens.textDim
        font.pixelSize: 16
    }
}
