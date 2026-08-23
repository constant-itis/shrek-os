import QtQuick
import "../theme"

Rectangle {
    id: root

    default property alias content: body.data

    color: Tokens.panelBg
    radius: Tokens.radiusLg
    border.width: 1
    border.color: Tokens.outline
    clip: true

    Column {
        id: body
        anchors.fill: parent
        anchors.margins: Tokens.panelPadding
        spacing: Tokens.spaceMd
    }
}
