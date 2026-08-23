import QtQuick
import "../../themes"
import "../../services"

// Workspaces — live Sway workspaces from the Sway service (native i3/Sway IPC). Focused = accent fill;
// active-on-another-output = raised; urgent = notice border. Click switches. Reacts to Sway events; no
// polling.
Column {
    id: root
    spacing: Tokens.spaceSm

    Repeater {
        model: Sway.workspaces

        Item {
            implicitWidth: 26
            implicitHeight: 22

            Rectangle {
                anchors.fill: parent
                radius: Tokens.radiusSm
                color: modelData.focused ? Tokens.accent
                     : modelData.active  ? Tokens.surfaceAlt
                                          : "transparent"
                border.width: 1
                border.color: modelData.urgent ? Tokens.notice : Tokens.border
            }

            Text {
                id: label
                anchors.centerIn: parent
                text: modelData.name
                color: modelData.focused ? Tokens.accentText : Tokens.textDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
            }

            MouseArea {
                anchors.fill: parent
                cursorShape: Qt.PointingHandCursor
                onClicked: modelData.activate()
            }
        }
    }
}
