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

        Text { width: parent.width; text: "Displays/System"; color: Tokens.textPrimary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontHeadline; font.bold: true }
        Text { width: parent.width; text: "Current compositor state. Display arrangement controls are deferred until the mature Sway output backend is wrapped cleanly."; color: Tokens.textSecondary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontCaption; wrapMode: Text.WordWrap }

        Rectangle {
            width: parent.width; height: 58; radius: Tokens.radius; color: Tokens.surface; border.color: Tokens.outline
            Column { anchors.left: parent.left; anchors.right: parent.right; anchors.verticalCenter: parent.verticalCenter; anchors.leftMargin: Tokens.spaceMd; anchors.rightMargin: Tokens.spaceMd; spacing: 2
                Text { width: parent.width; text: "Workspace " + (CompositorService.activeWorkspace ? CompositorService.activeWorkspace.name : "1"); color: Tokens.textPrimary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontBody; font.bold: true; elide: Text.ElideRight }
                Text { width: parent.width; text: CompositorService.windowCount + " visible windows"; color: Tokens.textSecondary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontCaption; elide: Text.ElideRight }
            }
        }

        Repeater {
            model: CompositorService.workspaces
            Rectangle {
                required property var modelData
                width: body.width; height: 40; radius: Tokens.radius
                color: modelData.focused || modelData.active ? Tokens.accentDim : (hover.containsMouse ? Tokens.surfaceRaised : Tokens.surface)
                border.color: modelData.urgent ? Tokens.warning : Tokens.outline
                Text { anchors.left: parent.left; anchors.right: parent.right; anchors.verticalCenter: parent.verticalCenter; anchors.leftMargin: Tokens.spaceMd; anchors.rightMargin: Tokens.spaceMd; text: "Workspace " + (modelData.number > 0 ? modelData.number : modelData.name); color: Tokens.textPrimary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontBody; elide: Text.ElideRight }
                MouseArea { id: hover; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: CompositorService.focusWorkspace(modelData.number) }
            }
        }
    }
}
