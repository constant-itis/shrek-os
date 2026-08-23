import QtQuick
import QtQuick.Shapes
import "../themes"

// Clean-room QML geometry primitive for panels attached to a screen edge.
// It models the geometry role only; a native SDF renderer can later keep this API.
Shape {
    id: root
    property string edge: "bottom" // top, bottom, left, right
    property real radius: Tokens.radiusLg
    property color fill: Tokens.panelBg
    property color stroke: Tokens.border

    readonly property real r: Math.max(0, Math.min(radius, width / 2, height / 2))

    ShapePath {
        fillColor: root.edge === "bottom" ? root.fill : "transparent"
        strokeColor: root.edge === "bottom" ? root.stroke : "transparent"
        strokeWidth: root.edge === "bottom" ? 1 : 0
        startX: root.r
        startY: 0

        PathLine { x: root.width - root.r; y: 0 }
        PathArc { x: root.width; y: root.r; radiusX: root.r; radiusY: root.r; direction: PathArc.Clockwise }
        PathLine { x: root.width; y: root.height }
        PathLine { x: 0; y: root.height }
        PathLine { x: 0; y: root.r }
        PathArc { x: root.r; y: 0; radiusX: root.r; radiusY: root.r; direction: PathArc.Clockwise }
    }

    ShapePath {
        fillColor: root.edge === "top" ? root.fill : "transparent"
        strokeColor: root.edge === "top" ? root.stroke : "transparent"
        strokeWidth: root.edge === "top" ? 1 : 0
        startX: 0
        startY: 0

        PathLine { x: root.width; y: 0 }
        PathLine { x: root.width; y: root.height - root.r }
        PathArc { x: root.width - root.r; y: root.height; radiusX: root.r; radiusY: root.r; direction: PathArc.Clockwise }
        PathLine { x: root.r; y: root.height }
        PathArc { x: 0; y: root.height - root.r; radiusX: root.r; radiusY: root.r; direction: PathArc.Clockwise }
        PathLine { x: 0; y: 0 }
    }

    ShapePath {
        fillColor: root.edge === "left" ? root.fill : "transparent"
        strokeColor: root.edge === "left" ? root.stroke : "transparent"
        strokeWidth: root.edge === "left" ? 1 : 0
        startX: 0
        startY: 0

        PathLine { x: root.width - root.r; y: 0 }
        PathArc { x: root.width; y: root.r; radiusX: root.r; radiusY: root.r; direction: PathArc.Clockwise }
        PathLine { x: root.width; y: root.height - root.r }
        PathArc { x: root.width - root.r; y: root.height; radiusX: root.r; radiusY: root.r; direction: PathArc.Clockwise }
        PathLine { x: 0; y: root.height }
        PathLine { x: 0; y: 0 }
    }

    ShapePath {
        fillColor: root.edge === "right" ? root.fill : "transparent"
        strokeColor: root.edge === "right" ? root.stroke : "transparent"
        strokeWidth: root.edge === "right" ? 1 : 0
        startX: root.r
        startY: 0

        PathLine { x: root.width; y: 0 }
        PathLine { x: root.width; y: root.height }
        PathLine { x: root.r; y: root.height }
        PathArc { x: 0; y: root.height - root.r; radiusX: root.r; radiusY: root.r; direction: PathArc.Clockwise }
        PathLine { x: 0; y: root.r }
        PathArc { x: root.r; y: 0; radiusX: root.r; radiusY: root.r; direction: PathArc.Clockwise }
    }
}
