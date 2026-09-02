import QtQuick
import QtQuick.Layouts
import "../theme"

Item {
    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        Chrome { Layout.fillWidth: true; step: 7 }

        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true

            ColumnLayout {
                anchors.centerIn: parent
                width: Math.min(parent.width - 2 * Tokens.stagePad, 520)
                spacing: Tokens.spaceMd

                Rectangle {
                    width: 72
                    height: 72
                    radius: 36
                    Layout.alignment: Qt.AlignHCenter
                    Layout.bottomMargin: Tokens.spaceSm
                    color: Tokens.sealSurface
                    border.width: 1
                    border.color: Tokens.sealOutline
                    Text {
                        anchors.centerIn: parent
                        text: "✓"
                        color: Tokens.accent
                        font.family: Tokens.fontFamily
                        font.pixelSize: 34
                    }
                }
                Text {
                    text: "Shrek OS is installed"
                    color: Tokens.textPrimary
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontHero
                    font.weight: Font.Bold
                    horizontalAlignment: Text.AlignHCenter
                    Layout.alignment: Qt.AlignHCenter
                }
                Text {
                    text: "After reboot, you'll finish setting up your owner account and passphrase. Remove the install media before you restart."
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
            asideText: "Safe to remove install media now"
            primaryText: "Restart now"
        }
    }
}
