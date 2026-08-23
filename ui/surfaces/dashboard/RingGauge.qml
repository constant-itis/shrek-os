import QtQuick
import QtQuick.Shapes
import "../../themes"

// RingGauge — a circular progress ring with a centered readout, for the dashboard Performance tab. A 270°
// arc (gap at the bottom) drawn with QtQuick.Shapes: a full track under a value arc. All colour comes from
// Tokens (semantic contract), so gauges repaint with the active theme. Purely presentational.
Item {
    id: root
    implicitWidth: 128
    implicitHeight: 128

    property real value: 0            // 0..1 (clamped)
    property string primaryText: ""   // big centre readout
    property string labelText: ""     // role under the number
    property string subText: ""       // small secondary line
    property color arcColor: Tokens.accent

    readonly property real _v: Math.max(0, Math.min(1, value))
    readonly property int _thick: 9
    readonly property real _r: (Math.min(width, height) - _thick) / 2 - 1

    Shape {
        anchors.fill: parent
        preferredRendererType: Shape.CurveRenderer

        // track
        ShapePath {
            strokeColor: Tokens.surfaceAlt
            strokeWidth: root._thick
            fillColor: "transparent"
            capStyle: ShapePath.RoundCap
            PathAngleArc {
                centerX: root.width / 2; centerY: root.height / 2
                radiusX: root._r; radiusY: root._r
                startAngle: 135; sweepAngle: 270
            }
        }
        // value
        ShapePath {
            strokeColor: root.arcColor
            strokeWidth: root._thick
            fillColor: "transparent"
            capStyle: ShapePath.RoundCap
            PathAngleArc {
                centerX: root.width / 2; centerY: root.height / 2
                radiusX: root._r; radiusY: root._r
                startAngle: 135; sweepAngle: 270 * root._v
            }
        }
    }

    Column {
        anchors.centerIn: parent
        spacing: 1
        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            text: root.primaryText
            color: Tokens.text
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontHeadline
            font.bold: true
        }
        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            text: root.labelText
            color: Tokens.textDim
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontCaption
        }
        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            visible: root.subText.length > 0
            text: root.subText
            color: Tokens.textFaint
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontCaption
        }
    }
}
