import Quickshell
import Quickshell.Wayland
import QtQuick
import "../../themes"

// Bar — the always-present left SPINE (Desktop Slice 1, vertical-rail pass). A FLOATING, rounded, elevated
// rail inset from the screen edges over the wallpaper. Quickshell owns this surface (Sway draws no bar).
// Top cluster: apps/terminal actions + live Sway workspaces. Bottom cluster: status glances + tray + Work
// pill + stacked clock + power. The horizontal taskbar (WindowList) and media pill do NOT live in the rail
// — window nav is workspaces + Super+Tab, media moves to the dashboard. Sub-components live in this
// directory (vertical variants) and are auto-visible by name.
PanelWindow {
    id: bar
    WlrLayershell.layer: WlrLayer.Top
    anchors { top: true; left: true; bottom: true }
    implicitWidth: Tokens.railWidth + 2 * Tokens.spaceSm
    exclusiveZone: Tokens.railWidth + Tokens.spaceSm
    color: "transparent"

    // read-only session view injected from Shell.qml (drives the Work affordance count)
    property var session

    Rail {
        anchors {
            left: parent.left; top: parent.top; bottom: parent.bottom
            leftMargin: Tokens.spaceSm; topMargin: Tokens.spaceSm; bottomMargin: Tokens.spaceSm
        }
        width: Tokens.railWidth
        session: bar.session
        window: bar
    }
}
