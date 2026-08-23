import Quickshell
import Quickshell.Wayland
import QtQuick
import QtQuick.Shapes
import "../../themes"

// ScreenFrame — the rounded desktop frame. A full-screen, CLICK-THROUGH overlay (empty input mask) that
// draws a dark inset border with rounded inner corners, so the whole desktop reads as one soft rounded
// panel. `gaps outer` (sway.config) insets windows to match the frame's inner edge; the frame's rounded
// corners overlay the screen corners to round them. Purely decorative — never steals input, mounts under
// the shell surfaces (created first in Shell.qml) and above windows (layer Top). One per screen.
//
// NOTE: this rounds the SCREEN corners, not individual window corners (per-window rounding is a compositor
// feature Sway lacks). Colour is Tokens.bg so the frame stays theme-coherent.
PanelWindow {
    id: frame
    WlrLayershell.layer: WlrLayer.Top
    anchors { top: true; left: true; right: true; bottom: true }
    color: "transparent"
    exclusiveZone: 0
    mask: Region {}   // empty region → fully click-through, never intercepts the desktop

    readonly property int inset: Tokens.spaceSm    // matches `gaps outer` in sway.config
    readonly property int rad: 16                  // screen-corner radius

    Shape {
        anchors.fill: parent
        // frame ring = area between the full-screen outer rect and an inner rounded rect (even-odd fill)
        ShapePath {
            fillColor: Tokens.bg
            strokeWidth: 0
            fillRule: ShapePath.OddEvenFill

            // outer contour: the whole screen
            startX: 0; startY: 0
            PathLine { x: frame.width; y: 0 }
            PathLine { x: frame.width; y: frame.height }
            PathLine { x: 0; y: frame.height }
            PathLine { x: 0; y: 0 }

            // inner contour: rounded rect hole (traced clockwise), inset by `inset`, corners radius `rad`
            PathMove { x: frame.inset + frame.rad; y: frame.inset }
            PathLine { x: frame.width - frame.inset - frame.rad; y: frame.inset }
            PathArc { x: frame.width - frame.inset; y: frame.inset + frame.rad; radiusX: frame.rad; radiusY: frame.rad; direction: PathArc.Clockwise }
            PathLine { x: frame.width - frame.inset; y: frame.height - frame.inset - frame.rad }
            PathArc { x: frame.width - frame.inset - frame.rad; y: frame.height - frame.inset; radiusX: frame.rad; radiusY: frame.rad; direction: PathArc.Clockwise }
            PathLine { x: frame.inset + frame.rad; y: frame.height - frame.inset }
            PathArc { x: frame.inset; y: frame.height - frame.inset - frame.rad; radiusX: frame.rad; radiusY: frame.rad; direction: PathArc.Clockwise }
            PathLine { x: frame.inset; y: frame.inset + frame.rad }
            PathArc { x: frame.inset + frame.rad; y: frame.inset; radiusX: frame.rad; radiusY: frame.rad; direction: PathArc.Clockwise }
        }
    }
}
