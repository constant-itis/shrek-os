import Quickshell
import Quickshell.Wayland
import QtQuick
import "../themes"

// WorkDrawer.qml — the Work drawer HOST. Driven by an injected read-only `provider`.
// It renders `provider.sessions` GENERICALLY: an empty list (no live sessions) shows
// "Nothing running"; otherwise one opaque row per gatekeeperd-authored shrek-session/1
// record. Strictly DISPLAY — NO authority badges, NO grant/stop actions, NO mutation.
PanelWindow {
    id: drawer
    property var provider

    anchors { top: true; right: true; bottom: true }
    implicitWidth: Tokens.drawerWidth
    color: Tokens.surface

    Column {
        anchors.fill: parent
        anchors.margins: Tokens.gap
        spacing: Tokens.gap

        Text { text: "Work"; color: Tokens.text; font.pixelSize: 15; font.bold: true }

        Text {
            visible: !drawer.provider || drawer.provider.sessions.length === 0
            text: "Nothing running"
            color: Tokens.textDim
            font.pixelSize: 13
        }

        // Generic row rendering (empty under the mock). Opaque schema on purpose.
        Repeater {
            model: drawer.provider ? drawer.provider.sessions : []
            Rectangle {
                width: drawer.width - 2 * Tokens.gap
                height: 40
                radius: Tokens.radius
                color: Tokens.bg
                border.color: Tokens.border
                Text {
                    anchors { left: parent.left; leftMargin: Tokens.gap; verticalCenter: parent.verticalCenter }
                    text: modelData.title !== undefined ? modelData.title : "session"
                    color: Tokens.text
                }
            }
        }
    }
}
