import QtQuick
import "../theme"

Rectangle {
    id: root

    property string text: ""
    property string kind: "default"
    property bool compact: false
    property int horizontalAlignment: Text.AlignHCenter
    signal activated()

    implicitWidth: label.implicitWidth + Tokens.spaceLg * 2
    implicitHeight: compact ? Tokens.controlHeightSm : Tokens.controlHeight
    width: implicitWidth
    height: implicitHeight
    radius: Tokens.radiusSm
    color: !enabled ? Tokens.surface :
           kind === "primary" ? (press.pressed ? Tokens.accentDim : Tokens.accent) :
           kind === "danger" ? (press.pressed ? Tokens.surfaceRaised : Tokens.surface) :
           kind === "ghost" ? (press.containsMouse ? Tokens.surfaceRaised : "transparent") :
           press.containsMouse ? Tokens.surfaceRaised : Tokens.surface
    border.width: kind === "primary" ? 0 : 1
    border.color: kind === "danger" ? Tokens.danger : Tokens.outline
    opacity: enabled ? 1 : 0.45

    Behavior on color { ColorAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }
    Behavior on opacity { NumberAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }

    Rectangle {
        anchors.fill: parent
        radius: parent.radius
        color: root.kind === "primary" ? Tokens.accentText : Tokens.textPrimary
        opacity: press.pressed ? 0.16 : (press.containsMouse ? 0.08 : 0)
        visible: opacity > 0

        Behavior on opacity { NumberAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }
    }

    Text {
        id: label
        anchors.verticalCenter: parent.verticalCenter
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.leftMargin: Tokens.spaceMd
        anchors.rightMargin: Tokens.spaceMd
        width: parent.width - Tokens.spaceMd
        text: root.text
        color: root.kind === "primary" ? Tokens.accentText :
               root.kind === "danger" ? Tokens.danger : Tokens.textPrimary
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontSmall
        font.weight: root.kind === "primary" ? Font.DemiBold : Font.Medium
        horizontalAlignment: root.horizontalAlignment
        elide: Text.ElideRight
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
