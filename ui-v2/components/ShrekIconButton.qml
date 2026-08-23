import QtQuick
import "../theme"

Rectangle {
    id: root

    property string icon: ""
    property string tooltip: ""
    property bool active: false
    property int buttonSize: Tokens.iconButtonSize
    property int iconSize: 18
    signal activated()

    width: buttonSize
    height: buttonSize
    radius: Tokens.radius
    color: !enabled ? Tokens.surface :
           active ? Tokens.accentDim :
           press.containsMouse ? Tokens.surfaceRaised : "transparent"
    border.width: active || press.containsMouse ? 1 : 0
    border.color: active ? Tokens.accent : Tokens.outline
    opacity: enabled ? 1 : 0.45

    Behavior on color { ColorAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }

    Text {
        anchors.centerIn: parent
        text: root.icon
        color: root.active ? Tokens.textPrimary : Tokens.textSecondary
        font.family: Tokens.fontFamily
        font.pixelSize: root.iconSize
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
    }

    MouseArea {
        id: press
        anchors.fill: parent
        enabled: root.enabled
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: root.activated()
    }
}
