import QtQuick
import "../../themes"

// Clock — local time, minute resolution. A 10 s timer is enough for HH:mm.
Text {
    id: root
    function refresh() { text = Qt.formatDateTime(new Date(), "ddd HH:mm") }
    color: Tokens.text
    font.family: Tokens.fontFamily
    font.pixelSize: Tokens.fontBody
    Component.onCompleted: refresh()
    Timer { interval: 10000; running: true; repeat: true; onTriggered: root.refresh() }
}
