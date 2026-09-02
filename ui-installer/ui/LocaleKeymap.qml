import QtQuick
import QtQuick.Layouts
import "../theme"

Item {
    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        Chrome { Layout.fillWidth: true; step: 2 }

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
                    text: "STEP 2 OF 7"
                    color: Tokens.accent
                    font.family: Tokens.fontMono
                    font.pixelSize: Tokens.fontCaption
                    font.letterSpacing: 1.6
                }
                Text {
                    text: "Language & keyboard"
                    color: Tokens.textPrimary
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontHero
                    font.weight: Font.Bold
                }
                Text {
                    text: "Sets your system language and the keyboard layout used everywhere — including the password prompt at first start."
                    color: Tokens.textSecondary
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                    wrapMode: Text.WordWrap
                    Layout.maximumWidth: 520
                }

                Item { height: Tokens.spaceSm }

                Field { label: "Language"; value: "English (United States)"; mono: "en_US.UTF-8"; kind: "select" }
                Field { label: "Keyboard layout"; value: "English (US)"; mono: "us"; kind: "select" }
                Field { label: "Test your keyboard"; value: "Type here to check the layout"; placeholder: true; kind: "test" }

                Item { Layout.fillHeight: true }
            }
        }

        ActionBar { Layout.fillWidth: true; backText: "Back"; primaryText: "Continue" }
    }
}
