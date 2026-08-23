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
            title: "Appearance"
            detail: "Semantic Shrek theme modes. Dynamic and Custom keep their contracts even when no provider is active."

            Repeater {
                model: Appearance.modes

                ShrekSettingRow {
                    required property var modelData
                    title: modelData.label
                    detail: modelData.detail
                    active: Appearance.mode === modelData.mode
                    onActivated: Appearance.setMode(modelData.mode)
                }
            }
        }
    }
}
