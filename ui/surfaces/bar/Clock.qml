import QtQuick
import "../../themes"

// Clock — local time, minute resolution, stacked HH over MM to fit the vertical rail. A 10 s timer is
// enough for HH:mm.
Column {
    id: root
    spacing: 0
    function refresh() {
        var now = new Date()
        hh.text = Qt.formatDateTime(now, "HH")
        mm.text = Qt.formatDateTime(now, "mm")
    }
    Text {
        id: hh
        anchors.horizontalCenter: parent.horizontalCenter
        color: Tokens.text
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontBody
    }
    Text {
        id: mm
        anchors.horizontalCenter: parent.horizontalCenter
        color: Tokens.textDim
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontBody
    }
    Component.onCompleted: refresh()
    Timer { interval: 10000; running: true; repeat: true; onTriggered: root.refresh() }
}
