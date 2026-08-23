import Quickshell
import Quickshell.Wayland
import QtQuick
import QtQuick.Shapes
import "../../themes"
import "../../state"

// ScreenFrame — the per-monitor shell geometry owner. It draws the continuous desktop frame, the left
// rail material, and the right drawer socket in one click-through layer. Current content surfaces stay
// in their own PanelWindows, but their visible background material starts here so the shell reads as one
// inset frame instead of unrelated rounded rectangles.
//
// NOTE: this rounds the SCREEN corners, not individual window corners (per-window rounding is a compositor
// feature Sway lacks).
PanelWindow {
    id: frame
    WlrLayershell.layer: WlrLayer.Top
    anchors { top: true; left: true; right: true; bottom: true }
    color: "transparent"
    exclusiveZone: 0
    mask: Region {}   // empty region → fully click-through, never intercepts the desktop

    property var activeScreen

    readonly property bool activeOutput: activeScreen === screen
    readonly property bool rightDrawerOpen: activeOutput && (ShellState.workOpen || ShellState.systemOpen)
    property real rightDrawerProgress: rightDrawerOpen ? 1 : 0
    readonly property bool rightDrawerVisible: rightDrawerOpen || rightDrawerProgress > 0.001
    readonly property int inset: Tokens.spaceSm
    readonly property int railTotal: Tokens.railWidth + 2 * Tokens.spaceSm
    readonly property real drawerSocketWidth: Tokens.drawerWidth * rightDrawerProgress
    readonly property int rad: 18
    readonly property int socketRad: Tokens.radiusLg
    readonly property color material: Tokens.panelBg
    readonly property color outline: Tokens.border

    Behavior on rightDrawerProgress {
        NumberAnimation { duration: Tokens.animMed; easing.type: Easing.OutCubic }
    }

    Shape {
        anchors.fill: parent

        // Frame ring. The left side of the desktop hole starts AFTER the rail, matching the Sway layer
        // exclusive zone + outer gap. That is the key architectural change: the rail is part of the
        // shell frame, not a floating card over the desktop.
        ShapePath {
            fillColor: frame.material
            strokeWidth: 0
            fillRule: ShapePath.OddEvenFill

            // outer contour: the whole screen
            startX: 0; startY: 0
            PathLine { x: frame.width; y: 0 }
            PathLine { x: frame.width; y: frame.height }
            PathLine { x: 0; y: frame.height }
            PathLine { x: 0; y: 0 }

            // inner contour: desktop/work-area hole
            PathMove { x: frame.railTotal + frame.rad; y: frame.inset }
            PathLine { x: frame.width - frame.inset - frame.rad; y: frame.inset }
            PathArc { x: frame.width - frame.inset; y: frame.inset + frame.rad; radiusX: frame.rad; radiusY: frame.rad; direction: PathArc.Clockwise }
            PathLine { x: frame.width - frame.inset; y: frame.height - frame.inset - frame.rad }
            PathArc { x: frame.width - frame.inset - frame.rad; y: frame.height - frame.inset; radiusX: frame.rad; radiusY: frame.rad; direction: PathArc.Clockwise }
            PathLine { x: frame.railTotal + frame.rad; y: frame.height - frame.inset }
            PathArc { x: frame.railTotal; y: frame.height - frame.inset - frame.rad; radiusX: frame.rad; radiusY: frame.rad; direction: PathArc.Clockwise }
            PathLine { x: frame.railTotal; y: frame.inset + frame.rad }
            PathArc { x: frame.railTotal + frame.rad; y: frame.inset; radiusX: frame.rad; radiusY: frame.rad; direction: PathArc.Clockwise }
        }

        // A subtle inner edge keeps the work-area boundary legible without making the rail a separate card.
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

        // Right-side socket for Work/System. It is drawn by the frame owner and overlaps the right border,
        // so the drawer content appears to grow out of the frame rather than sitting on a second rectangle.
        ShapePath {
            fillColor: frame.rightDrawerVisible ? frame.material : "transparent"
            strokeColor: frame.rightDrawerVisible ? frame.outline : "transparent"
            strokeWidth: frame.rightDrawerVisible ? 1 : 0
            startX: frame.width; startY: frame.inset
            PathLine { x: frame.width - frame.drawerSocketWidth + frame.socketRad; y: frame.inset }
            PathArc { x: frame.width - frame.drawerSocketWidth; y: frame.inset + frame.socketRad; radiusX: frame.socketRad; radiusY: frame.socketRad; direction: PathArc.Counterclockwise }
            PathLine { x: frame.width - frame.drawerSocketWidth; y: frame.height - frame.inset - frame.socketRad }
            PathArc { x: frame.width - frame.drawerSocketWidth + frame.socketRad; y: frame.height - frame.inset; radiusX: frame.socketRad; radiusY: frame.socketRad; direction: PathArc.Counterclockwise }
            PathLine { x: frame.width; y: frame.height - frame.inset }
            PathLine { x: frame.width; y: frame.inset }
        }
    }
}
