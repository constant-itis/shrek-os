import QtQuick
import QtQuick.Layouts
import "../theme"
import "../state"

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

                // Bound to the Intent singleton (single source the collect bridge reads). Rich pickers are a
                // follow-up; M1 ships the §5a defaults wired through the flow.
                Field { label: "Language"; value: Intent.localeLabel; mono: Intent.locale; kind: "select" }
                Field { label: "Keyboard layout"; value: Intent.keymapLabel; mono: Intent.keymap; kind: "select" }
                Field { label: "Test your keyboard"; value: "Type here to check the layout"; placeholder: true; kind: "test" }

                Item { Layout.fillHeight: true }
            }
        }

        ActionBar {
            Layout.fillWidth: true
            backText: "Back"
            primaryText: "Continue"
            onBackClicked: Intent.back()
            onPrimaryClicked: Intent.next()
        }
    }
}
