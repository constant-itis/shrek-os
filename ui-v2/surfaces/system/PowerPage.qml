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
            title: "Power"
            detail: Power.present ? (Math.round(Power.percentage) + "% - " + Power.estimate) : "AC power. No laptop battery reported by UPower."

            ShrekSettingRow {
                title: Power.present ? Math.round(Power.percentage) + "% " + Power.state : "Desktop power"
                detail: Power.onBattery ? "Battery" : "AC"
                active: Power.onBattery
                enabledRow: false
            }
        }

        ShrekSection {
            visible: Power.profilesAvailable
            title: "Power Profile"

            Repeater {
                model: Power.profiles

                ShrekSettingRow {
                    required property string modelData
                    visible: Power.profilesAvailable
                    title: modelData
                    active: Power.profile === modelData
                    onActivated: Power.setProfile(modelData)
                }
            }
        }
    }
}
