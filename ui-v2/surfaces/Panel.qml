import Quickshell
import Quickshell.Wayland
import QtQuick
import "../config"
import "../state"
import "../theme"

// Panel — one per screen. An edge-attached popout that slides out just right of the rail. A full-height
// transparent layer surface (ExclusionMode.Ignore => overlaps windows without displacing them) whose
// card animates width + opacity on UI.panelOpen. The Region mask tracks the card's live geometry, so
// clicks pass through everywhere the card isn't — and when closed (width 0) the whole surface is
// click-through. This is the input-pass-through + animated open/close proof.
PanelWindow {
    anchors { left: true; top: true; bottom: true }
    implicitWidth: Config.railWidth + Config.gap + Config.panelWidth
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
        width: UI.panelOpen ? Config.panelWidth : 0
        opacity: UI.panelOpen ? 1 : 0
        clip: true
        radius: Config.frameRadius
        color: Tokens.panelBg
        border.width: 1
        border.color: Tokens.outline

        Behavior on width   { NumberAnimation { duration: Config.animMs; easing.type: Easing.OutCubic } }
        Behavior on opacity { NumberAnimation { duration: Config.animMs } }

        Column {
            anchors.fill: parent
            anchors.margins: 16
            spacing: 12
            visible: UI.panelOpen
            Text {
                text: "Panel"
                font.pixelSize: 20; font.bold: true
                color: Tokens.textPrimary
            }
            Text {
                width: parent.width
                wrapMode: Text.WordWrap
                font.pixelSize: 13
                color: Tokens.textSecondary
                text: "Edge-attached popout. Width and opacity animate on open/close. "
                    + "Clicks pass through everywhere outside this card — the Region mask tracks it live."
            }
        }
    }
}
