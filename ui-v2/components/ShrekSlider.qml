import QtQuick
import "../theme"

Item {
    id: root

    property real value: 0
    property bool muted: false
    signal moved(real value)

    implicitHeight: Tokens.sliderHeight
    opacity: enabled ? 1 : 0.45

    Rectangle {
        id: track
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        height: Tokens.sliderTrack
        radius: Tokens.radiusFull
        color: Tokens.surfaceRaised
        border.width: 1
        border.color: drag.containsMouse ? Tokens.outlineStrong : Tokens.outline

        Rectangle {
            anchors.left: parent.left
            anchors.top: parent.top
            anchors.bottom: parent.bottom
            width: parent.width * Math.max(0, Math.min(1, root.value))
            radius: Tokens.radiusFull
            color: root.muted ? Tokens.muted : Tokens.accent

            Behavior on width { NumberAnimation { duration: drag.pressed ? 0 : Tokens.animFast; easing.type: Easing.OutCubic } }
        }
    }

    Rectangle {
        width: 18
        height: 18
        radius: Tokens.radiusFull
        anchors.verticalCenter: track.verticalCenter
        x: Math.max(0, Math.min(root.width - width, root.value * root.width - width / 2))
        color: Tokens.textPrimary
        border.width: 1
        border.color: Tokens.outlineStrong
        visible: root.enabled

        Behavior on x { NumberAnimation { duration: drag.pressed ? 0 : Tokens.animFast; easing.type: Easing.OutCubic } }
    }

    MouseArea {
        id: drag
        anchors.fill: parent
        enabled: root.enabled
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onPressed: m => root.moved(Math.max(0, Math.min(1, m.x / width)))
        onPositionChanged: m => {
            if (pressed)
                root.moved(Math.max(0, Math.min(1, m.x / width)))
        }
    }
}
