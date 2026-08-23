import Quickshell
import Quickshell.Wayland
import QtQuick
import QtQuick.Shapes
import Caelestia.Blobs
import "../../themes"
import "../../state"
import "../bar"

// ScreenFrame — the per-monitor shell geometry owner. This file is derivative shell-port work built
// against the GPLv3 Caelestia.Blobs API vendored under third_party/caelestia-shell; keep the upstream
// license and Shrek attribution notes with that vendored source.
//
// G1 port slice: render the shell material through one BlobGroup. BlobInvertedRect cuts the live desktop
// hole out of the full-screen material, while attached BlobRects share that group so future drawers and
// popouts deform/merge with the frame instead of reading as unrelated cards. The window remains
// click-through; input ownership stays with the content surfaces and the real applications underneath.
//
// NOTE: this rounds the SCREEN corners, not individual window corners (per-window rounding is a compositor
// feature Sway lacks).
PanelWindow {
    id: frame
    WlrLayershell.layer: WlrLayer.Top
    WlrLayershell.exclusionMode: ExclusionMode.Ignore
    anchors { top: true; left: true; right: true; bottom: true }
    color: "transparent"
    exclusiveZone: 0
    mask: Region { item: railInput }

    property var activeScreen
    property var session

    readonly property bool activeOutput: activeScreen === screen
    readonly property bool mergedPanelOpen: activeOutput && (ShellState.workOpen || ShellState.systemOpen)
    property real mergedPanelProgress: mergedPanelOpen ? 1 : 0
    readonly property bool mergedPanelVisible: mergedPanelOpen || mergedPanelProgress > 0.001
    readonly property int inset: Tokens.spaceSm
    readonly property int railTotal: Tokens.railWidth + 2 * Tokens.spaceSm
    readonly property real mergedPanelWidth: (Tokens.drawerWidth + inset) * mergedPanelProgress
    readonly property int rad: 18
    readonly property int socketRad: Tokens.radiusLg
    readonly property color material: Tokens.panelBg
    readonly property color outline: Tokens.border
    readonly property color shadow: Qt.rgba(0, 0, 0, 0.24)

    Behavior on mergedPanelProgress {
        NumberAnimation { duration: Tokens.animMed; easing.type: Easing.OutCubic }
    }

    BlobGroup {
        id: shellBlobs
        color: frame.material
        smoothing: 28
        cornerFill: true
    }

    BlobGroup {
        id: shadowBlobs
        color: frame.shadow
        smoothing: shellBlobs.smoothing
        cornerFill: true
    }

    Item {
        id: blobLayer
        anchors.fill: parent

        // Slight offset duplicate gives the Blob material depth without introducing separate panels.
        Item {
            anchors.fill: parent
            y: 2
            opacity: 0.7

            BlobInvertedRect {
                anchors.fill: parent
                group: shadowBlobs
                radius: frame.rad
                borderLeft: frame.railTotal
                borderRight: frame.inset
                borderTop: frame.inset
                borderBottom: frame.inset
            }

            BlobRect {
                visible: frame.mergedPanelVisible
                group: shadowBlobs
                x: frame.width - frame.mergedPanelWidth
                y: frame.inset
                width: frame.mergedPanelWidth
                height: frame.height - 2 * frame.inset
                radius: frame.socketRad
                deformScale: 0.00035
            }
        }

        // The actual continuous shell material. The left border is intentionally rail-sized, so the bar is
        // an occupied part of the same shell frame instead of a floating rectangle.
        BlobInvertedRect {
            anchors.fill: parent
            group: shellBlobs
            radius: frame.rad
            borderLeft: frame.railTotal
            borderRight: frame.inset
            borderTop: frame.inset
            borderBottom: frame.inset
        }

        // G2 attached geometry: the Work/System content windows draw no panel background of their own.
        // This animated BlobRect is the panel body, merged into the desktop frame by the shared group.
        BlobRect {
            id: mergedPanel
            visible: frame.mergedPanelVisible
            group: shellBlobs
            x: frame.width - frame.mergedPanelWidth
            y: frame.inset
            width: frame.mergedPanelWidth
            height: frame.height - 2 * frame.inset
            radius: frame.socketRad
            deformScale: 0.00035
        }
    }

    Shape {
        anchors.fill: parent

        // A subtle inner edge keeps the work-area boundary legible. Material and joins are Blob-rendered;
        // this path is only a hairline over the punched-out desktop edge.
        ShapePath {
            fillColor: "transparent"
            strokeColor: frame.outline
            strokeWidth: 1
            startX: frame.railTotal + frame.rad; startY: frame.inset
            PathLine { x: frame.width - frame.inset - frame.rad; y: frame.inset }
            PathArc { x: frame.width - frame.inset; y: frame.inset + frame.rad; radiusX: frame.rad; radiusY: frame.rad; direction: PathArc.Clockwise }
            PathLine { x: frame.width - frame.inset; y: frame.height - frame.inset - frame.rad }
            PathArc { x: frame.width - frame.inset - frame.rad; y: frame.height - frame.inset; radiusX: frame.rad; radiusY: frame.rad; direction: PathArc.Clockwise }
            PathLine { x: frame.railTotal + frame.rad; y: frame.height - frame.inset }
            PathArc { x: frame.railTotal; y: frame.height - frame.inset - frame.rad; radiusX: frame.rad; radiusY: frame.rad; direction: PathArc.Clockwise }
            PathLine { x: frame.railTotal; y: frame.inset + frame.rad }
            PathArc { x: frame.railTotal + frame.rad; y: frame.inset; radiusX: frame.rad; radiusY: frame.rad; direction: PathArc.Clockwise }
        }
    }

    Item {
        id: railInput
        anchors { left: parent.left; top: parent.top; bottom: parent.bottom }
        width: frame.railTotal

        Rail {
            anchors {
                left: parent.left; top: parent.top; bottom: parent.bottom
                leftMargin: frame.inset; topMargin: frame.inset; bottomMargin: frame.inset
            }
            width: Tokens.railWidth
            session: frame.session
            window: frame
        }
    }
}
