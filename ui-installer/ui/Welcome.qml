import QtQuick
import QtQuick.Layouts
import "../theme"

Item {
    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        Chrome { Layout.fillWidth: true; step: 1 }

        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true

            ColumnLayout {
                anchors.centerIn: parent
                width: Math.min(parent.width - 2 * Tokens.stagePad, 560)
                spacing: Tokens.spaceMd

                Helmet { size: 76; Layout.alignment: Qt.AlignHCenter; Layout.bottomMargin: Tokens.spaceSm }
                Text {
                    text: "Welcome to Shrek OS"
                    color: Tokens.textPrimary
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontHero
                    font.weight: Font.Bold
                    horizontalAlignment: Text.AlignHCenter
                    Layout.alignment: Qt.AlignHCenter
                }
                Text {
                    text: "A sealed, self-healing desktop. This installer writes a complete system image to a disk of your choosing, then hands off to first-run setup after a restart."
                    color: Tokens.textSecondary
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.WordWrap
                    Layout.fillWidth: true
                    Layout.alignment: Qt.AlignHCenter
                }
            }
        }

        ActionBar {
            Layout.fillWidth: true
            asideText: "Live session · nothing has been written yet"
            primaryText: "Begin install"
        }
    }
}
