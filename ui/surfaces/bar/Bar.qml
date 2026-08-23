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

    Rectangle {
        id: body
        anchors {
            left: parent.left; top: parent.top; bottom: parent.bottom
            leftMargin: Tokens.spaceSm; topMargin: Tokens.spaceSm; bottomMargin: Tokens.spaceSm
        }
        width: Tokens.railWidth
        radius: Tokens.radiusLg
        color: Tokens.barBg
        border.color: Tokens.border
        border.width: 1

        // top cluster — identity/actions + theme toggle + workspaces
        Column {
            anchors { top: parent.top; horizontalCenter: parent.horizontalCenter; topMargin: Tokens.spaceMd }
            spacing: Tokens.spaceMd
            BarActions { anchors.horizontalCenter: parent.horizontalCenter }
            ThemeToggle { anchors.horizontalCenter: parent.horizontalCenter }
            Workspaces { anchors.horizontalCenter: parent.horizontalCenter }
        }

        // bottom cluster — system glances + tray + work + clock + power. Tray hides itself when empty, so
        // the common case is status + work + clock + power (no gap reserved).
        Column {
            anchors { bottom: parent.bottom; horizontalCenter: parent.horizontalCenter; bottomMargin: Tokens.spaceMd }
            spacing: Tokens.spaceMd
            StatusCluster { anchors.horizontalCenter: parent.horizontalCenter }
            TrayCluster { anchors.horizontalCenter: parent.horizontalCenter; window: bar }
            WorkPill { anchors.horizontalCenter: parent.horizontalCenter; session: bar.session }
            Clock { anchors.horizontalCenter: parent.horizontalCenter }
            PowerButton { anchors.horizontalCenter: parent.horizontalCenter }
        }
    }
}
