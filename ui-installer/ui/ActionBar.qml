import QtQuick
import QtQuick.Layouts
import "../theme"

// Bottom action bar shared by every surface. Left slot: optional Back and/or an aside note. Right slot:
// the primary action, whose label states exactly what it does.
Rectangle {
    id: bar
    property string backText: ""
    property string asideText: ""
    property string primaryText: ""
    property string primaryKind: "accent"
    property bool primaryEnabled: true
    signal backClicked()
    signal primaryClicked()

    implicitHeight: Tokens.actionHeight
    color: Tokens.footerBg

    Rectangle {
        anchors { left: parent.left; right: parent.right; top: parent.top }
        height: 1
        color: Tokens.outline
    }

    RowLayout {
        anchors.fill: parent
        anchors.leftMargin: Tokens.spaceXl
        anchors.rightMargin: Tokens.spaceXl
        spacing: Tokens.spaceMd

        PillButton { visible: bar.backText.length > 0; text: bar.backText; kind: "ghost"; onClicked: bar.backClicked() }
        Text {
            visible: bar.asideText.length > 0
            text: bar.asideText
            color: Tokens.muted
            font.family: Tokens.fontMono
            font.pixelSize: Tokens.fontSmall
            Layout.alignment: Qt.AlignVCenter
        }

        Item { Layout.fillWidth: true }

        PillButton {
            text: bar.primaryText
            kind: bar.primaryKind
            enabled: bar.primaryEnabled
            onClicked: bar.primaryClicked()
        }
    }
}
