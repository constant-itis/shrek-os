import Quickshell
import Quickshell.Wayland
import QtQuick
import "../../themes"
import "../../state"
import "../../services"
import "../../geometry"

// RailPopout — contextual rail hover surface. This is the QML bridge for the Caelestia-style bar popout
// route: rail items set a semantic popout name and anchor, while this edge-attached surface renders the
// content. It is display-only; clicks go to the owning rail item or drawer.
PanelWindow {
    id: pop
    WlrLayershell.layer: WlrLayer.Overlay
    anchors { top: true; bottom: true; left: true; right: true }
    exclusiveZone: 0
    color: "transparent"
    visible: ShellState.railPopoutOpen
    mask: Region { item: panel }

    property var provider
    readonly property string name: ShellState.railPopoutName
    readonly property int sessionCount: provider ? provider.sessions.length : 0
    readonly property real targetY: Math.max(Tokens.spaceSm, Math.min(ShellState.railPopoutY, height - panel.height - Tokens.spaceSm))

    Item {
        id: panel
        x: Tokens.railWidth + 2 * Tokens.spaceSm
        y: pop.targetY
        width: 236
        height: body.implicitHeight + 2 * Tokens.spaceMd
        opacity: ShellState.railPopoutOpen ? 1 : 0
        transform: Translate { x: ShellState.railPopoutOpen ? 0 : -12 }

        Behavior on y { NumberAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }
        Behavior on opacity { NumberAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }

        EdgePanelShape {
            anchors.fill: parent
            edge: "left"
        }

        Column {
            id: body
            anchors { left: parent.left; right: parent.right; top: parent.top; margins: Tokens.spaceMd }
            spacing: Tokens.spaceXs

            Text {
                width: parent.width
                text: pop.name === "system" ? "System"
                    : pop.name === "work" ? "Work"
                    : pop.name === "clock" ? Qt.formatDateTime(new Date(), "dddd, MMM d")
                    : pop.name === "terminal" ? "Terminal"
                    : "Applications"
                color: Tokens.text
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontBody
                font.bold: true
                elide: Text.ElideRight
            }

            Text {
                width: parent.width
                text: pop.name === "system"
                    ? (Network.online ? "Online on " + Network.iface : "Offline")
                    : pop.name === "work"
                      ? (pop.sessionCount + " isolated session" + (pop.sessionCount === 1 ? "" : "s"))
                    : pop.name === "clock"
                      ? Qt.formatDateTime(new Date(), "HH:mm")
                    : pop.name === "terminal"
                      ? "Open a user terminal"
                    : "Open launcher or right-click for menu"
                color: Tokens.textDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
                elide: Text.ElideRight
            }

            Text {
                width: parent.width
                visible: pop.name === "system"
                text: Audio.ready ? (Audio.muted ? "Audio muted" : "Volume " + Math.round(Audio.volume * 100) + "%") : "Audio unavailable"
                color: Tokens.textFaint
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontCaption
                elide: Text.ElideRight
            }
        }
    }
}
