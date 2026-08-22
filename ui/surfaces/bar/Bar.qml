import Quickshell
import Quickshell.Wayland
import QtQuick
import "../../themes"

// Bar — the always-present top spine (Desktop Slice 1). Quickshell owns this surface (Sway draws no
// bar). Left: live Sway workspaces. Center: focused-window title. Right: Work affordance + clock. The
// status cluster (audio/net/bt) lands with SYSTEM in a later phase. Sub-components live in this same
// directory and are auto-visible by name (no import needed).
PanelWindow {
    id: bar
    WlrLayershell.layer: WlrLayer.Top
    anchors { top: true; left: true; right: true }
    implicitHeight: Tokens.barHeight
    exclusiveZone: Tokens.barHeight
    color: Tokens.surface

    // read-only session view injected from Shell.qml (drives the Work affordance count)
    property var session

    // hairline under the bar
    Rectangle {
        anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
        height: 1
        color: Tokens.border
    }

    Workspaces {
        anchors { left: parent.left; verticalCenter: parent.verticalCenter; leftMargin: Tokens.spaceMd }
    }

    WindowTitle {
        anchors.centerIn: parent
        width: Math.min(implicitWidth, bar.width * 0.4)
    }

    Row {
        anchors { right: parent.right; verticalCenter: parent.verticalCenter; rightMargin: Tokens.spaceMd }
        spacing: Tokens.spaceLg
        StatusCluster { anchors.verticalCenter: parent.verticalCenter }
        WorkPill { anchors.verticalCenter: parent.verticalCenter; session: bar.session }
        Clock { anchors.verticalCenter: parent.verticalCenter }
    }
}
