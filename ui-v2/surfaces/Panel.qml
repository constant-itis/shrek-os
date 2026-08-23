import Quickshell
import Quickshell.Wayland
import QtQuick
import "../config"
import "../state"
import "../theme"
import "system"

// Panel — the Shrek System center. Left edge attached, ordinary desktop state only.
PanelWindow {
    anchors { left: true; top: true; bottom: true }
    implicitWidth: Config.railWidth + Config.gap + Config.systemWidth
    exclusionMode: ExclusionMode.Ignore
    color: "transparent"
    mask: Region { item: card }

    Component.onCompleted: if (this.WlrLayershell != null) this.WlrLayershell.layer = WlrLayer.Top

    Rectangle {
        id: card
        x: Config.railWidth + Config.gap
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        anchors.topMargin: Config.gap
        anchors.bottomMargin: Config.gap
        width: UI.panelOpen ? Config.systemWidth : 0
        opacity: UI.panelOpen ? 1 : 0
        clip: true
        radius: Config.frameRadius
        color: Tokens.panelBg
        border.width: 1
        border.color: Tokens.outline

        Behavior on width   { NumberAnimation { duration: Config.animMs; easing.type: Easing.OutCubic } }
        Behavior on opacity { NumberAnimation { duration: Config.animMs } }

        SystemCenter {
            anchors.fill: parent
            anchors.margins: Tokens.spaceLg
            visible: UI.panelOpen
        }
    }
}
