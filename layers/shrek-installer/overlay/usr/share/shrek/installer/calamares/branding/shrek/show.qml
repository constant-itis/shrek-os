import QtQuick 2.15

Item {
    Text {
        anchors.centerIn: parent
        width: Math.min(parent.width * 0.7, 520)
        horizontalAlignment: Text.AlignHCenter
        wrapMode: Text.WordWrap
        text: "Installing Shrek OS"
        font.pixelSize: 24
        color: "#12120F"
    }
}
