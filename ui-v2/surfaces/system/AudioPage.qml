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
            title: "Audio"
            detail: Audio.ready ? Audio.label : "No PipeWire output available."

            ShrekSettingRow {
                title: Audio.muted ? "Output muted" : "Output " + Math.round(Audio.volume * 100) + "%"
                detail: Audio.ready ? Audio.label : "PipeWire default sink is unavailable"
                enabledRow: false

                ShrekButton {
                    text: Audio.muted ? "Unmute" : "Mute"
                    kind: Audio.muted ? "primary" : "default"
                    compact: true
                    enabled: Audio.ready
                    onActivated: Audio.toggleMute()
                }
            }

            ShrekSlider {
                width: parent.width
                value: Audio.volume
                muted: Audio.muted
                enabled: Audio.ready
                onMoved: value => Audio.setVolume(value)
            }
        }

        ShrekSection {
            title: "Output Device"

            Repeater {
                model: Audio.outputs

                ShrekSettingRow {
                    required property var modelData
                    title: modelData.label
                    active: modelData.active
                    onActivated: Audio.selectOutput(modelData.id)
                }
            }
        }

        ShrekSection {
            title: "Input"
            detail: Audio.inputReady ? (Audio.inputMuted ? "Muted" : Math.round(Audio.inputVolume * 100) + "%") + " - " + Audio.inputLabel : "No input device"

            ShrekSlider {
                width: parent.width
                value: Audio.inputVolume
                muted: Audio.inputMuted
                enabled: Audio.inputReady
                onMoved: value => Audio.setInputVolume(value)
            }
        }
    }
}
