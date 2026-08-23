import QtQuick
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

        Text { width: parent.width; text: "Bluetooth"; color: Tokens.textPrimary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontHeadline; font.bold: true }
        Text { width: parent.width; text: Bluetooth.available ? (Bluetooth.enabled ? "Adapter enabled" : "Adapter off") : "No Bluetooth adapter detected."; color: Tokens.textSecondary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontCaption; wrapMode: Text.WordWrap }

        Row {
            width: parent.width
            spacing: Tokens.spaceSm
            Text { width: parent.width - 56; anchors.verticalCenter: parent.verticalCenter; text: Bluetooth.enabled ? "Bluetooth on" : "Bluetooth off"; color: Tokens.textPrimary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontBody }
            Rectangle {
                width: 48; height: 26; radius: Tokens.radiusFull; color: Bluetooth.enabled ? Tokens.accent : Tokens.surfaceRaised; border.color: Tokens.outline; opacity: Bluetooth.available ? 1 : 0.45
                Rectangle { width: 20; height: 20; radius: Tokens.radiusFull; anchors.verticalCenter: parent.verticalCenter; x: Bluetooth.enabled ? parent.width - width - 3 : 3; color: Bluetooth.enabled ? Tokens.accentText : Tokens.textSecondary }
                MouseArea { anchors.fill: parent; enabled: Bluetooth.available; cursorShape: Qt.PointingHandCursor; onClicked: Bluetooth.toggle() }
            }
        }

        Text { width: parent.width; text: "Known devices"; color: Tokens.textSecondary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontCaption }
        Repeater {
            model: Bluetooth.devices
            Rectangle {
                required property var modelData
                width: body.width; height: 48; radius: Tokens.radius
                color: Bluetooth.connected(modelData) ? Tokens.accentDim : (hover.containsMouse ? Tokens.surfaceRaised : Tokens.surface)
                border.color: Bluetooth.connected(modelData) ? Tokens.accent : Tokens.outline
                Column {
                    anchors.left: parent.left; anchors.right: action.left; anchors.verticalCenter: parent.verticalCenter
                    anchors.leftMargin: Tokens.spaceMd; anchors.rightMargin: Tokens.spaceSm
                    Text { width: parent.width; text: Bluetooth.label(modelData); color: Tokens.textPrimary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontBody; font.bold: true; elide: Text.ElideRight }
                    Text { width: parent.width; text: Bluetooth.connected(modelData) ? "connected" : "known"; color: Bluetooth.connected(modelData) ? Tokens.textPrimary : Tokens.textSecondary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontCaption; elide: Text.ElideRight }
                }
                Rectangle {
                    id: action
                    anchors.right: parent.right; anchors.rightMargin: Tokens.spaceSm; anchors.verticalCenter: parent.verticalCenter
                    width: 86; height: 28; radius: Tokens.radiusSm; color: Tokens.surfaceRaised; border.color: Tokens.outline
                    Text { anchors.centerIn: parent; text: Bluetooth.connected(modelData) ? "Disconnect" : "Connect"; color: Tokens.textPrimary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontSmall }
                }
                MouseArea { id: hover; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: Bluetooth.connected(modelData) ? Bluetooth.disconnectDevice(modelData) : Bluetooth.connectDevice(modelData) }
            }
        }
    }
}
