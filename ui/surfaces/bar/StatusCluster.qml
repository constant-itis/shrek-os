import QtQuick
import "../../themes"
import "../../state"
import "../../services"

// StatusCluster — the bar's compact system indicators (audio / network / bluetooth). Read-only glances
// at live service state; clicking anywhere opens the SYSTEM drawer for the actual controls. Calm and
// text-first (no icon font in the layer yet); a dot carries on/off where a word would be noise.
Item {
    id: root
    implicitWidth: col.implicitWidth
    implicitHeight: col.implicitHeight

    Column {
        id: col
        anchors.centerIn: parent
        spacing: Tokens.spaceSm

        // audio
        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            visible: Audio.ready
            text: Audio.muted ? "mute" : Math.round(Audio.volume * 100) + "%"
            color: Audio.muted ? Tokens.notice : Tokens.textDim
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontCaption
        }

        // network — a dot carries on/off where a word would be noise in the narrow rail
        Rectangle {
            anchors.horizontalCenter: parent.horizontalCenter
            width: 7; height: 7; radius: 3.5
            color: Network.online ? Tokens.accent : Tokens.textFaint
        }

        // bluetooth (only when an adapter exists)
        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            visible: Bluetooth.available
            text: "bt"
            color: Bluetooth.enabled ? Tokens.textDim : Tokens.textFaint
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontCaption
        }
    }

    MouseArea {
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onEntered: ShellState.openRailPopout("system", root.mapToItem(null, 0, root.height / 2).y)
        onExited: ShellState.closeRailPopout("system")
        onClicked: ShellState.toggleSystem()
    }
}
