import QtQuick
import "../../themes"
import "../../state"

// Clock — local time, minute resolution, stacked HH over MM to fit the vertical rail. Clicking it opens
// the Dashboard (the overview surface). A 10 s timer is enough for HH:mm.
Item {
    id: root
    implicitWidth: c.implicitWidth
    implicitHeight: c.implicitHeight
    function refresh() {
        var now = new Date()
        hh.text = Qt.formatDateTime(now, "HH")
        mm.text = Qt.formatDateTime(now, "mm")
    }
    Column {
        id: c
        anchors.centerIn: parent
        spacing: 0
        Text { id: hh; anchors.horizontalCenter: parent.horizontalCenter; color: Tokens.text; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontBody }
        Text { id: mm; anchors.horizontalCenter: parent.horizontalCenter; color: Tokens.textDim; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontBody }
    }
    MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: ShellState.toggleDashboard() }
    Component.onCompleted: refresh()
    Timer { interval: 10000; running: true; repeat: true; onTriggered: root.refresh() }
}
