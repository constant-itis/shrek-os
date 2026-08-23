import QtQuick
import "../theme"

Rectangle {
    id: root

    property bool checked: false
    property bool available: true
    signal toggled()

    width: Tokens.toggleWidth
    height: Tokens.toggleHeight
    radius: Tokens.radiusFull
    color: checked ? Tokens.accent : Tokens.surfaceRaised
    border.width: 1
    border.color: press.containsMouse ? Tokens.outlineStrong : Tokens.outline
    opacity: available ? 1 : 0.45

    Behavior on color { ColorAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }
    Behavior on opacity { NumberAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }

    Rectangle {
        width: 20
        height: 20
        radius: Tokens.radiusFull
        anchors.verticalCenter: parent.verticalCenter
        x: root.checked ? parent.width - width - 3 : 3
        color: root.checked ? Tokens.accentText : Tokens.textSecondary

        Behavior on x { NumberAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }
    }

    MouseArea {
        id: press
        anchors.fill: parent
        enabled: root.available
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: root.toggled()
    }
}
