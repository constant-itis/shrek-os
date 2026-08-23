import QtQuick
import "../theme"

Rectangle {
    id: root

    property string text: ""

    implicitWidth: Math.min(300, label.implicitWidth + Tokens.spaceMd * 2)
    implicitHeight: label.implicitHeight + Tokens.spaceSm * 2
    radius: Tokens.radius
    color: Tokens.overlay
    border.width: 1
    border.color: Tokens.outlineStrong

    Text {
        id: label
        anchors.centerIn: parent
        width: Math.min(260, implicitWidth)
        text: root.text
        color: Tokens.textPrimary
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontCaption
        elide: Text.ElideRight
    }
}
