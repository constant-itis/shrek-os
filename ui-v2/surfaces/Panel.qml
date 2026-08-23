import Quickshell
import Quickshell.Wayland
import QtQuick
import "../components"
import "../config"
import "../state"
import "../theme"
import "system"

// Panel — the Shrek System center. Edge/size behavior follows Config; ordinary desktop state only.
PanelWindow {
    id: panel

    readonly property bool vertical: Config.edgeVertical
    readonly property int targetWidth: UI.panelMode === "control" ? 620 : 640
    readonly property int targetHeight: UI.panelMode === "control" ? (UI.controlSection === "network" ? 660 : 500) : 560
    readonly property int openWidth: vertical ? Config.railWidth + Config.gap + targetWidth : 1
    readonly property int openHeight: vertical ? 1 : Config.railWidth + Config.gap + targetHeight

    anchors {
        left: Config.panelEdge === "left" || !panel.vertical
        right: Config.panelEdge === "right" || !panel.vertical
        top: Config.panelEdge === "top" || panel.vertical
        bottom: Config.panelEdge === "bottom" || panel.vertical
    }

    implicitWidth: openWidth
    implicitHeight: openHeight
    exclusionMode: ExclusionMode.Ignore
    color: "transparent"
    mask: Region { item: card }

    Component.onCompleted: if (this.WlrLayershell != null) this.WlrLayershell.layer = WlrLayer.Top

    ShrekPanel {
        id: card
        x: panel.vertical ? (Config.panelEdge === "left" ? Config.railWidth + Config.gap : parent.width - width - Config.railWidth - Config.gap) : Math.round((parent.width - width) / 2)
        y: panel.vertical ? Config.gap : (Config.panelEdge === "top" ? Config.railWidth + Config.gap : parent.height - height - Config.railWidth - Config.gap)
        width: panel.vertical ? (UI.panelOpen ? Math.min(panel.targetWidth, parent.width - Config.railWidth - Config.gap * 2) : 0) : Math.min(panel.targetWidth, parent.width - Config.gap * 2)
        height: panel.vertical ? Math.min(panel.targetHeight, parent.height - Config.gap * 2) : (UI.panelOpen ? Math.min(panel.targetHeight, parent.height - Config.railWidth - Config.gap * 3) : 0)
        opacity: UI.panelOpen ? 1 : 0

        Behavior on width   { NumberAnimation { duration: Config.animMs; easing.type: Easing.OutCubic } }
        Behavior on height  { NumberAnimation { duration: Config.animMs; easing.type: Easing.OutCubic } }
        Behavior on opacity { NumberAnimation { duration: Config.animMs } }

        Loader {
            anchors.fill: parent
            visible: UI.panelOpen
            sourceComponent: UI.panelMode === "settings" ? settingsSurface : controlSurface
        }

        Component { id: controlSurface; ControlCenter {} }
        Component { id: settingsSurface; SystemCenter {} }
    }
}
