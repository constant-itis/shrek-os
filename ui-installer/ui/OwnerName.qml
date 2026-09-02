import QtQuick
import QtQuick.Layouts
import "../theme"

Item {
    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        Chrome { Layout.fillWidth: true; step: 3 }

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
                    text: "STEP 3 OF 7"
                    color: Tokens.accent
                    font.family: Tokens.fontMono
                    font.pixelSize: Tokens.fontCaption
                    font.letterSpacing: 1.6
                }
                Text {
                    text: "What should we call you?"
                    color: Tokens.textPrimary
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontHero
                    font.weight: Font.Bold
                }
                Text {
                    text: "This is how Shrek OS greets you. You'll create your password after the first restart — it's never stored on the install media."
                    color: Tokens.textSecondary
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                    wrapMode: Text.WordWrap
                    Layout.maximumWidth: 520
                }

                Item { height: Tokens.spaceSm }

                Field {
                    label: "Your name"
                    value: "Sebastian"
                    hint: "Shown on the lock screen and in the menu. You can change it later in Settings."
                }

                Item { Layout.fillHeight: true }
            }
        }

        ActionBar { Layout.fillWidth: true; backText: "Back"; primaryText: "Continue" }
    }
}
