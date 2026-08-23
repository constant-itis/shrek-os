import Quickshell
import Quickshell.Wayland
import QtQuick
import "../../themes"

// ShellExclusions — invisible layer-shell reservations for the Blob frame.
//
// Caelestia keeps the full-screen visual content window click-through and uses separate exclusion windows
// to tell the compositor where tiled clients may not live. Shrek mirrors that split: ScreenFrame owns the
// material/input rail; this file owns only work-area reservation and draws nothing.
Scope {
    id: root

    required property var screen

    readonly property int inset: Tokens.spaceSm
    readonly property int railTotal: Tokens.railWidth + 2 * Tokens.spaceSm

    component ExclusionZone: PanelWindow {
        WlrLayershell.layer: WlrLayer.Top
        color: "transparent"
        mask: Region {}
        implicitWidth: 1
        implicitHeight: 1
    }

    ExclusionZone {
        screen: root.screen
        anchors.left: true
        exclusiveZone: root.railTotal
    }

    ExclusionZone {
        screen: root.screen
        anchors.top: true
        exclusiveZone: root.inset
    }

    ExclusionZone {
        screen: root.screen
        anchors.right: true
        exclusiveZone: root.inset
    }

    ExclusionZone {
        screen: root.screen
        anchors.bottom: true
        exclusiveZone: root.inset
    }
}
