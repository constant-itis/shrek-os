import QtQuick
import "../../themes"
import "../../state"

// PowerButton — a visible session/power affordance at the far right of the bar. Opens the root context
// menu (its session actions — Log out / Reboot / Power off — sit at the bottom) clamped to the screen's
// right edge, so power is reachable by mouse without knowing a keybind or hunting the right-click.
Item {
    id: root
    implicitWidth: 26
    implicitHeight: 24
    readonly property bool hot: ma.containsMouse

    Rectangle {
        anchors.fill: parent
        radius: Tokens.radiusSm
        color: root.hot ? Tokens.surfaceAlt : "transparent"
        border.color: root.hot ? Tokens.border : "transparent"
        border.width: 1
    }

    // power glyph: a ring with a stem through the top gap
    Item {
        anchors.centerIn: parent
        width: 14; height: 14
        readonly property color ink: root.hot ? Tokens.danger : Tokens.textDim

        Rectangle {   // ring
            anchors.centerIn: parent
            width: 14; height: 14; radius: 7
            color: "transparent"
            border.width: 2
            border.color: parent.ink
        }
        Rectangle {   // gap mask (bar background shows through), then the stem over it
            anchors.horizontalCenter: parent.horizontalCenter
            y: -1
            width: 4; height: 6
            color: Tokens.barBg
        }
        Rectangle {   // stem
            anchors.horizontalCenter: parent.horizontalCenter
            y: 0
            width: 2; height: 7; radius: 1
            color: parent.ink
        }
    }

    MouseArea {
        id: ma
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onEntered: ShellState.openRailPopout("power", root.mapToItem(null, 0, root.height / 2).y)
        onExited: ShellState.closeRailPopout("power")
        onClicked: ShellState.openMenu(Tokens.railWidth + 2 * Tokens.spaceSm, 99999, Menus.root())
    }
}
