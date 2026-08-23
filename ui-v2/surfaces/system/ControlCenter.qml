import QtQuick
import "../../components"
import "../../state"
import "../../theme"
import "../../services"

Item {
    id: root

    component CcTile: Rectangle {
        id: tile

        property string title: ""
        property string detail: ""
        property string mark: ""
        property bool active: false
        property bool available: true
        property bool wide: false
        signal activated()

        width: wide ? grid.width : Math.floor((grid.width - Tokens.spaceSm) / 2)
        height: 72
        radius: active ? Tokens.radius : Tokens.radiusLg
        color: active ? Tokens.accentDim : (hover.containsMouse ? Tokens.surfaceRaised : Tokens.surface)
        border.width: 1
        border.color: active ? Tokens.accent : (hover.containsMouse ? Tokens.outlineStrong : Tokens.outline)
        opacity: available ? 1 : 0.48

        Behavior on color { ColorAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }
        Behavior on radius { NumberAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }

        Rectangle {
            anchors.fill: parent
            radius: parent.radius
            color: active ? Tokens.accentText : Tokens.textPrimary
            opacity: hover.pressed ? 0.14 : (hover.containsMouse ? 0.07 : 0)
            visible: opacity > 0
        }

        Rectangle {
            anchors.left: parent.left
            anchors.leftMargin: Tokens.spaceMd
            anchors.verticalCenter: parent.verticalCenter
            width: 34
            height: 34
            radius: Tokens.radius
            color: active ? Tokens.accent : Tokens.surfaceRaised
            border.width: 1
            border.color: active ? Tokens.accent : Tokens.outline

            Text {
                anchors.centerIn: parent
                text: tile.mark
                color: active ? Tokens.accentText : Tokens.textPrimary
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
                font.weight: Font.DemiBold
                horizontalAlignment: Text.AlignHCenter
            }
        }

        Column {
            anchors.left: parent.left
            anchors.leftMargin: Tokens.spaceMd + 46
            anchors.right: parent.right
            anchors.rightMargin: Tokens.spaceMd
            anchors.verticalCenter: parent.verticalCenter
            spacing: 2

            Text {
                width: parent.width
                text: tile.title
                color: active ? Tokens.accentText : Tokens.textPrimary
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontBody
                font.weight: Font.DemiBold
                elide: Text.ElideRight
            }

            Text {
                width: parent.width
                text: tile.detail
                color: active ? Tokens.accentText : Tokens.textSecondary
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontCaption
                elide: Text.ElideRight
            }
        }

        MouseArea {
            id: hover
            anchors.fill: parent
            enabled: tile.available
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: tile.activated()
        }
    }

    component NetworkRow: Rectangle {
        id: row

        property var net

        width: parent ? parent.width : 360
        height: 44
        radius: Tokens.radius
        color: net && net.active ? Tokens.accentDim : (hover.containsMouse ? Tokens.surfaceRaised : "transparent")
        border.width: net && net.active ? 1 : 0
        border.color: Tokens.accent

        Text {
            anchors.left: parent.left
            anchors.leftMargin: Tokens.spaceMd
            anchors.right: parent.right
            anchors.rightMargin: Tokens.spaceMd
            anchors.top: parent.top
            anchors.topMargin: 6
            text: row.net ? row.net.ssid : ""
            color: row.net && row.net.active ? Tokens.accentText : Tokens.textPrimary
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontSmall
            font.weight: Font.DemiBold
            elide: Text.ElideRight
        }

        Text {
            anchors.left: parent.left
            anchors.leftMargin: Tokens.spaceMd
            anchors.right: parent.right
            anchors.rightMargin: Tokens.spaceMd
            anchors.bottom: parent.bottom
            anchors.bottomMargin: 6
            text: row.net ? row.net.signal + "%  " + (row.net.saved ? "saved  " : "") + (row.net.secured ? row.net.security : "open") : ""
            color: row.net && row.net.active ? Tokens.accentText : Tokens.textSecondary
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontCaption
            elide: Text.ElideRight
        }

        MouseArea {
            id: hover
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: if (row.net) Network.requestConnect(row.net)
        }
    }

    Column {
        anchors.fill: parent
        spacing: Tokens.spaceMd

        Rectangle {
            width: parent.width
            height: 72
            radius: Tokens.radius
            color: Tokens.surface
            border.width: 1
            border.color: Tokens.outline

            Column {
                anchors.left: parent.left
                anchors.leftMargin: Tokens.spaceLg
                anchors.verticalCenter: parent.verticalCenter
                spacing: 2

                Text {
                    text: "Control Center"
                    color: Tokens.textPrimary
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontHeadline
                    font.weight: Font.DemiBold
                }

                Text {
                    text: Network.online ? "Online via " + (Network.activeConnection || Network.connectivity) : "Desktop controls"
                    color: Tokens.textSecondary
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontCaption
                }
            }

            ShrekButton {
                anchors.right: parent.right
                anchors.rightMargin: Tokens.spaceLg
                anchors.verticalCenter: parent.verticalCenter
                text: "Settings"
                compact: true
                onActivated: UI.openSystem(UI.systemSection || "overview")
            }
        }

        Flickable {
            width: parent.width
            height: parent.height - y
            contentWidth: width
            contentHeight: content.implicitHeight
            clip: true
            boundsBehavior: Flickable.StopAtBounds

            Column {
                id: content
                width: parent.width
                spacing: Tokens.spaceMd

                Grid {
                    id: grid
                    width: parent.width
                    columns: 2
                    spacing: Tokens.spaceSm

                    CcTile {
                        title: Network.wifiEnabled ? "Wi-Fi" : "Wi-Fi Off"
                        detail: Network.online ? (Network.activeConnection || "Connected") : (Network.available ? "Disconnected" : "NetworkManager unavailable")
                        mark: "net"
                        active: Network.online
                        available: Network.available && Network.wifiHardware
                        onActivated: UI.controlSection = UI.controlSection === "network" ? "overview" : "network"
                    }

                    CcTile {
                        title: Audio.muted ? "Muted" : "Volume " + Math.round(Audio.volume * 100) + "%"
                        detail: Audio.ready ? Audio.label : "PipeWire unavailable"
                        mark: "vol"
                        active: Audio.ready && !Audio.muted
                        available: Audio.ready
                        onActivated: Audio.toggleMute()
                    }

                    CcTile {
                        title: Bluetooth.enabled ? "Bluetooth" : "Bluetooth Off"
                        detail: Bluetooth.available ? Bluetooth.connectedCount + " connected" : "No adapter"
                        mark: "bt"
                        active: Bluetooth.available && Bluetooth.enabled
                        available: Bluetooth.available
                        onActivated: Bluetooth.toggle()
                    }

                    CcTile {
                        title: Power.present ? Math.round(Power.percentage) + "% Battery" : "Power"
                        detail: Power.present ? Power.estimate : "AC power"
                        mark: "pwr"
                        active: Power.onBattery
                        onActivated: UI.openSystem("power")
                    }

                    CcTile {
                        title: "Brightness"
                        detail: "Backend not wired"
                        mark: "sun"
                        available: false
                    }

                    CcTile {
                        title: "Appearance"
                        detail: "Theme and contrast"
                        mark: "ui"
                        onActivated: UI.openSystem("appearance")
                    }
                }

                Rectangle {
                    width: parent.width
                    height: 76
                    radius: Tokens.radiusLg
                    color: Tokens.surface
                    border.width: 1
                    border.color: Tokens.outline

                    Column {
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.leftMargin: Tokens.spaceLg
                        anchors.rightMargin: Tokens.spaceLg
                        spacing: Tokens.spaceXs

                        Row {
                            width: parent.width

                            Text {
                                width: parent.width - muteButton.width
                                text: "Output"
                                color: Tokens.textPrimary
                                font.family: Tokens.fontFamily
                                font.pixelSize: Tokens.fontBody
                                font.weight: Font.DemiBold
                            }

                            ShrekButton {
                                id: muteButton
                                text: Audio.muted ? "Unmute" : "Mute"
                                compact: true
                                enabled: Audio.ready
                                onActivated: Audio.toggleMute()
                            }
                        }

                        ShrekSlider {
                            width: parent.width
                            value: Audio.volume
                            muted: Audio.muted
                            icon: "vol"
                            valueText: Math.round(Audio.volume * 100) + "%"
                            enabled: Audio.ready
                            onMoved: value => Audio.setVolume(value)
                        }
                    }
                }

                Rectangle {
                    visible: UI.controlSection === "network"
                    width: parent.width
                    height: visible ? Math.min(250, networkColumn.implicitHeight + Tokens.spaceLg * 2) : 0
                    radius: Tokens.radiusLg
                    color: Tokens.surface
                    border.width: 1
                    border.color: Tokens.outline
                    clip: true

                    Column {
                        id: networkColumn
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.top: parent.top
                        anchors.margins: Tokens.spaceLg
                        spacing: Tokens.spaceSm

                        Row {
                            width: parent.width
                            spacing: Tokens.spaceSm

                            Column {
                                width: parent.width - wifiToggle.width - Tokens.spaceSm
                                spacing: 2

                                Text {
                                    width: parent.width
                                    text: "Wi-Fi"
                                    color: Tokens.textPrimary
                                    font.family: Tokens.fontFamily
                                    font.pixelSize: Tokens.fontTitle
                                    font.weight: Font.DemiBold
                                }

                                Text {
                                    width: parent.width
                                    text: Network.online ? (Network.activeConnection || "Connected") : "Choose a network"
                                    color: Tokens.textSecondary
                                    font.family: Tokens.fontFamily
                                    font.pixelSize: Tokens.fontCaption
                                    elide: Text.ElideRight
                                }
                            }

                            ShrekToggle {
                                id: wifiToggle
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

                            ShrekButton {
                                text: "Network Settings"
                                compact: true
                                kind: "primary"
                                onActivated: UI.openSystem("network")
                            }
                        }

                        Repeater {
                            model: Network.networks

                            NetworkRow {
                                required property var modelData
                                net: modelData
                            }
                        }
                    }
                }

                Rectangle {
                    visible: Network.pendingSsid.length > 0
                    width: parent.width
                    height: visible ? 94 : 0
                    radius: Tokens.radiusLg
                    color: Tokens.surface
                    border.width: 1
                    border.color: Tokens.accent

                    Column {
                        anchors.fill: parent
                        anchors.margins: Tokens.spaceMd
                        spacing: Tokens.spaceSm

                        Text {
                            width: parent.width
                            text: "Password for " + Network.pendingSsid
                            color: Tokens.textPrimary
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                            font.weight: Font.DemiBold
                            elide: Text.ElideRight
                        }

                        Row {
                            width: parent.width
                            spacing: Tokens.spaceSm

                            Rectangle {
                                width: parent.width - connectBtn.width - cancelBtn.width - Tokens.spaceSm * 2
                                height: Tokens.controlHeight
                                radius: Tokens.radius
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
                }
            }
        }
    }
}
