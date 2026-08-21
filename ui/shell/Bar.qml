import Quickshell
import Quickshell.Wayland
import QtQuick
import "../themes"

// Bar.qml — the top bar. Bootstrap-0: workspace indicators (left) + system status
// (right), both static placeholders. No real workspace tracking, no clock, no tray.
PanelWindow {
    id: bar
    anchors { top: true; left: true; right: true }
    implicitHeight: Tokens.barHeight
    color: Tokens.bg

    // left: workspace indicators (placeholder — first one "active")
    Row {
        anchors { left: parent.left; verticalCenter: parent.verticalCenter; leftMargin: Tokens.gap }
        spacing: Tokens.gap
        Repeater {
            model: 3
            Rectangle {
                width: 18; height: 18; radius: Tokens.radius
                color: index === 0 ? Tokens.accent : Tokens.surface
                border.color: Tokens.border
            }
        }
    }

    // right: system status (placeholder)
    Text {
        anchors { right: parent.right; verticalCenter: parent.verticalCenter; rightMargin: Tokens.gap }
        text: "shrek"
        color: Tokens.textDim
        font.pixelSize: 13
    }
}
