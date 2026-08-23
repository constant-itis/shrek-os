import Quickshell
import Quickshell.Wayland
import QtQuick
import "../../themes"
import "../../state"
import "../../services"

// QuickDock — a right-edge cluster of circular quick-action buttons. Purpose-driven for mouse-first use
// (a host compositor can swallow Super, so surfacing keybind-only actions as visible buttons matters):
// screenshot region, clipboard history, focus/Do-Not-Disturb. Only the buttons capture input (mask) so
// the rest of the edge stays click-through to windows. Display + ordinary user actions only — no authority.
PanelWindow {
    id: dock
    WlrLayershell.layer: WlrLayer.Top
    anchors { top: true; right: true; bottom: true }
    implicitWidth: 56
    color: "transparent"

    // only the button column is interactive; the rest of the strip passes clicks through to windows
    mask: Region { item: col }

    Column {
        id: col
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        anchors.rightMargin: Tokens.spaceSm
        spacing: Tokens.spaceSm

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
                onClicked: if (btn.act) btn.act()
            }
        }

        DockBtn { kind: "shot";  act: function () { ShellState.closeAll(); Screenshot.region() } }
        DockBtn { kind: "clip";  act: function () { ShellState.toggleClipboard() } }
        DockBtn { kind: "focus"; on: Notifications.dnd; act: function () { Notifications.toggleDnd() } }
    }
}
