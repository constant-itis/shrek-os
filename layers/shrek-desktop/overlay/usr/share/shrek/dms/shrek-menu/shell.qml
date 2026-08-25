// shrek-menu — shrek-owned standalone command surface (docs/menu-engine.md; mycelium menu-engine GO decision).
//
// A SEPARATE `qs` process, NOT a DMS plugin and NOT spliced into DMS's tree: DMS's plugin schema has no
// surface type and plugins can't make a top-level PanelWindow, and quickshell IPC target names are scoped
// to one instance (`dms ipc` is hard-pinned to DMS's own PID/config — verified from quickshell
// command.cpp::selectInstance + DankMaterialShell shell.go). So:
//
//   launch:  exec_always qs -p /usr/share/shrek/dms/shrek-menu/shell.qml   (from sway.config)
//   toggle:  qs -p /usr/share/shrek/dms/shrek-menu/shell.qml ipc call shrek-menu toggle   ($mod+slash)
//
// `dms ipc call shrek-menu` will NOT reach this process — the toggle MUST go through `qs -p <this file>`.
import QtQuick
import Quickshell
import Quickshell.Io
import Quickshell.Wayland

ShellRoot {
    id: root

    Component.onCompleted: console.info("SHREK-MENU surface loaded (hidden until toggle)")

    // --- Live theme parity from DMS matugen output ------------------------------------------------
    // DMS rewrites ~/.cache/DankMaterialShell/dms-colors.json on every wallpaper/theme change. We can't
    // `import qs.Common` DMS's Theme singleton — a second qs process is blackholed by quickshell's
    // per-instance qs:// URL interceptor — so this surface reads the same file itself. The write is atomic
    // (matugen stages a .tmp then renames), so a watched read never sees a partial file. Schema verified
    // live against dms=1.5.3db1 (mycelium #2819): { colors: { dark|light: { <M3 key>: "#rrggbb" } }, dank16 }.
    // Any missing/invalid file falls back to the baked swamp palette below, so the menu always themes
    // safely — even on first boot before matugen has ever run.
    readonly property string colorMode: "dark"   // shipped image is dark; DMS defaults to dark too.

    property var dmsColors: ({})
    function reloadColors() {
        try { root.dmsColors = JSON.parse(colorsFile.text() || "{}") || ({}); }
        catch (e) { root.dmsColors = ({}); }
    }
    // Look up one semantic key in colors[colorMode]; fall back when the file/key is absent or malformed.
    function themed(key, fallback) {
        var mode = root.dmsColors && root.dmsColors.colors ? root.dmsColors.colors[root.colorMode] : null;
        var v = mode ? mode[key] : undefined;
        return (typeof v === "string" && v.length > 0) ? v : fallback;
    }

    // Watcher for the live palette. Mirrors DMS's own dynamicColorsFileView (Common/Theme.qml): a
    // change fires onFileChanged -> reload() -> onLoaded -> re-parse. onLoaded (NOT onLoadedChanged) is
    // load-safe here: `loaded` transitions false->true only on the FIRST load, so onLoadedChanged would
    // never re-fire on subsequent reloads and the card would freeze at the first palette; onLoaded fires
    // on every completed (re)load, so the surface recolors each time matugen rewrites the file.
    FileView {
        id: colorsFile
        path: (Quickshell.env("HOME") || "") + "/.cache/DankMaterialShell/dms-colors.json"
        blockLoading: false
        watchChanges: true
        printErrors: false
        onFileChanged: reload()
        onLoaded: root.reloadColors()
    }

    // Surface tokens: live DMS value first, baked swamp-green (DMS default palette) as the fallback.
    // These are bindings, so they re-resolve the moment dmsColors is reassigned — the card recolors live.
    readonly property color cSurface:     themed("surface_container_high", "#1b2a1a")  // elevated overlay card
    readonly property color cSurfaceText: themed("on_surface",             "#e6efe0")
    readonly property color cPrimary:     themed("primary",                "#7cae5a")
    readonly property color cOutline:     themed("outline_variant",        "#3a4a34")

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
                    text: "themed live from dms-colors.json"
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
