import QtQuick
import "../theme"

Item {
    id: root

    property real value: 0
    property bool muted: false
    property string icon: ""
    property string valueText: ""
    signal moved(real value)

    width: parent ? parent.width : 260
    implicitHeight: Tokens.sliderHeight
    height: implicitHeight
    opacity: enabled ? 1 : 0.45

    Rectangle {
        id: iconButton
        width: root.icon.length > 0 ? Tokens.tileHeightSm - Tokens.spaceSm : 0
        height: Tokens.tileHeightSm - Tokens.spaceSm
        anchors.left: parent.left
        anchors.verticalCenter: parent.verticalCenter
        radius: Tokens.radiusFull
        color: iconHover.containsMouse ? Tokens.accentDim : "transparent"
        visible: root.icon.length > 0

        Text {
            anchors.centerIn: parent
            text: root.icon
            color: root.muted ? Tokens.textSecondary : Tokens.accent
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontTitle
            font.weight: Font.DemiBold
        }

        MouseArea {
            id: iconHover
            anchors.fill: parent
            hoverEnabled: true
            enabled: false
        }
    }

    Rectangle {
        id: track
        anchors.left: iconButton.visible ? iconButton.right : parent.left
        anchors.leftMargin: iconButton.visible ? Tokens.spaceSm : 0
        anchors.right: valueLabel.visible ? valueLabel.left : parent.right
        anchors.rightMargin: valueLabel.visible ? Tokens.spaceSm : 0
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
        x: track.x + Math.max(0, Math.min(track.width - width, root.value * track.width - width / 2))
        color: Tokens.textPrimary
        border.width: 1
        border.color: Tokens.outlineStrong
        visible: root.enabled

        Behavior on x { NumberAnimation { duration: drag.pressed ? 0 : Tokens.animFast; easing.type: Easing.OutCubic } }
    }

    MouseArea {
        id: drag
        anchors.left: track.left
        anchors.right: track.right
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        enabled: root.enabled
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onPressed: m => root.moved(Math.max(0, Math.min(1, m.x / width)))
        onPositionChanged: m => {
            if (pressed)
                root.moved(Math.max(0, Math.min(1, m.x / width)))
        }
    }

    Text {
        id: valueLabel
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        visible: root.valueText.length > 0
        text: root.valueText
        color: Tokens.textSecondary
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontCaption
        horizontalAlignment: Text.AlignRight
    }
}
