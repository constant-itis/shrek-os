import Quickshell
import Quickshell.Wayland
import QtQuick
import "../../themes"
import "../../state"
import "../../services"
import "../../geometry"

// QuickDock — a right-edge contextual action cluster. It is edge-triggered instead of always visible:
// the normal desktop should read as a coherent framed stage, while screenshot/clipboard/focus remain
// available to mouse users when they intentionally touch the right edge.
PanelWindow {
    id: dock
    WlrLayershell.layer: WlrLayer.Top
    anchors { top: true; right: true; bottom: true }
    implicitWidth: 64
    color: "transparent"
    visible: !(ShellState.workOpen || ShellState.systemOpen || ShellState.dashboardOpen
               || ShellState.launcherOpen || ShellState.clipboardOpen || ShellState.menuOpen)

    property bool expanded: false

    // The shell only captures the edge trigger while closed, then the revealed notch while open.
    mask: Region { item: hitRegion }

    Item {
        id: hitRegion
        anchors { top: parent.top; right: parent.right; bottom: parent.bottom }
        width: dock.expanded ? dock.implicitWidth : 6

        MouseArea {
            anchors.fill: parent
            hoverEnabled: true
            onEntered: dock.expanded = true
            onExited: dock.expanded = false
        }
    }

    Item {
        id: socket
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        width: 54
        height: col.implicitHeight + 2 * Tokens.spaceSm
        opacity: dock.expanded ? 1 : 0
        transform: Translate { x: dock.expanded ? 0 : 14 }

        Behavior on opacity { NumberAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }

        EdgePanelShape {
            anchors.fill: parent
            edge: "right"
            fill: Tokens.panelBg
            stroke: Tokens.border
        }
    }

    Column {
        id: col
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        anchors.rightMargin: Tokens.spaceSm
        spacing: Tokens.spaceSm
        opacity: dock.expanded ? 1 : 0
        transform: Translate { x: dock.expanded ? 0 : 14 }

        Behavior on opacity { NumberAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }

        // a circular action button with a drawn glyph (no icon font in the layer)
        component DockBtn: Rectangle {
            id: btn
            property string kind: ""
            property bool on: false
            property var act
            width: 40; height: 40; radius: 20
            color: bma.containsMouse ? Tokens.accent : (on ? Tokens.accentDim : Tokens.surface)
            border.color: bma.containsMouse || on ? Tokens.accent : Tokens.border
            border.width: 1
            Behavior on color { ColorAnimation { duration: Tokens.animFast } }

            readonly property color ink: bma.containsMouse ? Tokens.accentText : Tokens.text

            // screenshot — viewfinder: rounded frame + centre dot
            Item {
                anchors.centerIn: parent
                width: 18; height: 16
                visible: btn.kind === "shot"
                Rectangle { anchors.fill: parent; radius: 3; color: "transparent"; border.color: btn.ink; border.width: 2 }
                Rectangle { anchors.centerIn: parent; width: 6; height: 6; radius: 3; color: btn.ink }
            }
            // clipboard — three stacked lines
            Column {
                anchors.centerIn: parent
                spacing: 3
                visible: btn.kind === "clip"
                Repeater { model: 3; Rectangle { width: 16; height: 2; radius: 1; color: btn.ink } }
            }
            // focus / DND — a ring that fills when active
            Rectangle {
                anchors.centerIn: parent
                width: 16; height: 16; radius: 8
                visible: btn.kind === "focus"
                color: btn.on ? btn.ink : "transparent"
                border.color: btn.ink; border.width: 2
            }

            MouseArea {
                id: bma
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onEntered: dock.expanded = true
                onClicked: if (btn.act) btn.act()
            }
        }

        DockBtn { kind: "shot";  act: function () { ShellState.closeAll(); Screenshot.region() } }
        DockBtn { kind: "clip";  act: function () { ShellState.toggleClipboard() } }
        DockBtn { kind: "focus"; on: Notifications.dnd; act: function () { Notifications.toggleDnd() } }
    }
}
