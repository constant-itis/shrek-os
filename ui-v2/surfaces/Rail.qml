import Quickshell
import Quickshell.Wayland
import QtQuick
import "../config"
import "../state"
import "../theme"
import "../services"

// Rail — one per screen. A left-anchored vertical bar that RESERVES its width: exactly three connected
// anchors (left+top+bottom) trigger ExclusionMode.Auto, which reserves precisely implicitWidth. Fully
// interactive (default mask = whole window clickable).
//
// Modules (top→bottom): panel toggle, Work toggle (hero surface), then LIVE Sway workspace pips driven by
// CompositorService — each shows its workspace number, highlights the focused one, and dispatches a
// workspace switch on click. A window-count badge at the bottom reflects ToplevelManager state. This is
// the shell's first real compositor-connected surface.
PanelWindow {
    id: rail
    anchors { left: true; top: true; bottom: true }
    implicitWidth: Config.railWidth
    color: Tokens.surface
    property string clockText: ""

    Component.onCompleted: if (this.WlrLayershell != null) this.WlrLayershell.layer = WlrLayer.Top

    Timer {
        interval: 15000
        running: true
        repeat: true
        triggeredOnStart: true
        onTriggered: {
            var d = new Date()
            rail.clockText = Qt.formatTime(d, "h:mm")
        }
    }

    Column {
        anchors.top: parent.top
        anchors.topMargin: Config.gap
        anchors.horizontalCenter: parent.horizontalCenter
        spacing: Config.gap

        // Panel trigger — click toggles; hover highlights.
        Rectangle {
            width: 36; height: 36; radius: Tokens.radius
            color: (trig.containsMouse || UI.panelOpen) ? Tokens.accent : Tokens.surfaceRaised
            Behavior on color { ColorAnimation { duration: Config.animMs } }
            Text {
                anchors.centerIn: parent
                text: "≡"   // ≡
                font.pixelSize: 20
                color: (trig.containsMouse || UI.panelOpen) ? Tokens.accentText : Tokens.textPrimary
            }
            MouseArea { id: trig; anchors.fill: parent; hoverEnabled: true; onClicked: UI.togglePanel() }
        }

        // Work trigger — the hero surface (effective-authority view of live agent sessions).
        Rectangle {
            width: 36; height: 36; radius: Tokens.radius
            color: (wtrig.containsMouse || UI.workOpen) ? Tokens.work : Tokens.surfaceRaised
            Behavior on color { ColorAnimation { duration: Config.animMs } }
            Text {
                anchors.centerIn: parent
                text: "◪"   // ◪ — quadrant square, "isolated work"
                font.pixelSize: 18
                color: (wtrig.containsMouse || UI.workOpen) ? Tokens.accentText : Tokens.textPrimary
            }
            MouseArea { id: wtrig; anchors.fill: parent; hoverEnabled: true; onClicked: UI.toggleWork() }
        }

        // Divider.
        Rectangle { width: 24; height: 1; color: Tokens.outline; anchors.horizontalCenter: parent.horizontalCenter }

        // Live Sway workspace pips.
        Repeater {
            model: CompositorService.workspaces
            Rectangle {
                required property var modelData
                readonly property bool here: modelData.focused || modelData.active
                width: 36; height: 36; radius: Tokens.radius
                color: here ? Tokens.accentDim : (wsm.containsMouse ? Tokens.surfaceRaised : Tokens.surface)
                border.width: modelData.urgent ? 2 : 0
                border.color: Tokens.attention
                Behavior on color { ColorAnimation { duration: Config.animMs } }
                Text {
                    anchors.centerIn: parent
                    text: ("" + (modelData.number > 0 ? modelData.number : modelData.name))
                    font.family: Tokens.fontMono
                    font.pixelSize: Tokens.fontBody
                    font.bold: parent.here
                    color: parent.here ? Tokens.accentText : Tokens.textSecondary
                }
                MouseArea {
                    id: wsm
                    anchors.fill: parent
                    hoverEnabled: true
                    onClicked: CompositorService.focusWorkspace(modelData.number)
                }
            }
        }
    }

    // Compact daily-driver status, pinned to the rail bottom.
    Column {
        anchors.bottom: parent.bottom
        anchors.bottomMargin: Config.gap
        anchors.horizontalCenter: parent.horizontalCenter
        spacing: Tokens.spaceXs

        component StatusCell: Rectangle {
            property string label: ""
            property color tint: Tokens.textSecondary
            property var action
            width: 36; height: 28; radius: Tokens.radiusSm
            color: hover.containsMouse ? Tokens.surfaceRaised : Tokens.surface
            border.width: 1
            border.color: hover.containsMouse ? Tokens.outlineStrong : "transparent"
            Text {
                anchors.centerIn: parent
                text: parent.label
                color: parent.tint
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontCaption
                font.bold: parent.tint === Tokens.accent
            }
            MouseArea {
                id: hover
                anchors.fill: parent
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor
                onClicked: if (parent.action) parent.action()
            }
        }

        StatusCell {
            label: Network.online ? "net" : "--"
            tint: Network.online ? Tokens.accent : Tokens.muted
            action: function () { UI.openSystem("network") }
        }
        StatusCell {
            visible: Audio.ready
            label: Audio.muted ? "mut" : Math.round(Audio.volume * 100)
            tint: Audio.muted ? Tokens.warning : Tokens.textSecondary
            action: function () { UI.openSystem("audio") }
        }
        StatusCell {
            visible: Bluetooth.available
            label: "bt"
            tint: Bluetooth.enabled ? Tokens.textSecondary : Tokens.muted
            action: function () { UI.openSystem("bluetooth") }
        }
        StatusCell {
            visible: Power.present
            label: "" + Math.round(Power.percentage)
            tint: Power.onBattery ? Tokens.warning : Tokens.textSecondary
            action: function () { UI.openSystem("power") }
        }
        StatusCell {
            visible: CompositorService.windowCount > 0
            label: "" + CompositorService.windowCount
            tint: Tokens.textSecondary
            action: function () { UI.openSystem("system") }
        }
        StatusCell {
            label: rail.clockText
            tint: Tokens.textSecondary
            action: function () { UI.openSystem("overview") }
        }
    }
}
