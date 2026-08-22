import Quickshell
import Quickshell.Wayland
import QtQuick
import "../../themes"

// Bar — the always-present top spine (Desktop Slice 1). A FLOATING, rounded, elevated bar inset from the
// screen edges over the wallpaper. Quickshell owns this surface (Sway draws no bar). Left: apps/terminal
// actions + live Sway workspaces. Center: focused-window title. Right: status cluster + Work pill + clock.
// Sub-components live in this directory and are auto-visible by name.
PanelWindow {
    id: bar
    WlrLayershell.layer: WlrLayer.Top
    anchors { top: true; left: true; right: true }
    implicitHeight: Tokens.barHeight + 2 * Tokens.spaceSm
    exclusiveZone: Tokens.barHeight + Tokens.spaceSm
    color: "transparent"

    // read-only session view injected from Shell.qml (drives the Work affordance count)
    property var session

    Rectangle {
        id: body
        anchors {
            left: parent.left; right: parent.right; top: parent.top
            leftMargin: Tokens.spaceMd; rightMargin: Tokens.spaceMd; topMargin: Tokens.spaceSm
        }
        height: Tokens.barHeight
        radius: Tokens.radiusLg
        color: Tokens.barBg
        border.color: Tokens.border
        border.width: 1

        // left module
        Row {
            anchors { left: parent.left; verticalCenter: parent.verticalCenter; leftMargin: Tokens.spaceMd }
            spacing: Tokens.spaceMd
            BarActions { anchors.verticalCenter: parent.verticalCenter }
            Workspaces { anchors.verticalCenter: parent.verticalCenter }
        }

        // centre: live window list (taskbar) — click to focus, middle-click to close
        WindowList {
            anchors.centerIn: parent
            width: Math.min(implicitWidth, body.width * 0.46)
            clip: true
        }

        // right module
        Row {
            anchors { right: parent.right; verticalCenter: parent.verticalCenter; rightMargin: Tokens.spaceMd }
            spacing: Tokens.spaceLg
            StatusCluster { anchors.verticalCenter: parent.verticalCenter }
            WorkPill { anchors.verticalCenter: parent.verticalCenter; session: bar.session }
            Clock { anchors.verticalCenter: parent.verticalCenter }
            PowerButton { anchors.verticalCenter: parent.verticalCenter }
        }
    }
}
