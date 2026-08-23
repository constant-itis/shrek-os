import QtQuick
import "../theme"

Column {
    id: root

    property string title: ""
    property string detail: ""
    default property alias content: body.data

    width: parent ? parent.width : 360
    spacing: Tokens.spaceSm

    Text {
        width: parent.width
        text: root.title
        color: Tokens.textPrimary
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontHeadline
        font.weight: Font.DemiBold
        elide: Text.ElideRight
    }

    Text {
        width: parent.width
        visible: root.detail.length > 0
        text: root.detail
        color: Tokens.textSecondary
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontCaption
        wrapMode: Text.WordWrap
    }

    Column {
        id: body
        width: parent.width
        spacing: Tokens.spaceSm
    }
}
