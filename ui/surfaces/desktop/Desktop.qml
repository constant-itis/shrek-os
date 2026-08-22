import Quickshell
import Quickshell.Wayland
import QtQuick
import "../../themes"
import "../../state"

// Desktop — a background surface under all windows that gives the empty desktop normal affordances:
// right-click opens the launcher (the root menu people reach for), left-click dismisses open drawers.
// It sits on the Background layer, so real windows render above it and keep their own clicks; only
// clicks on bare desktop reach here.
PanelWindow {
    id: desktop
    WlrLayershell.layer: WlrLayer.Background
    anchors { top: true; bottom: true; left: true; right: true }
    exclusiveZone: 0
    color: "transparent"   // Sway paints the wallpaper/background colour; stay out of its way

    MouseArea {
        anchors.fill: parent
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        onClicked: (m) => {
            if (m.button === Qt.RightButton)
                ShellState.toggleLauncher()
            else
                ShellState.closeAll()
        }
    }
}
