import QtQuick
import "../../themes"
import "../../services"

// MediaControl — a compact now-playing transport in the bar (MPRIS, via the Mpris service). Shows only
// when a player is present: prev · play/pause · next + the track title (click the title to raise the
// player's own window). Ordinary media control — no authority here. Transport buttons dim when the active
// player reports it can't do that action.
Row {
    id: media
    spacing: 2
    visible: Mpris.hasPlayer

    // round hover button carrying a text glyph
    component IconBtn: Rectangle {
        property string glyph: ""
        property bool on: true
        property var act
        width: 22
        height: 22
        radius: 11
        anchors.verticalCenter: parent ? parent.verticalCenter : undefined
        color: bMa.containsMouse && on ? Tokens.surfaceAlt : "transparent"
        opacity: on ? 1 : 0.4
        Text {
            anchors.centerIn: parent
            text: glyph
            color: Tokens.text
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontSmall
        }
        MouseArea {
            id: bMa
            anchors.fill: parent
            hoverEnabled: true
            enabled: on
            cursorShape: Qt.PointingHandCursor
            onClicked: if (act) act()
        }
    }

    IconBtn { glyph: "◀◀"; on: Mpris.canGoPrevious; act: function () { Mpris.previous() } }

    // play/pause — play triangle when paused, twin bars when playing (font-independent)
    Rectangle {
        width: 22
        height: 22
        radius: 11
        anchors.verticalCenter: parent ? parent.verticalCenter : undefined
        color: ppMa.containsMouse ? Tokens.surfaceAlt : "transparent"
        Text {
            visible: !Mpris.playing
            anchors.centerIn: parent
            text: "▶"
            color: Tokens.text
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontSmall
        }
        Row {
            visible: Mpris.playing
            anchors.centerIn: parent
            spacing: 3
            Rectangle { width: 3; height: 10; radius: 1; color: Tokens.text }
            Rectangle { width: 3; height: 10; radius: 1; color: Tokens.text }
        }
        MouseArea {
            id: ppMa
            anchors.fill: parent
            hoverEnabled: true
            enabled: Mpris.canTogglePlaying
            cursorShape: Qt.PointingHandCursor
            onClicked: Mpris.playPause()
        }
    }

    IconBtn { glyph: "▶▶"; on: Mpris.canGoNext; act: function () { Mpris.next() } }

    // track title — click to raise the player's own window
    Text {
        anchors.verticalCenter: parent ? parent.verticalCenter : undefined
        leftPadding: Tokens.spaceXs
        text: Mpris.title.length > 0 ? Mpris.title : Mpris.identity
        color: Tokens.textDim
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontSmall
        elide: Text.ElideRight
        width: Math.min(implicitWidth + leftPadding, 170)
        MouseArea {
            anchors.fill: parent
            anchors.margins: -4
            cursorShape: Qt.PointingHandCursor
            onClicked: Mpris.raise()
        }
    }
}
