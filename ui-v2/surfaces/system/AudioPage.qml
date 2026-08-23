import QtQuick
import "../../theme"
import "../../services"

Flickable {
    contentWidth: width
    contentHeight: body.implicitHeight
    clip: true

    Column {
        id: body
        width: parent.width
        spacing: Tokens.spaceMd

        Text { width: parent.width; text: "Audio"; color: Tokens.textPrimary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontHeadline; font.bold: true }
        Text { width: parent.width; text: Audio.ready ? Audio.label : "No PipeWire output available."; color: Tokens.textSecondary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontCaption; wrapMode: Text.WordWrap }

        Row {
            width: parent.width
            spacing: Tokens.spaceSm
            visible: Audio.ready
            Text { width: parent.width - 72; anchors.verticalCenter: parent.verticalCenter; text: Audio.muted ? "Output muted" : "Output " + Math.round(Audio.volume * 100) + "%"; color: Tokens.textPrimary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontBody }
            Rectangle { width: 64; height: 30; radius: Tokens.radiusSm; color: Audio.muted ? Tokens.warning : Tokens.surfaceRaised; border.color: Tokens.outline
                Text { anchors.centerIn: parent; text: Audio.muted ? "Unmute" : "Mute"; color: Audio.muted ? Tokens.accentText : Tokens.textPrimary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontSmall }
                MouseArea { anchors.fill: parent; enabled: Audio.ready; cursorShape: Qt.PointingHandCursor; onClicked: Audio.toggleMute() }
            }
        }
        Rectangle {
            width: parent.width; height: 8; radius: 4; color: Tokens.surfaceRaised; opacity: Audio.ready ? 1 : 0.45
            visible: Audio.ready
            Rectangle { anchors.left: parent.left; anchors.top: parent.top; anchors.bottom: parent.bottom; width: parent.width * Math.min(1, Audio.volume); radius: 4; color: Audio.muted ? Tokens.muted : Tokens.accent }
            MouseArea { anchors.fill: parent; enabled: Audio.ready; onPressed: (m) => Audio.setVolume(m.x / width); onPositionChanged: (m) => Audio.setVolume(m.x / width) }
        }

        Text { width: parent.width; text: "Output device"; color: Tokens.textSecondary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontCaption }
        Repeater {
            model: Audio.outputs
            Rectangle {
                required property var modelData
                width: body.width; height: 42; radius: Tokens.radius
                color: modelData.active ? Tokens.accentDim : (hover.containsMouse ? Tokens.surfaceRaised : Tokens.surface)
                border.color: modelData.active ? Tokens.accent : Tokens.outline
                Text { anchors.left: parent.left; anchors.right: parent.right; anchors.verticalCenter: parent.verticalCenter; anchors.leftMargin: Tokens.spaceMd; anchors.rightMargin: Tokens.spaceMd; text: modelData.label; color: Tokens.textPrimary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontBody; elide: Text.ElideRight }
                MouseArea { id: hover; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: Audio.selectOutput(modelData.id) }
            }
        }

        Text { width: parent.width; text: Audio.inputReady ? "Input " + (Audio.inputMuted ? "muted" : Math.round(Audio.inputVolume * 100) + "%") + " - " + Audio.inputLabel : "No input device"; color: Tokens.textSecondary; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontCaption; elide: Text.ElideRight }
        Rectangle {
            width: parent.width; height: 8; radius: 4; color: Tokens.surfaceRaised; opacity: Audio.inputReady ? 1 : 0.45
            Rectangle { anchors.left: parent.left; anchors.top: parent.top; anchors.bottom: parent.bottom; width: parent.width * Math.min(1, Audio.inputVolume); radius: 4; color: Audio.inputMuted ? Tokens.muted : Tokens.accent }
            MouseArea { anchors.fill: parent; enabled: Audio.inputReady; onPressed: (m) => Audio.setInputVolume(m.x / width); onPositionChanged: (m) => Audio.setInputVolume(m.x / width) }
        }
    }
}
