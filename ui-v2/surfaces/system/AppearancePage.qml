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

        Text { width: parent.width; text: "Appearance"; color: Tokens.textPrimary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontHeadline; font.bold: true }
        Text { width: parent.width; text: "Semantic Shrek theme modes. Dynamic and Custom keep their contracts even when no provider is active."; color: Tokens.textSecondary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontCaption; wrapMode: Text.WordWrap }

        Repeater {
            model: Appearance.modes
            Rectangle {
                required property var modelData
                width: body.width; height: 50; radius: Tokens.radius
                color: Appearance.mode === modelData.mode ? Tokens.accentDim : (hover.containsMouse ? Tokens.surfaceRaised : Tokens.surface)
                border.color: Appearance.mode === modelData.mode ? Tokens.accent : Tokens.outline
                Column { anchors.left: parent.left; anchors.right: parent.right; anchors.verticalCenter: parent.verticalCenter; anchors.leftMargin: Tokens.spaceMd; anchors.rightMargin: Tokens.spaceMd; spacing: 2
                    Text { width: parent.width; text: modelData.label; color: Tokens.textPrimary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontBody; font.bold: true; elide: Text.ElideRight }
                    Text { width: parent.width; text: modelData.detail; color: Appearance.mode === modelData.mode ? Tokens.textPrimary : Tokens.textSecondary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontCaption; elide: Text.ElideRight }
                }
                MouseArea { id: hover; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: Appearance.setMode(modelData.mode) }
            }
        }
    }
}
