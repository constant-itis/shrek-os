import QtQuick
import "../../themes"
import "../../state"

// WorkPill — the bar's Work affordance. A dot (accent when workloads are live) + count, reading the
// injected read-only SessionProvider. Click toggles the Work drawer. Strictly display; no authority.
Item {
    id: root
    property var session
    readonly property int count: session ? session.sessions.length : 0

    implicitWidth: row.implicitWidth
    implicitHeight: Tokens.barHeight

    Row {
        id: row
        anchors.centerIn: parent
        spacing: Tokens.spaceSm

        // live indicator: a calm accent dot; when sandboxed sessions are running it gains a soft
        // pulsing halo so the eye catches active work without noise.
        Item {
            anchors.verticalCenter: parent.verticalCenter
            width: 12; height: 12

            Rectangle {
                id: halo
                anchors.centerIn: parent
                width: 12; height: 12; radius: 6
                color: "transparent"
                border.color: Tokens.accent
                border.width: 2
                visible: root.count > 0
                SequentialAnimation on opacity {
                    running: root.count > 0
                    loops: Animation.Infinite
                    NumberAnimation { from: 0.55; to: 0.0; duration: 1400; easing.type: Easing.OutCubic }
                    PauseAnimation { duration: 200 }
                }
                SequentialAnimation on scale {
                    running: root.count > 0
                    loops: Animation.Infinite
                    NumberAnimation { from: 0.7; to: 1.6; duration: 1400; easing.type: Easing.OutCubic }
                    PauseAnimation { duration: 200 }
                }
            }
            Rectangle {
                anchors.centerIn: parent
                width: 8; height: 8; radius: 4
                color: root.count > 0 ? Tokens.accent : Tokens.textFaint
            }
        }

        Text {
            anchors.verticalCenter: parent.verticalCenter
            text: root.count > 0 ? "Work " + root.count : "Work"
            color: root.count > 0 ? Tokens.text : Tokens.textDim
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontSmall
        }
    }

    MouseArea {
        anchors.fill: parent
        cursorShape: Qt.PointingHandCursor
        onClicked: ShellState.toggleWork()
    }
}
