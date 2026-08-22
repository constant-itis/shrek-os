import Quickshell
import QtQuick
import "../../themes"
import "../../state"

// BarActions — mouse affordances at the far left of the bar so the desktop is fully operable WITHOUT the
// keyboard (a host compositor easily swallows the Super key). Apps button opens the launcher; terminal
// button spawns foot. Ordinary user actions; no authority.
Row {
    id: root
    spacing: Tokens.spaceSm

    // apps / launcher
    Rectangle {
        anchors.verticalCenter: parent.verticalCenter
        width: 30; height: 24; radius: Tokens.radiusSm
        color: appsMa.containsMouse ? Tokens.accentDim : Tokens.accent

        Grid {
            anchors.centerIn: parent
            columns: 2
            rowSpacing: 3
            columnSpacing: 3
            Repeater {
                model: 4
                Rectangle { width: 4; height: 4; radius: 1; color: Tokens.accentText }
            }
        }
        MouseArea {
            id: appsMa
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: ShellState.toggleLauncher()
        }
    }

    // terminal
    Rectangle {
        anchors.verticalCenter: parent.verticalCenter
        width: 30; height: 24; radius: Tokens.radiusSm
        color: termMa.containsMouse ? Tokens.surfaceAlt : Tokens.surface
        border.color: Tokens.border

        Text {
            anchors.centerIn: parent
            text: ">_"
            color: Tokens.textDim
            font.family: Tokens.fontMono
            font.pixelSize: Tokens.fontSmall
        }
        MouseArea {
            id: termMa
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: Quickshell.execDetached(["foot", "--font=DejaVu Sans Mono:size=11"])
        }
    }
}
