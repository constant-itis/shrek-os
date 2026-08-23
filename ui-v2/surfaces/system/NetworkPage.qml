import QtQuick
import "../../components"
import "../../theme"
import "../../services"

Flickable {
    contentWidth: width
    contentHeight: body.implicitHeight
    clip: true
    boundsBehavior: Flickable.StopAtBounds

    Column {
        id: body
        width: parent.width
        spacing: Tokens.spaceLg

        ShrekSection {
            title: "Network"
            detail: Network.available ? (Network.online ? "Connected: " + (Network.activeConnection || Network.connectivity) : "Disconnected") : "NetworkManager is not available."

            ShrekSettingRow {
                title: Network.wifiEnabled ? "Wi-Fi enabled" : "Wi-Fi off"
                detail: Network.wifiHardware ? "Host Wi-Fi adapter" : "No Wi-Fi hardware"
                enabledRow: false

                ShrekToggle {
                    checked: Network.wifiEnabled
                    available: Network.available && Network.wifiHardware
                    onToggled: Network.toggleWifi()
                }
            }

            Row {
                width: parent.width
                spacing: Tokens.spaceSm

                ShrekButton {
                    text: "Rescan"
                    compact: true
                    onActivated: Network.reload()
                }

                ShrekButton {
                    text: "Disconnect"
                    compact: true
                    enabled: Network.activeDevice.length > 0
                    onActivated: Network.disconnect()
                }
            }
        }

        ShrekCard {
            visible: Network.pendingSsid.length > 0
            height: visible ? implicitHeight : 0

            Text {
                width: parent.width
                text: "Password for " + Network.pendingSsid
                color: Tokens.textPrimary
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontBody
                font.weight: Font.DemiBold
                elide: Text.ElideRight
            }

            Row {
                width: parent.width
                spacing: Tokens.spaceSm

                Rectangle {
                    width: parent.width - connectBtn.width - cancelBtn.width - Tokens.spaceSm * 2
                    height: Tokens.controlHeight
                    radius: Tokens.radiusSm
                    color: Tokens.surfaceRaised
                    border.width: 1
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
                        onAccepted: {
                            Network.connectWifi(Network.pendingSsid, text)
                            text = ""
                        }
                    }
                }

                ShrekButton {
                    id: connectBtn
                    text: "Connect"
                    kind: "primary"
                    onActivated: {
                        Network.connectWifi(Network.pendingSsid, pass.text)
                        pass.text = ""
                    }
                }

                ShrekButton {
                    id: cancelBtn
                    text: "Cancel"
                    onActivated: Network.clearPending()
                }
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

        ShrekSection {
            title: "Wi-Fi Networks"

            Repeater {
                model: Network.networks

                ShrekSettingRow {
                    required property var modelData
                    title: modelData.ssid
                    detail: modelData.signal + "%  " + (modelData.saved ? "saved  " : "") + (modelData.secured ? modelData.security : "open")
                    active: modelData.active
                    onActivated: Network.requestConnect(modelData)
                }
            }
        }
    }
}
