import QtQuick
import "../theme"

Rectangle {
    id: root

    property string label: ""
    property bool active: false
    property bool compact: true
    property bool vertical: true
    signal activated()

    width: vertical ? (compact ? 36 : 44) : (compact ? 44 : 104)
    height: vertical ? (compact ? 36 : 44) : 36
    radius: active ? Tokens.radius : Tokens.radiusLg
    color: active ? Tokens.accent : (hover.containsMouse ? Tokens.surfaceRaised : Tokens.surface)
    border.width: active ? 1 : (hover.containsMouse ? 1 : 0)
    border.color: active ? Tokens.accent : Tokens.outline

    Behavior on color { ColorAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }
    Behavior on radius { NumberAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }

    Rectangle {
        anchors.fill: parent
        radius: parent.radius
        color: active ? Tokens.accentText : Tokens.textPrimary
        opacity: hover.pressed ? 0.16 : (hover.containsMouse ? 0.08 : 0)
        visible: opacity > 0

        Behavior on opacity { NumberAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }
    }

    Text {
        anchors.centerIn: parent
        width: parent.width - Tokens.spaceSm
        text: root.label
        color: root.active ? Tokens.accentText : Tokens.textPrimary
        font.family: Tokens.fontFamily
        font.pixelSize: root.compact ? Tokens.fontBody : Tokens.fontSmall
        font.weight: root.active ? Font.DemiBold : Font.Medium
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
