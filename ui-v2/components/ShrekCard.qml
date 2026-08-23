import QtQuick
import "../theme"

Rectangle {
    id: root

    property bool active: false
    property bool interactive: false
    default property alias content: body.data

    width: parent ? parent.width : 360
    implicitHeight: body.implicitHeight + Tokens.spaceMd * 2
    radius: Tokens.radius
    color: active ? Tokens.accentDim : (interactive && hover.containsMouse ? Tokens.surfaceRaised : Tokens.surface)
    border.width: 1
    border.color: active ? Tokens.accent : (interactive && hover.containsMouse ? Tokens.outlineStrong : Tokens.outline)

    Behavior on color { ColorAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }

    Rectangle {
        anchors.fill: parent
        radius: parent.radius
        color: root.active ? Tokens.accentText : Tokens.textPrimary
        opacity: root.interactive && hover.containsMouse ? 0.06 : 0
        visible: opacity > 0

        Behavior on opacity { NumberAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }
    }

    Column {
        id: body
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.margins: Tokens.spaceMd
        spacing: Tokens.spaceSm
    }

    MouseArea {
        id: hover
        anchors.fill: parent
        enabled: root.interactive
        hoverEnabled: root.interactive
    }
}
