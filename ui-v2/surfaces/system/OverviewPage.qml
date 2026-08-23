import QtQuick
import "../../state"
import "../../theme"
import "../../services"

Flickable {
    contentWidth: width
    contentHeight: body.implicitHeight
    clip: true

    Column {
        id: body
        width: parent.width
        spacing: Tokens.spaceMd

        Text {
            width: parent.width
            text: "Overview"
            color: Tokens.textPrimary
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontHeadline
            font.bold: true
        }
        Text {
            width: parent.width
            text: "Normal desktop controls. Agent authority stays read-only in Work."
            color: Tokens.textSecondary
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontCaption
            wrapMode: Text.WordWrap
        }

        Repeater {
            model: [
                { section: "network", title: "Network", detail: Network.online ? (Network.activeConnection || Network.connectivity) : (Network.available ? "Disconnected" : "NetworkManager unavailable"), active: Network.online },
                { section: "audio", title: "Audio", detail: Audio.ready ? (Audio.muted ? "Muted" : Math.round(Audio.volume * 100) + "%") + " - " + Audio.label : "No PipeWire output", active: Audio.ready && !Audio.muted },
                { section: "bluetooth", title: "Bluetooth", detail: Bluetooth.available ? (Bluetooth.enabled ? (Bluetooth.connectedCount + " connected") : "Off") : "No adapter", active: Bluetooth.available && Bluetooth.enabled },
                { section: "power", title: "Power", detail: Power.present ? (Math.round(Power.percentage) + "% - " + Power.estimate) : "AC power", active: Power.onBattery },
                { section: "appearance", title: "Appearance", detail: Appearance.mode, active: false },
                { section: "system", title: "Displays/System", detail: CompositorService.windowCount + " windows on workspace " + (CompositorService.activeWorkspace ? CompositorService.activeWorkspace.name : "1"), active: false }
            ]
            Rectangle {
                required property var modelData
                width: body.width
                height: 54
                radius: Tokens.radius
                color: hover.containsMouse ? Tokens.surfaceRaised : Tokens.surface
                border.color: modelData.active ? Tokens.accent : Tokens.outline
                Column {
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    anchors.leftMargin: Tokens.spaceMd
                    anchors.rightMargin: Tokens.spaceMd
                    spacing: 2
                    Text {
                        width: parent.width
                        text: modelData.title
                        color: Tokens.textPrimary
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontBody
                        font.bold: true
                    }
                    Text {
                        width: parent.width
                        text: modelData.detail
                        color: Tokens.textSecondary
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontCaption
                        elide: Text.ElideRight
                    }
                }
                MouseArea {
                    id: hover
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: UI.openSystem(modelData.section)
                }
            }
        }
    }
}
