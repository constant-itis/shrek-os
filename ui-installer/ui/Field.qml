import QtQuick
import QtQuick.Layouts
import "../theme"

// Static labelled control. kind: text | select | password | test. `mono` renders a monospace suffix (e.g.
// the locale/keymap code). `test` shows a blinking caret (the keyboard-test field).
ColumnLayout {
    id: f
    property string label: ""
    property string value: ""
    property string mono: ""
    property string hint: ""
    property string kind: "text"
    property bool placeholder: false
    property int boxWidth: 520
    // Editable mode (the owner-name field): renders a TextInput bound to `text` and emits edited() on user
    // input so the owning screen can push it into the Intent singleton without a binding loop.
    property bool editable: false
    property string text: value
    signal edited(string t)
    spacing: 7

    Text {
        text: f.label
        color: Tokens.textSecondary
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontSmall
    }

    Rectangle {
        Layout.preferredWidth: f.boxWidth
        height: Tokens.controlHeight
        radius: Tokens.radius
        color: Tokens.surface
        border.width: 1
        border.color: Tokens.outlineStrong

        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 14
            anchors.rightMargin: 14
            spacing: 10

            Text {
                visible: !f.editable
                text: f.kind === "password" ? "••••••••••" : f.value
                color: f.placeholder ? Tokens.muted : Tokens.textPrimary
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontBody
            }
            TextInput {
                id: input
                visible: f.editable
                text: f.text
                color: Tokens.textPrimary
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontBody
                selectByMouse: true
                clip: true
                Layout.fillWidth: true
                // Grab focus so the field actually receives keystrokes once the window holds the keyboard.
                focus: f.editable
                activeFocusOnTab: true
                Component.onCompleted: if (f.editable) forceActiveFocus()
                onTextEdited: f.edited(text)
            }
            Text {
                visible: f.mono.length > 0
                text: f.mono
                color: Tokens.muted
                font.family: Tokens.fontMono
                font.pixelSize: Tokens.fontSmall
            }
            Rectangle {
                visible: f.kind === "test"
                width: 1
                height: 18
                color: Tokens.accent
                Layout.alignment: Qt.AlignVCenter
                SequentialAnimation on opacity {
                    running: f.kind === "test"
                    loops: Animation.Infinite
                    NumberAnimation { to: 0; duration: 1 }
                    PauseAnimation { duration: 550 }
                    NumberAnimation { to: 1; duration: 1 }
                    PauseAnimation { duration: 550 }
                }
            }

            Item { Layout.fillWidth: true }

            Text {
                visible: f.kind === "select"
                text: "▾"
                color: Tokens.textSecondary
                font.pixelSize: Tokens.fontBody
            }
        }
    }

    Text {
        visible: f.hint.length > 0
        text: f.hint
        color: Tokens.muted
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontSmall
        wrapMode: Text.WordWrap
        Layout.preferredWidth: f.boxWidth
    }
}
