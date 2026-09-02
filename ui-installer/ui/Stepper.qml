import QtQuick
import "../theme"

// The 7-node install progress indicator. Encodes true sequence state: done (check) / active / upcoming.
Row {
    id: root
    property int current: 1
    readonly property var labels: ["Welcome", "Language", "Name", "Disk", "Confirm", "Install", "Done"]
    spacing: 0

    Repeater {
        model: root.labels.length
        delegate: Row {
            id: node
            required property int index
            readonly property bool done: (index + 1) < root.current
            readonly property bool active: (index + 1) === root.current
            spacing: 7
            rightPadding: 14

            Rectangle {
                width: 20
                height: 20
                radius: 10
                anchors.verticalCenter: parent.verticalCenter
                color: node.done ? Tokens.accentDim : node.active ? Tokens.accent : Tokens.surface
                border.width: 1
                border.color: node.done ? Tokens.accentDim : node.active ? Tokens.accent : Tokens.outline

                Text {
                    anchors.centerIn: parent
                    text: node.done ? "✓" : (node.index + 1)
                    font.family: Tokens.fontMono
                    font.pixelSize: 10
                    color: (node.done || node.active) ? Tokens.accentText : Tokens.muted
                }
            }

            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: root.labels[node.index]
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontCaption
                font.weight: node.active ? Font.DemiBold : Font.Normal
                color: node.done ? Tokens.textSecondary : node.active ? Tokens.textPrimary : Tokens.muted
            }
        }
    }
}
