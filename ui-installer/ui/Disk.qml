import QtQuick
import QtQuick.Layouts
import "../theme"
import "../state"

Item {
    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        Chrome { Layout.fillWidth: true; step: 4 }

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
                    text: "STEP 4 OF 7"
                    color: Tokens.accent
                    font.family: Tokens.fontMono
                    font.pixelSize: Tokens.fontCaption
                    font.letterSpacing: 1.6
                }
                Text {
                    text: "Where should Shrek OS live?"
                    color: Tokens.textPrimary
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontHero
                    font.weight: Font.Bold
                }
                Text {
                    text: "Pick the disk to install onto. You'll confirm before anything is erased."
                    color: Tokens.textSecondary
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                    wrapMode: Text.WordWrap
                    Layout.maximumWidth: 520
                }

                Item { height: Tokens.spaceXs }

                DiskRow { Layout.preferredWidth: 640; selected: true; model_: "Samsung SSD 980 PRO"; dev: "/dev/nvme0n1"; size: "1.0 TB" }
                DiskRow { Layout.preferredWidth: 640; model_: "WDC WD10EZEX"; dev: "/dev/sda"; size: "1.0 TB" }
                DiskRow { Layout.preferredWidth: 640; excluded: true; model_: "SanDisk Ultra (install media)"; dev: "/dev/sdb"; size: "excluded" }

                Item { height: Tokens.spaceXs }

                Rectangle {
                    Layout.preferredWidth: 640
                    implicitHeight: warnRow.implicitHeight + 22
                    radius: Tokens.radius
                    color: Tokens.noticeSurface
                    border.width: 1
                    border.color: Tokens.noticeOutline

                    RowLayout {
                        id: warnRow
                        anchors.fill: parent
                        anchors.leftMargin: 14
                        anchors.rightMargin: 14
                        spacing: 10
                        Text { text: "!"; color: Tokens.notice; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontBody; font.weight: Font.Bold }
                        Text {
                            text: "Everything on the selected disk will be erased. You'll confirm on the next step."
                            color: Tokens.notice
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                            wrapMode: Text.WordWrap
                            Layout.fillWidth: true
                        }
                    }
                }

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
