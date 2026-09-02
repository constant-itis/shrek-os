import QtQuick
import "../theme"

// Button. kind: accent | danger | ghost. `enabled` controls opacity AND interactivity. Emits clicked().
Rectangle {
    id: root
    property string text: ""
    property string kind: "accent"
    property bool enabled: true
    signal clicked()

    implicitHeight: Tokens.buttonHeight
    implicitWidth: label.implicitWidth + 44
    radius: Tokens.radius
    opacity: enabled ? 1.0 : 0.4
    color: kind === "accent" ? Tokens.accent : kind === "danger" ? Tokens.danger : "transparent"
    border.width: kind === "ghost" ? 1 : 0
    border.color: Tokens.outlineStrong

    Text {
        id: label
        anchors.centerIn: parent
        text: root.text
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontBody
        font.weight: Font.DemiBold
        color: root.kind === "accent" ? Tokens.accentText
             : root.kind === "danger" ? Tokens.dangerText
             : Tokens.textSecondary
    }

    MouseArea {
        anchors.fill: parent
        enabled: root.enabled
        cursorShape: Qt.PointingHandCursor
        onClicked: root.clicked()
    }
}
