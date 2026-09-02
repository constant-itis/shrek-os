import QtQuick
import QtQuick.Layouts
import "../theme"

// A selectable disk row for the disk picker (static). `model_`/`dev`/`size` avoid the reserved `model`.
Rectangle {
    id: row
    property bool selected: false
    property bool excluded: false
    property string model_: ""
    property string dev: ""
    property string size: ""

    implicitHeight: 60
    radius: Tokens.radius
    opacity: excluded ? 0.5 : 1.0
    color: selected ? Tokens.rowHighlight : Tokens.surface
    border.width: 1
    border.color: selected ? Tokens.accent : Tokens.outline

    RowLayout {
        anchors.fill: parent
        anchors.leftMargin: 16
        anchors.rightMargin: 16
        spacing: 14

        Rectangle {
            width: 18
            height: 18
            radius: 9
            color: "transparent"
            border.width: 2
            border.color: row.selected ? Tokens.accent : Tokens.muted
            Rectangle {
                visible: row.selected
                anchors.centerIn: parent
                width: 9
                height: 9
                radius: 4.5
                color: Tokens.accent
            }
        }

        ColumnLayout {
            spacing: 2
            Layout.fillWidth: true
            Text {
                text: row.model_
                color: Tokens.textPrimary
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontBody
            }
            Text {
                text: row.dev
                color: Tokens.muted
                font.family: Tokens.fontMono
                font.pixelSize: Tokens.fontSmall
            }
        }

        Text {
            text: row.size
            color: row.excluded ? Tokens.muted : Tokens.textSecondary
            font.family: Tokens.fontMono
            font.pixelSize: row.excluded ? Tokens.fontCaption : Tokens.fontBody
        }
    }
}
