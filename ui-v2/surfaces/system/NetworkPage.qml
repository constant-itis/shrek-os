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

        Text {
            width: parent.width
            text: "Network"
            color: Tokens.textPrimary
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontHeadline
            font.bold: true
        }
        Text {
            width: parent.width
            text: Network.available ? (Network.online ? "Connected: " + (Network.activeConnection || Network.connectivity) : "Disconnected") : "NetworkManager is not available."
            color: Tokens.textSecondary
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontCaption
            wrapMode: Text.WordWrap
        }

        Row {
            width: parent.width
            spacing: Tokens.spaceSm
            Text {
                width: parent.width - wifiToggle.width - Tokens.spaceSm
                anchors.verticalCenter: parent.verticalCenter
                text: Network.wifiEnabled ? "Wi-Fi enabled" : "Wi-Fi off"
                color: Tokens.textPrimary
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontBody
            }
            Rectangle {
                id: wifiToggle
                width: 48; height: 26; radius: Tokens.radiusFull
                color: Network.wifiEnabled ? Tokens.accent : Tokens.surfaceRaised
                border.color: Tokens.outline
                opacity: Network.available && Network.wifiHardware ? 1 : 0.45
                Rectangle {
                    width: 20; height: 20; radius: Tokens.radiusFull
                    anchors.verticalCenter: parent.verticalCenter
                    x: Network.wifiEnabled ? parent.width - width - 3 : 3
                    color: Network.wifiEnabled ? Tokens.textPrimary : Tokens.textSecondary
                    Behavior on x { NumberAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }
                }
                MouseArea { anchors.fill: parent; enabled: Network.available && Network.wifiHardware; cursorShape: Qt.PointingHandCursor; onClicked: Network.toggleWifi() }
            }
        }

        Rectangle {
            width: parent.width
            height: pending.visible ? 92 : 0
            visible: Network.pendingSsid.length > 0
            radius: Tokens.radius
            color: Tokens.surface
            border.color: Tokens.outline
            clip: true
            Column {
                id: pending
                anchors.fill: parent
                anchors.margins: Tokens.spaceMd
                spacing: Tokens.spaceSm
                Text {
                    width: parent.width
                    text: "Password for " + Network.pendingSsid
                    color: Tokens.textPrimary
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                    font.bold: true
                    elide: Text.ElideRight
                }
                Row {
                    width: parent.width
                    spacing: Tokens.spaceSm
                    Rectangle {
                        width: parent.width - connectBtn.width - cancelBtn.width - 2 * Tokens.spaceSm
                        height: 30
                        radius: Tokens.radiusSm
                        color: Tokens.surfaceRaised
                        border.color: pass.activeFocus ? Tokens.accent : Tokens.outline
                        TextInput {
                            id: pass
                            anchors.fill: parent
                            anchors.leftMargin: Tokens.spaceSm
                            anchors.rightMargin: Tokens.spaceSm
                            verticalAlignment: TextInput.AlignVCenter
                            echoMode: TextInput.PasswordEchoOnEdit
                            color: Tokens.textPrimary
                            selectionColor: Tokens.accentDim
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontBody
                            clip: true
                            onAccepted: { Network.connectWifi(Network.pendingSsid, text); text = "" }
                        }
                    }
                    Rectangle {
                        id: connectBtn
                        width: 72; height: 30; radius: Tokens.radiusSm
                        color: Tokens.accent
                        Text { anchors.centerIn: parent; text: "Connect"; color: Tokens.accentText; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontSmall; font.bold: true }
                        MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: { Network.connectWifi(Network.pendingSsid, pass.text); pass.text = "" } }
                    }
                    Rectangle {
                        id: cancelBtn
                        width: 54; height: 30; radius: Tokens.radiusSm
                        color: Tokens.surfaceRaised
                        border.color: Tokens.outline
                        Text { anchors.centerIn: parent; text: "Cancel"; color: Tokens.textSecondary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontSmall }
                        MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: Network.clearPending() }
                    }
                }
            }
        }

        Row {
            width: parent.width
            spacing: Tokens.spaceSm
            Rectangle {
                width: 96; height: 30; radius: Tokens.radiusSm
                color: Tokens.surfaceRaised
                border.color: Tokens.outline
                Text { anchors.centerIn: parent; text: "Rescan"; color: Tokens.textPrimary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontSmall }
                MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: Network.reload() }
            }
            Rectangle {
                width: 108; height: 30; radius: Tokens.radiusSm
                color: Tokens.surfaceRaised
                border.color: Tokens.outline
                opacity: Network.activeDevice.length > 0 ? 1 : 0.45
                Text { anchors.centerIn: parent; text: "Disconnect"; color: Tokens.textPrimary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontSmall }
                MouseArea { anchors.fill: parent; enabled: Network.activeDevice.length > 0; cursorShape: Qt.PointingHandCursor; onClicked: Network.disconnect() }
            }
        }

        Text {
            width: parent.width
            visible: Network.lastError.length > 0
            text: Network.lastError
            color: Tokens.warning
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontCaption
            wrapMode: Text.WordWrap
        }

        Repeater {
            model: Network.networks
            Rectangle {
                required property var modelData
                width: body.width
                height: 50
                radius: Tokens.radius
                color: modelData.active ? Tokens.accentDim : (hover.containsMouse ? Tokens.surfaceRaised : Tokens.surface)
                border.color: modelData.active ? Tokens.accent : Tokens.outline
                Column {
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    anchors.leftMargin: Tokens.spaceMd
                    anchors.rightMargin: Tokens.spaceMd
                    spacing: 2
                    Text { width: parent.width; text: modelData.ssid; color: Tokens.textPrimary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontBody; font.bold: true; elide: Text.ElideRight }
                    Text { width: parent.width; text: modelData.signal + "%  " + (modelData.saved ? "saved  " : "") + (modelData.secured ? modelData.security : "open"); color: modelData.active ? Tokens.textPrimary : Tokens.textSecondary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontCaption; elide: Text.ElideRight }
                }
                MouseArea { id: hover; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: Network.requestConnect(modelData) }
            }
        }
    }
}
