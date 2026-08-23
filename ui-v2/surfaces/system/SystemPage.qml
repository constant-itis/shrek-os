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
            title: "Displays/System"
            detail: "Current compositor state. Display arrangement controls are deferred until the mature Sway output backend is wrapped cleanly."

            ShrekSettingRow {
                title: "Workspace " + (CompositorService.activeWorkspace ? CompositorService.activeWorkspace.name : "1")
                detail: CompositorService.windowCount + " visible windows"
                enabledRow: false
            }
        }

        ShrekSection {
            title: "Workspaces"

            Repeater {
                model: CompositorService.workspaces

                ShrekSettingRow {
                    required property var modelData
                    title: "Workspace " + (modelData.number > 0 ? modelData.number : modelData.name)
                    detail: modelData.urgent ? "urgent" : ""
                    active: modelData.focused || modelData.active
                    onActivated: CompositorService.focusWorkspace(modelData.number)
                }
            }
        }
    }
}
