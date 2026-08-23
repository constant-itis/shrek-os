import QtQuick
import "../../components"
import "../../state"
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
            title: "Overview"
            detail: "Normal desktop controls. Agent authority stays read-only in Work."

            Repeater {
                model: [
                    { section: "network", title: "Network", detail: Network.online ? (Network.activeConnection || Network.connectivity) : (Network.available ? "Disconnected" : "NetworkManager unavailable"), active: Network.online },
                    { section: "audio", title: "Audio", detail: Audio.ready ? (Audio.muted ? "Muted" : Math.round(Audio.volume * 100) + "%") + " - " + Audio.label : "No PipeWire output", active: Audio.ready && !Audio.muted },
                    { section: "bluetooth", title: "Bluetooth", detail: Bluetooth.available ? (Bluetooth.enabled ? (Bluetooth.connectedCount + " connected") : "Off") : "No adapter", active: Bluetooth.available && Bluetooth.enabled },
                    { section: "power", title: "Power", detail: Power.present ? (Math.round(Power.percentage) + "% - " + Power.estimate) : "AC power", active: Power.onBattery },
                    { section: "appearance", title: "Appearance", detail: Appearance.mode, active: false },
                    { section: "system", title: "Displays/System", detail: CompositorService.windowCount + " windows on workspace " + (CompositorService.activeWorkspace ? CompositorService.activeWorkspace.name : "1"), active: false }
                ]

                ShrekSettingRow {
                    required property var modelData
                    title: modelData.title
                    detail: modelData.detail
                    active: modelData.active
                    onActivated: UI.openSystem(modelData.section)
                }
            }
        }
    }
}
