import QtQuick
import QtQuick.Layouts
import "../theme"

// First-run owner enrollment — runs on the installed, sealed system (post-reboot), a SEPARATE surface from
// the installer. Confirms the transplanted display name and establishes the owner credential (created here,
// target-side, never on the install media — ADR-005 §3/§5). `fault` renders the ADR-005 §10.5 degraded
// state: a one-line notice when some install settings couldn't be applied. Static — no wiring.
Item {
    id: screen
    property bool fault: false

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        Chrome { Layout.fillWidth: true; step: 0; ctx: "First run" }

        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true

            ColumnLayout {
                anchors.centerIn: parent
                width: 440
                spacing: Tokens.spaceMd

                Rectangle {
                    visible: screen.fault
                    Layout.fillWidth: true
                    implicitHeight: faultRow.implicitHeight + 2 * Tokens.spaceMd
                    radius: Tokens.radius
                    color: Tokens.noticeSurface
                    border.width: 1
                    border.color: Tokens.noticeOutline
                    RowLayout {
                        id: faultRow
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.leftMargin: 14
                        anchors.rightMargin: 14
                        spacing: 10
                        Text { text: "◆"; color: Tokens.notice; font.pixelSize: Tokens.fontSmall; Layout.alignment: Qt.AlignTop }
                        Text {
                            text: "Some install settings couldn't be applied — using defaults. Change them in Settings."
                            color: Tokens.notice
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                            wrapMode: Text.WordWrap
                            Layout.fillWidth: true
                        }
                    }
                }

                Text {
                    text: "ALMOST THERE"
                    color: Tokens.accent
                    font.family: Tokens.fontMono
                    font.pixelSize: Tokens.fontCaption
                    font.letterSpacing: 1.6
                    horizontalAlignment: Text.AlignHCenter
                    Layout.alignment: Qt.AlignHCenter
                }
                Text {
                    text: "Finish setting up"
                    color: Tokens.textPrimary
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontHero
                    font.weight: Font.Bold
                    horizontalAlignment: Text.AlignHCenter
                    Layout.alignment: Qt.AlignHCenter
                }
                Text {
                    text: "Confirm your name and create a password. Your password is made here, on this computer."
                    color: Tokens.textSecondary
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.WordWrap
                    Layout.fillWidth: true
                    Layout.bottomMargin: Tokens.spaceSm
                }

                Field { label: "Your name"; value: "Sebastian"; hint: "Carried over from install — edit if you like."; boxWidth: 440 }
                Field { label: "Create a password"; kind: "password"; placeholder: true; boxWidth: 440 }
                Field { label: "Confirm password"; kind: "password"; placeholder: true; boxWidth: 440 }
            }
        }

        ActionBar {
            Layout.fillWidth: true
            asideText: "Required before the desktop starts"
            primaryText: "Create account & continue"
        }
    }
}
