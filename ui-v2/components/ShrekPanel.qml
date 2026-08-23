import QtQuick
import "../theme"

Rectangle {
    id: root

    default property alias content: contentLayer.data

    color: Tokens.panelBg
    radius: Tokens.radiusLg
    border.width: 1
    border.color: Tokens.outline
    clip: true

    Item {
        id: contentLayer
        anchors.fill: parent
        anchors.margins: Tokens.panelPadding
    }
}
