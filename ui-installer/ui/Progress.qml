import QtQuick
import QtQuick.Layouts
import "../theme"

Item {
    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        Chrome { Layout.fillWidth: true; step: 6 }

        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true

            ColumnLayout {
                anchors.fill: parent
                anchors.leftMargin: Tokens.stagePad
                anchors.rightMargin: Tokens.stagePad
                anchors.topMargin: 44
                spacing: Tokens.spaceMd

                Text {
                    text: "STEP 6 OF 7"
                    color: Tokens.accent
                    font.family: Tokens.fontMono
                    font.pixelSize: Tokens.fontCaption
                    font.letterSpacing: 1.6
                }
                Text {
                    text: "Installing Shrek OS"
                    color: Tokens.textPrimary
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontHero
                    font.weight: Font.Bold
                }
                Text {
                    text: "Writing the sealed system image, verifying its integrity, and staging your choices for first boot. This takes a few minutes — don't power off."
                    color: Tokens.textSecondary
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                    wrapMode: Text.WordWrap
                    Layout.maximumWidth: 540
                }

                Item { height: Tokens.spaceMd }

                ProgressRow { state_: "done"; title: "Prepare disk"; sub: "system layout & shrek-data on nvme0n1" }
                ProgressRow { state_: "now"; title: "Write system"; sub: "copying the sealed image to disk"; pct: 62 }
                ProgressRow { state_: "pending"; index_: "2"; title: "Verify system"; sub: "dm-verity integrity check" }
                ProgressRow { state_: "pending"; index_: "3"; title: "Prepare first boot"; sub: "stage language, keyboard & name to the new home" }

                Item { Layout.fillHeight: true }
            }
        }

        ActionBar {
            Layout.fillWidth: true
            asideText: "Do not remove power or media"
            primaryText: "Continue"
            primaryEnabled: false
        }
    }
}
