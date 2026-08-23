import QtQuick
import "../theme"

Rectangle {
    id: root

    property string label: ""
    property color tint: Tokens.textSecondary
    property bool compact: true
    property bool vertical: true
    signal activated()

    width: vertical ? 36 : (compact ? 44 : 72)
    height: vertical ? 28 : 28
    radius: Tokens.radiusSm
    color: hover.containsMouse ? Tokens.surfaceRaised : Tokens.surface
    border.width: hover.containsMouse ? 1 : 0
    border.color: hover.containsMouse ? Tokens.outlineStrong : Tokens.outline

    Behavior on color { ColorAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }

    Text {
        anchors.centerIn: parent
        width: parent.width - Tokens.spaceXs
        text: root.label
        color: root.tint
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontCaption
        font.weight: root.tint === Tokens.accent ? Font.DemiBold : Font.Normal
        horizontalAlignment: Text.AlignHCenter
        elide: Text.ElideRight
    }

    MouseArea {
        id: hover
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: root.activated()
    }
}
