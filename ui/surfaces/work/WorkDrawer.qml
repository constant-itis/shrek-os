import Quickshell
import Quickshell.Wayland
import QtQuick
import "../../themes"
import "../../state"

// WorkDrawer — the Work zone. Driven by an injected read-only `provider` (the real SessionProvider that
// reads gatekeeperd-authored shrek-session/1 records). Now toggleable (Super+W via IPC) instead of
// always-on. Renders provider.sessions GENERICALLY: empty -> "Nothing running"; else one opaque row per
// record (title + subtitle projected by the provider). Strictly DISPLAY — NO authority badges, NO
// grant/stop actions, NO mutation. The read-only shrek-session/1 path is unchanged.
PanelWindow {
    id: drawer
    property var provider

    WlrLayershell.layer: WlrLayer.Top
    visible: ShellState.workOpen
    anchors { top: true; right: true; bottom: true }
    implicitWidth: Tokens.drawerWidth
    color: Tokens.panelBg

    // left hairline separating the drawer from content
    Rectangle {
        anchors { left: parent.left; top: parent.top; bottom: parent.bottom }
        width: 1
        color: Tokens.border
    }

    Column {
        anchors.fill: parent
        anchors.margins: Tokens.spaceLg
        spacing: Tokens.spaceMd

        Text {
            text: "Work"
            color: Tokens.text
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontTitle
            font.bold: true
        }

        Text {
            visible: !drawer.provider || drawer.provider.sessions.length === 0
            text: "Nothing running"
            color: Tokens.textDim
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontBody
        }

        Repeater {
            model: drawer.provider ? drawer.provider.sessions : []

            Rectangle {
                width: drawer.width - 2 * Tokens.spaceLg
                height: 54
                radius: Tokens.radius
                color: Tokens.surface
                border.color: Tokens.border

                Column {
                    anchors {
                        left: parent.left; right: parent.right; verticalCenter: parent.verticalCenter
                        leftMargin: Tokens.spaceMd; rightMargin: Tokens.spaceMd
                    }
                    spacing: 2

                    Text {
                        width: parent.width
                        text: modelData.title !== undefined ? modelData.title : "session"
                        color: Tokens.text
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontBody
                        elide: Text.ElideRight
                    }
                    Text {
                        width: parent.width
                        visible: modelData.subtitle !== undefined && ("" + modelData.subtitle).length > 0
                        text: modelData.subtitle || ""
                        color: Tokens.textFaint
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontCaption
                        elide: Text.ElideRight
                    }
                }
            }
        }
    }
}
