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

        Rectangle {
            anchors.verticalCenter: parent.verticalCenter
            width: 8; height: 8; radius: 4
            color: root.count > 0 ? Tokens.accent : Tokens.textFaint
        }

        Text {
            anchors.verticalCenter: parent.verticalCenter
            text: root.count > 0 ? "Work " + root.count : "Work"
            color: Tokens.textDim
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
