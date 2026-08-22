import Quickshell
import Quickshell.Wayland
import QtQuick
import "../../themes"
import "../../state"

// Launcher — Slice-1 shows a real, toggleable surface (Super+D via IPC) but the app list/search lands
// in the next build phase. Hidden until ShellState.launcherOpen. Click-out closes; a proper click
// catcher + keyboard search field arrive with the real launcher.
PanelWindow {
    id: launcher
    WlrLayershell.layer: WlrLayer.Overlay
    WlrLayershell.keyboardFocus: WlrKeyboardFocus.OnDemand
    visible: ShellState.launcherOpen
    anchors { top: true; bottom: true; left: true; right: true }
    exclusiveZone: 0
    color: "#66000000"   // dim scrim

    MouseArea { anchors.fill: parent; onClicked: ShellState.closeAll() }

    Rectangle {
        anchors.centerIn: parent
        width: 440; height: 128
        radius: Tokens.radius
        color: Tokens.overlay
        border.color: Tokens.border

        Column {
            anchors.centerIn: parent
            spacing: Tokens.spaceSm
            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                text: "Launcher"
                color: Tokens.text
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontTitle
                font.bold: true
            }
            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                text: "App search and launch arrive in the next build"
                color: Tokens.textDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
            }
        }
    }
}
