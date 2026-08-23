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
            title: "Bluetooth"
            detail: Bluetooth.available ? (Bluetooth.enabled ? "Adapter enabled" : "Adapter off") : "No Bluetooth adapter detected."

            ShrekSettingRow {
                title: Bluetooth.enabled ? "Bluetooth on" : "Bluetooth off"
                detail: Bluetooth.available ? "Host adapter state" : "Adapter unavailable"
                enabledRow: false

                ShrekToggle {
                    checked: Bluetooth.enabled
                    available: Bluetooth.available
                    onToggled: Bluetooth.toggle()
                }
            }
        }

        ShrekSection {
            title: "Known Devices"

            Repeater {
                model: Bluetooth.devices

                ShrekSettingRow {
                    required property var modelData
                    title: Bluetooth.label(modelData)
                    detail: Bluetooth.connected(modelData) ? "connected" : "known"
                    active: Bluetooth.connected(modelData)

                    ShrekButton {
                        text: Bluetooth.connected(modelData) ? "Disconnect" : "Connect"
                        compact: true
                        onActivated: Bluetooth.connected(modelData) ? Bluetooth.disconnectDevice(modelData) : Bluetooth.connectDevice(modelData)
                    }
                }
            }
        }
    }
}
