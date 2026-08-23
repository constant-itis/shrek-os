import QtQuick
import "../../theme"

Item {
    component Page: Flickable {
        contentWidth: width
        contentHeight: body.implicitHeight
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        default property alias content: body.data
        Column {
            id: body
            width: parent.width
            spacing: Tokens.spaceMd
        }
    }

    component Heading: Column {
        property string title: ""
        property string detail: ""
        width: parent ? parent.width : 360
        spacing: 2
        Text {
            width: parent.width
            text: parent.title
            color: Tokens.textPrimary
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontHeadline
            font.bold: true
            elide: Text.ElideRight
        }
        Text {
            width: parent.width
            text: parent.detail
            color: Tokens.textSecondary
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontCaption
            wrapMode: Text.WordWrap
        }
    }

    component RowCard: Rectangle {
        property string title: ""
        property string detail: ""
        property bool active: false
        property bool enabledRow: true
        property var action
        width: parent ? parent.width : 360
        height: Math.max(46, textBlock.implicitHeight + 2 * Tokens.spaceSm)
        radius: Tokens.radius
        color: active ? Tokens.accentDim : Tokens.surface
        border.color: hover.containsMouse ? Tokens.accent : Tokens.outline
        opacity: enabledRow ? 1 : 0.45
        Column {
            id: textBlock
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            anchors.leftMargin: Tokens.spaceMd
            anchors.rightMargin: Tokens.spaceMd
            spacing: 2
            Text {
                width: parent.width
                text: title
                color: active ? Tokens.accentText : Tokens.textPrimary
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontBody
                font.bold: true
                elide: Text.ElideRight
            }
            Text {
                width: parent.width
                visible: detail.length > 0
                text: detail
                color: active ? Tokens.accentText : Tokens.textSecondary
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontCaption
                elide: Text.ElideRight
            }
        }
        MouseArea {
            id: hover
            anchors.fill: parent
            enabled: enabledRow && action
            hoverEnabled: true
            cursorShape: enabled ? Qt.PointingHandCursor : Qt.ArrowCursor
            onClicked: if (action) action()
        }
    }

    component Toggle: Rectangle {
        property bool checked: false
        property bool available: true
        property var action
        width: 48; height: 26; radius: Tokens.radiusFull
        color: checked ? Tokens.accent : Tokens.surfaceRaised
        border.color: Tokens.outline
        opacity: available ? 1 : 0.45
        Rectangle {
            width: 20; height: 20; radius: Tokens.radiusFull
            anchors.verticalCenter: parent.verticalCenter
            x: parent.checked ? parent.width - width - 3 : 3
            color: parent.checked ? Tokens.accentText : Tokens.textSecondary
            Behavior on x { NumberAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }
        }
        MouseArea {
            anchors.fill: parent
            enabled: parent.available
            cursorShape: Qt.PointingHandCursor
            onClicked: if (parent.action) parent.action()
        }
    }
}
