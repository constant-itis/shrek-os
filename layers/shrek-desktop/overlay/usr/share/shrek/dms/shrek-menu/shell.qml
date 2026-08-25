// shrek-menu — SPIKE (docs/omarchy-portability.md Appendix; mycelium menu-engine GO decision).
//
// The 1-day de-risking spike: an EMPTY shrek-owned standalone Quickshell surface that opens via IPC,
// grabs keyboard focus, and closes on Escape. It proves the surface/IPC/focus shell works BEFORE any
// MenuModel.js port. NOT a DMS plugin (DMS's plugin schema has no surface type and plugins can't make a
// top-level PanelWindow) and NOT spliced into DMS's tree — a SEPARATE `qs` process, because Quickshell
// IPC target names are scoped to one instance and `dms ipc` is hard-pinned to DMS's own PID/config
// (verified from quickshell command.cpp::selectInstance + DankMaterialShell shell.go). So:
//
//   launch:  exec_always qs -p /usr/share/shrek/dms/shrek-menu/shell.qml   (from sway.config)
//   toggle:  qs -p /usr/share/shrek/dms/shrek-menu/shell.qml ipc call shrek-menu toggle   ($mod+slash)
//
// `dms ipc call shrek-menu` will NOT reach this process — the toggle MUST go through `qs -p <this file>`.
//
// THEME: `import qs.Common` (DMS's Theme/Color singletons) is per-instance and cross-instance imports
// are blackholed by quickshell's qs:// URL interceptor — a second process CANNOT import it. Parity comes
// later from reading ~/.cache/DankMaterialShell/dms-colors.json (matugen output; schema is verify-live).
// For the spike, a baked swamp palette matching DMS's default green so it reads native out of the box.
import QtQuick
import Quickshell
import Quickshell.Io
import Quickshell.Wayland

ShellRoot {
    id: root

    Component.onCompleted: console.info("SHREK-MENU spike surface loaded (hidden until toggle)")

    // Baked swamp palette (DMS default-green parity). TODO(next slice): source from dms-colors.json.
    readonly property color cSurface:     "#1b2a1a"
    readonly property color cSurfaceText: "#e6efe0"
    readonly property color cPrimary:     "#7cae5a"
    readonly property color cOutline:     "#3a4a34"

    // The only inter-process seam. Named "shrek-menu"; addressed via `qs -p <this file> ipc call shrek-menu ...`.
    IpcHandler {
        target: "shrek-menu"
        function toggle(): void { win.visible = !win.visible }
        function show():   void { win.visible = true }
        function hide():   void { win.visible = false }
    }

    PanelWindow {
        id: win
        visible: false                 // hidden until the IPC toggle — the desktop shows nothing at boot
        color: "transparent"           // no opaque flash before the card paints

        // No edge anchors -> wlr-layer-shell centers the surface; sized to its content.
        implicitWidth: 460
        implicitHeight: 300

        WlrLayershell.layer: WlrLayer.Overlay                    // above fullscreen windows
        WlrLayershell.keyboardFocus: WlrKeyboardFocus.Exclusive  // modal keyboard grab
        WlrLayershell.namespace: "shrek-menu"                    // surface id for sway/wlroots

        // Grab focus each time it opens so Escape lands here immediately.
        onVisibleChanged: if (visible) card.forceActiveFocus()

        Rectangle {
            id: card
            anchors.fill: parent
            focus: true
            color: root.cSurface
            radius: 16
            border.width: 1
            border.color: root.cOutline

            Keys.onEscapePressed: win.visible = false

            Column {
                anchors.centerIn: parent
                spacing: 10
                Text {
                    // Plain text, no emoji: the image has no colour-emoji font (renders a tofu box). The
                    // real menu uses Material Symbol icon names, which DMS's icon font renders natively.
                    anchors.horizontalCenter: parent.horizontalCenter
                    text: "shrek-menu"
                    color: root.cPrimary
                    font.pixelSize: 26
                    font.bold: true
                }
                Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    text: "spike surface — standalone qs, IPC-toggled"
                    color: root.cSurfaceText
                    font.pixelSize: 13
                }
                Text {
                    anchors.horizontalCenter: parent.horizontalCenter
                    text: "Esc to close"
                    color: root.cSurfaceText
                    opacity: 0.6
                    font.pixelSize: 12
                }
            }
        }
    }
}
