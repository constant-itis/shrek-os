import Quickshell
import QtQuick
import "../../themes"
import "../../state"

// BarActions — mouse affordances at the far left of the bar so the desktop is fully operable WITHOUT the
// keyboard (a host compositor easily swallows the Super key). Apps button opens the launcher; terminal
// button spawns foot. Ordinary user actions; no authority.
Column {
    id: root
    spacing: Tokens.spaceSm

    // apps / launcher
    Rectangle {
        anchors.horizontalCenter: parent.horizontalCenter
        width: 30; height: 26; radius: Tokens.radius
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
            acceptedButtons: Qt.LeftButton | Qt.RightButton
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onEntered: ShellState.openRailPopout("apps", parent.mapToItem(null, 0, parent.height / 2).y)
            onExited: ShellState.closeRailPopout("apps")
            // Left = launcher; right = the root context menu, dropped just under the bar so it is
            // reachable even when windows cover the bare desktop.
            onClicked: (m) => {
                if (m.button === Qt.RightButton)
                    ShellState.openMenu(Tokens.railWidth + 2 * Tokens.spaceSm, Tokens.spaceLg, Menus.root())
                else
                    ShellState.toggleLauncher()
            }
        }
    }

    // terminal
    Rectangle {
        anchors.horizontalCenter: parent.horizontalCenter
        width: 30; height: 26; radius: Tokens.radius
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
            onEntered: ShellState.openRailPopout("terminal", parent.mapToItem(null, 0, parent.height / 2).y)
            onExited: ShellState.closeRailPopout("terminal")
            onClicked: Quickshell.execDetached(["foot", "--font=DejaVu Sans Mono:size=11"])
        }
    }
}
