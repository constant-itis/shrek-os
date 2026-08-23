// SPDX-License-Identifier: GPL-3.0-only
//
// Shrek Shell derivative port of Caelestia Shell's modules/drawers/ContentWindow.qml.
// Upstream Caelestia Shell is GPLv3; the vendored license and attribution live
// under third_party/caelestia-shell. This port preserves the structural shell
// machinery: one content window owns the inset frame, Blob panel union geometry,
// panel deformation, dynamic input regions, bar hover routing, and edge triggers.

import QtQuick
import QtQuick.Shapes
import Quickshell
import Quickshell.Wayland
import Caelestia.Blobs
import "../themes"
import "../state"

PanelWindow {
    id: root

    property var activeScreen
    property var session

    readonly property bool activeOutput: activeScreen === screen
    readonly property bool modalOpen: activeOutput && (ShellState.launcherOpen || ShellState.dashboardOpen)
    readonly property bool sideOpen: activeOutput && (ShellState.workOpen || ShellState.systemOpen)
    readonly property bool hasFullscreen: false
    readonly property real fsTransitionProg: hasFullscreen ? 1 : 0
    readonly property real sdfBorderOffset: 2 * fsTransitionProg
    readonly property real borderThickness: Tokens.spaceMd * (1 - fsTransitionProg)
    readonly property real borderLayoutThickness: hasFullscreen ? 0 : Tokens.spaceMd
    readonly property real borderRounding: 22 * (1 - fsTransitionProg)
    readonly property real dragMaskPadding: 0
    readonly property color surfaceColour: Tokens.panelBg
    readonly property color shadowColour: Qt.rgba(0, 0, 0, 0.26)

    WlrLayershell.exclusionMode: ExclusionMode.Ignore
    WlrLayershell.layer: WlrLayer.Top
    WlrLayershell.keyboardFocus: modalOpen ? WlrKeyboardFocus.OnDemand : WlrKeyboardFocus.None

    anchors.top: true
    anchors.bottom: true
    anchors.left: true
    anchors.right: true

    color: "transparent"
    exclusiveZone: 0
    mask: regions

    Region {
        id: emptyRegion
    }

    Regions {
        id: regions

        bar: bar
        panels: panels
        win: root
    }

    Rectangle {
        anchors.fill: parent
        visible: modalOpen
        color: Tokens.scrim
        opacity: modalOpen ? 0.22 : 0

        Behavior on opacity { NumberAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }
    }

    BlobGroup {
        id: blobGroup

        color: root.surfaceColour
        smoothing: 28
        cornerFill: true
    }

    Item {
        id: materialLayer
        anchors.fill: parent
        opacity: root.surfaceColour.a

        BlobInvertedRect {
            anchors.fill: parent
            anchors.margins: -50
            group: blobGroup
            radius: root.borderRounding
            borderLeft: bar.implicitWidth - anchors.margins - root.sdfBorderOffset
            borderRight: root.borderThickness - anchors.margins - root.sdfBorderOffset
            borderTop: root.borderThickness - anchors.margins - root.sdfBorderOffset
            borderBottom: root.borderThickness - anchors.margins - root.sdfBorderOffset
        }

        PanelBg {
            id: dashBg
            panel: panels.dashboard
            deformAmount: 0.1
        }

        PanelBg {
            id: launcherBg
            panel: panels.launcher
            deformAmount: 0.1
        }

        PanelBg {
            id: sideBg
            panel: panels.sidePanel
            deformAmount: 0.12
        }

        PanelBg {
            id: edgeDockBg
            panel: panels.edgeDock
            deformAmount: 0.18
        }

        PanelBg {
            id: popoutBg
            property real extraWidth: panels.popoutsWrapper.offsetScale < 1 ? 0.18 : 0

            panel: panels.popoutsWrapper
            deformAmount: 0.14
            x: panels.popoutsWrapper.x + bar.implicitWidth - panels.popoutsWrapper.width * extraWidth
            width: panels.popoutsWrapper.width * (1 + extraWidth)

            Behavior on extraWidth { NumberAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }
        }

    }

    Shape {
        anchors.fill: parent

        ShapePath {
            fillColor: "transparent"
            strokeColor: Tokens.border
            strokeWidth: 1
            startX: bar.implicitWidth + root.borderRounding
            startY: root.borderLayoutThickness
            PathLine { x: root.width - root.borderLayoutThickness - root.borderRounding; y: root.borderLayoutThickness }
            PathArc { x: root.width - root.borderLayoutThickness; y: root.borderLayoutThickness + root.borderRounding; radiusX: root.borderRounding; radiusY: root.borderRounding; direction: PathArc.Clockwise }
            PathLine { x: root.width - root.borderLayoutThickness; y: root.height - root.borderLayoutThickness - root.borderRounding }
            PathArc { x: root.width - root.borderLayoutThickness - root.borderRounding; y: root.height - root.borderLayoutThickness; radiusX: root.borderRounding; radiusY: root.borderRounding; direction: PathArc.Clockwise }
            PathLine { x: bar.implicitWidth + root.borderRounding; y: root.height - root.borderLayoutThickness }
            PathArc { x: bar.implicitWidth; y: root.height - root.borderLayoutThickness - root.borderRounding; radiusX: root.borderRounding; radiusY: root.borderRounding; direction: PathArc.Clockwise }
            PathLine { x: bar.implicitWidth; y: root.borderLayoutThickness + root.borderRounding }
            PathArc { x: bar.implicitWidth + root.borderRounding; y: root.borderLayoutThickness; radiusX: root.borderRounding; radiusY: root.borderRounding; direction: PathArc.Clockwise }
        }
    }

    Interactions {
        id: interactions

        screen: root.screen
        popouts: panels.popoutsWrapper
        panels: panels
        bar: bar
        borderThickness: root.borderLayoutThickness
        fullscreen: root.hasFullscreen
        focus: true
    }

    Panels {
        id: panels

        screen: root.screen
        session: root.session
        active: root.activeOutput
        bar: bar
        borderThickness: root.borderLayoutThickness

        dashboard.transform: Matrix4x4 { matrix: dashBg.deformMatrix }
        launcher.transform: Matrix4x4 { matrix: launcherBg.deformMatrix }
        sidePanel.transform: Matrix4x4 { matrix: sideBg.deformMatrix }
        edgeDock.transform: Matrix4x4 { matrix: edgeDockBg.deformMatrix }
        popoutsWrapper.transform: Matrix4x4 { matrix: popoutBg.deformMatrix }
    }

    BarWrapper {
        id: bar

        anchors.top: parent.top
        anchors.bottom: parent.bottom

        session: root.session
        win: root
    }

    component PanelBg: BlobRect {
        required property Item panel
        property real deformAmount: 0.15

        group: blobGroup
        x: panel.x + bar.implicitWidth - root.borderThickness
        y: panel.y
        width: panel.width + root.borderThickness * 2
        height: panel.height + root.borderThickness * 2
        radius: Tokens.radiusLg
        deformScale: deformAmount / 1000
    }
}
