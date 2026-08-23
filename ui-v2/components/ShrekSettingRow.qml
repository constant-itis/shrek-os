import QtQuick
import "../theme"

Rectangle {
    id: root

    property string title: ""
    property string detail: ""
    property bool active: false
    property bool enabledRow: true
    default property alias content: trailing.data
    signal activated()

    width: parent ? parent.width : 360
    implicitHeight: Math.max(Tokens.rowHeight, textBlock.implicitHeight + Tokens.spaceMd * 2)
    radius: Tokens.radius
    color: active ? Tokens.surfaceRaised : (hover.containsMouse ? Tokens.surfaceRaised : Tokens.surface)
    border.width: 1
    border.color: active ? Tokens.accent : (hover.containsMouse ? Tokens.outlineStrong : Tokens.outline)
    opacity: enabledRow ? 1 : 0.45

    Behavior on color { ColorAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }
    Behavior on opacity { NumberAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }

    Rectangle {
        anchors.left: parent.left
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        width: root.active ? 4 : 0
        radius: Tokens.radiusSm
        color: Tokens.accent
        visible: width > 0

        Behavior on width { NumberAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }
    }

    Column {
        id: textBlock
        anchors.left: parent.left
        anchors.right: trailing.left
        anchors.verticalCenter: parent.verticalCenter
        anchors.leftMargin: root.active ? Tokens.spaceLg : Tokens.spaceMd
        anchors.rightMargin: trailing.children.length > 0 ? Tokens.spaceMd : Tokens.spaceMd
        spacing: 2

        Text {
            width: parent.width
            text: root.title
            color: root.active ? Tokens.textPrimary : Tokens.textPrimary
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontBody
            font.weight: Font.DemiBold
            elide: Text.ElideRight
        }

        Text {
            width: parent.width
            visible: root.detail.length > 0
            text: root.detail
            color: root.active ? Tokens.textPrimary : Tokens.textSecondary
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontCaption
            elide: Text.ElideRight
        }
    }

    Row {
        id: trailing
        anchors.right: parent.right
        anchors.rightMargin: Tokens.spaceMd
        anchors.verticalCenter: parent.verticalCenter
        spacing: Tokens.spaceSm
    }

    MouseArea {
        id: hover
        anchors.left: parent.left
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        anchors.right: trailing.children.length > 0 ? trailing.left : parent.right
        enabled: root.enabledRow
        hoverEnabled: true
        cursorShape: enabled ? Qt.PointingHandCursor : Qt.ArrowCursor
        onClicked: root.activated()
    }
}
