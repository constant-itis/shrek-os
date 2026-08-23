import QtQuick
import "../theme"

Rectangle {
    id: root

    property string title: ""
    property string detail: ""
    property string icon: ""
    property bool active: false
    property bool enabledRow: true
    default property alias content: trailing.data
    signal activated()

    width: parent ? parent.width : 360
    implicitHeight: Math.max(Tokens.rowHeight, textBlock.implicitHeight + Tokens.spaceMd * 2)
    radius: active ? Tokens.radius : Tokens.radiusLg
    color: active ? Tokens.accentDim : (hover.containsMouse ? Tokens.surfaceRaised : Tokens.surface)
    border.width: 1
    border.color: active ? Tokens.accent : (hover.containsMouse ? Tokens.outlineStrong : Tokens.outline)
    opacity: enabledRow ? 1 : 0.45

    Behavior on color { ColorAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }
    Behavior on radius { NumberAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }
    Behavior on opacity { NumberAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }

    Rectangle {
        anchors.fill: parent
        radius: parent.radius
        color: root.active ? Tokens.accentText : Tokens.textPrimary
        opacity: hover.pressed ? 0.16 : (hover.containsMouse ? 0.08 : 0)
        visible: opacity > 0

        Behavior on opacity { NumberAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }
    }

    Text {
        id: iconLabel
        anchors.left: parent.left
        anchors.leftMargin: Tokens.spaceLg
        anchors.verticalCenter: parent.verticalCenter
        width: root.icon.length > 0 ? Tokens.fontHeadline : 0
        visible: root.icon.length > 0
        text: root.icon
        color: root.active ? Tokens.accentText : Tokens.accent
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontTitle
        font.weight: Font.DemiBold
        horizontalAlignment: Text.AlignHCenter
    }

    Column {
        id: textBlock
        anchors.left: iconLabel.visible ? iconLabel.right : parent.left
        anchors.right: trailing.left
        anchors.verticalCenter: parent.verticalCenter
        anchors.leftMargin: iconLabel.visible ? Tokens.spaceMd : Tokens.spaceLg
        anchors.rightMargin: trailing.children.length > 0 ? Tokens.spaceMd : Tokens.spaceMd
        spacing: 2

        Text {
            width: parent.width
            text: root.title
            color: root.active ? Tokens.accentText : Tokens.textPrimary
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontBody
            font.weight: Font.DemiBold
            elide: Text.ElideRight
        }

        Text {
            width: parent.width
            visible: root.detail.length > 0
            text: root.detail
            color: root.active ? Tokens.accentText : Tokens.textSecondary
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
