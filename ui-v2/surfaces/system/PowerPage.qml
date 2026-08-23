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

        Text { width: parent.width; text: "Power"; color: Tokens.textPrimary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontHeadline; font.bold: true }
        Text { width: parent.width; text: Power.present ? (Math.round(Power.percentage) + "% - " + Power.estimate) : "AC power. No laptop battery reported by UPower."; color: Tokens.textSecondary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontCaption; wrapMode: Text.WordWrap }

        Rectangle {
            width: parent.width; height: 58; radius: Tokens.radius; color: Tokens.surface; border.color: Tokens.outline
            Column { anchors.left: parent.left; anchors.right: parent.right; anchors.verticalCenter: parent.verticalCenter; anchors.leftMargin: Tokens.spaceMd; anchors.rightMargin: Tokens.spaceMd; spacing: 2
                Text { width: parent.width; text: Power.present ? Math.round(Power.percentage) + "% " + Power.state : "Desktop power"; color: Tokens.textPrimary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontBody; font.bold: true; elide: Text.ElideRight }
                Text { width: parent.width; text: Power.onBattery ? "Battery" : "AC"; color: Tokens.textSecondary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontCaption; elide: Text.ElideRight }
            }
        }

        Text { width: parent.width; visible: Power.profilesAvailable; text: "Power profile"; color: Tokens.textSecondary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontCaption }
        Repeater {
            model: Power.profiles
            Rectangle {
                required property string modelData
                width: body.width; height: 40; radius: Tokens.radius
                visible: Power.profilesAvailable
                color: Power.profile === modelData ? Tokens.accentDim : (hover.containsMouse ? Tokens.surfaceRaised : Tokens.surface)
                border.color: Power.profile === modelData ? Tokens.accent : Tokens.outline
                Text { anchors.left: parent.left; anchors.right: parent.right; anchors.verticalCenter: parent.verticalCenter; anchors.leftMargin: Tokens.spaceMd; anchors.rightMargin: Tokens.spaceMd; text: modelData; color: Tokens.textPrimary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontBody; elide: Text.ElideRight }
                MouseArea { id: hover; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: Power.setProfile(modelData) }
            }
        }
    }
}
