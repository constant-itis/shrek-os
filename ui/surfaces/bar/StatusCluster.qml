import QtQuick
import "../../themes"
import "../../state"
import "../../services"

// StatusCluster — the bar's compact system indicators (audio / network / bluetooth). Read-only glances
// at live service state; clicking anywhere opens the SYSTEM drawer for the actual controls. Calm and
// text-first (no icon font in the layer yet); a dot carries on/off where a word would be noise.
Item {
    id: root
    implicitWidth: row.implicitWidth
    implicitHeight: Tokens.barHeight

    Row {
        id: row
        anchors.centerIn: parent
        spacing: Tokens.spaceMd

        // audio
        Text {
            anchors.verticalCenter: parent.verticalCenter
            visible: Audio.ready
            text: Audio.muted ? "muted" : Math.round(Audio.volume * 100) + "%"
            color: Audio.muted ? Tokens.notice : Tokens.textDim
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontSmall
        }

        // network
        Row {
            anchors.verticalCenter: parent.verticalCenter
            spacing: Tokens.spaceXs
            Rectangle {
                anchors.verticalCenter: parent.verticalCenter
                width: 7; height: 7; radius: 3.5
                color: Network.online ? Tokens.accent : Tokens.textFaint
            }
            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: "net"
                color: Tokens.textDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
            }
        }

        // bluetooth (only when an adapter exists)
        Text {
            anchors.verticalCenter: parent.verticalCenter
            visible: Bluetooth.available
            text: "bt"
            color: Bluetooth.enabled ? Tokens.textDim : Tokens.textFaint
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontSmall
        }
    }

    MouseArea {
        anchors.fill: parent
        cursorShape: Qt.PointingHandCursor
        onClicked: ShellState.toggleSystem()
    }
}
