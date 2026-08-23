import Quickshell
import Quickshell.Wayland
import QtQuick
import "../../themes"
import "../../state"

// InteractionPlane — Shrek's first Caelestia-style edge interaction layer.
//
// The center of this full-screen window is not in the mask, so ordinary applications keep pointer input.
// Only narrow edge trigger items are clickable/hoverable. G3 routes the right edge to QuickDock; future
// slices can add top/bottom/drag routing without changing the shell content windows.
PanelWindow {
    id: plane

    WlrLayershell.layer: WlrLayer.Top
    WlrLayershell.exclusionMode: ExclusionMode.Ignore
    anchors { top: true; bottom: true; left: true; right: true }
    exclusiveZone: 0
    color: "transparent"
    visible: !(ShellState.workOpen || ShellState.systemOpen || ShellState.dashboardOpen
               || ShellState.launcherOpen || ShellState.clipboardOpen || ShellState.menuOpen)
    mask: Region { item: rightEdge }

    Item {
        id: rightEdge
        anchors { top: parent.top; right: parent.right; bottom: parent.bottom }
        width: ShellState.rightEdgeHot ? 64 : 6

        MouseArea {
            anchors.fill: parent
            hoverEnabled: true
            acceptedButtons: Qt.LeftButton
            onEntered: {
                closeTimer.stop()
                ShellState.openRightEdge()
            }
            onExited: closeTimer.restart()
            onPressed: (m) => dragStart = Qt.point(m.x, m.y)
            onPositionChanged: (m) => {
                if (pressed && dragStart.x - m.x > 18)
                    ShellState.openRightEdge()
            }
        }
    }

    property point dragStart

    Timer {
        id: closeTimer
        interval: 160
        repeat: false
        onTriggered: ShellState.closeRightEdge()
    }
}
