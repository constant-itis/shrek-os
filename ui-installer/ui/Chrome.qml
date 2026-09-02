import QtQuick
import QtQuick.Layouts
import "../theme"

// Top chrome shared by every installer surface: helmet + wordmark + context label, plus the stepper
// (installer screens, step 1..7) or nothing (first-run, step 0).
Rectangle {
    id: root
    property int step: 0
    property string ctx: "Install"
    implicitHeight: Tokens.chromeHeight
    color: "transparent"

    Rectangle {
        anchors { left: parent.left; right: parent.right; bottom: parent.bottom }
        height: 1
        color: Tokens.outline
    }

    RowLayout {
        anchors.fill: parent
        anchors.leftMargin: Tokens.spaceXl
        anchors.rightMargin: Tokens.spaceXl
        spacing: Tokens.spaceMd

        Helmet { size: 22; Layout.alignment: Qt.AlignVCenter }
        Text {
            text: "Shrek OS"
            color: Tokens.textPrimary
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontBody
            font.weight: Font.DemiBold
        }
        Rectangle { width: 1; height: 16; color: Tokens.outline; Layout.alignment: Qt.AlignVCenter }
        Text {
            text: root.ctx.toUpperCase()
            color: Tokens.muted
            font.family: Tokens.fontMono
            font.pixelSize: Tokens.fontCaption
            font.letterSpacing: 1.4
        }

        Item { Layout.fillWidth: true }

        Stepper { visible: root.step > 0; current: root.step; Layout.alignment: Qt.AlignVCenter }
    }
}
