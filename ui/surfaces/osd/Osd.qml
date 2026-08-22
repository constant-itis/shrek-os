import Quickshell
import Quickshell.Wayland
import QtQuick
import "../../themes"
import "../../services"

// Osd — transient on-screen feedback (Desktop Slice 1, Phase 4). Shows a brief volume readout when the
// audio level/mute changes (e.g. the volume keys). Bottom-centre, auto-hides. Brightness OSD lands with
// real backlight hardware (the VM has none). Display only.
PanelWindow {
    id: osd
    WlrLayershell.layer: WlrLayer.Overlay
    anchors { bottom: true }
    implicitWidth: 260
    implicitHeight: 96
    color: "transparent"
    exclusiveZone: 0
    visible: false

    property real level: 0
    property bool muted: false

    function trigger() {
        if (!Audio.ready) return
        osd.level = Audio.volume
        osd.muted = Audio.muted
        osd.visible = true
        hideTimer.restart()
    }

    Connections {
        target: Audio
        function onVolumeChanged() { osd.trigger() }
        function onMutedChanged() { osd.trigger() }
    }

    Timer { id: hideTimer; interval: 1500; repeat: false; onTriggered: osd.visible = false }

    Rectangle {
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom: parent.bottom
        anchors.bottomMargin: Tokens.spaceXl
        width: 240
        height: 48
        radius: Tokens.radiusLg
        color: Tokens.overlay
        border.color: Tokens.border

        Row {
            anchors.centerIn: parent
            spacing: Tokens.spaceMd

            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: osd.muted ? "muted" : "vol"
                color: Tokens.textDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
            }
            Rectangle {
                anchors.verticalCenter: parent.verticalCenter
                width: 140; height: 6; radius: 3
                color: Tokens.surface
                Rectangle {
                    anchors { left: parent.left; top: parent.top; bottom: parent.bottom }
                    width: parent.width * Math.max(0, Math.min(1, osd.level))
                    radius: 3
                    color: osd.muted ? Tokens.textFaint : Tokens.accent
                }
            }
            Text {
                anchors.verticalCenter: parent.verticalCenter
                text: Math.round(osd.level * 100)
                color: Tokens.text
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
            }
        }
    }
}
