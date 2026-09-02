import QtQuick
import QtQuick.Layouts
import Quickshell.Io
import "../theme"
import "../state"

Item {
    // The file-legible collect bridge (ADR-005 §6): at the point of no return, hand the collected intent to
    // shrek-provision-collect as ARGV (never a shell string — the owner name is untrusted). It writes the
    // collect file and runs shrek-provision-stage, producing the staged manifest for the target transplant.
    Process {
        id: collector
        command: ["/usr/libexec/shrek/shrek-provision-collect",
                  "--schema", String(Intent.schemaVersion),
                  "--locale", Intent.locale,
                  "--keymap", Intent.keymap,
                  "--name",   Intent.ownerName]
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        Chrome { Layout.fillWidth: true; step: 5 }

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
                    text: "STEP 5 OF 7 · DESTRUCTIVE"
                    color: Tokens.danger
                    font.family: Tokens.fontMono
                    font.pixelSize: Tokens.fontCaption
                    font.letterSpacing: 1.6
                }
                Text {
                    text: "Erase this disk and install?"
                    color: Tokens.textPrimary
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontHero
                    font.weight: Font.Bold
                }
                Text {
                    text: "This permanently erases the entire disk. There's no undo once install begins."
                    color: Tokens.textSecondary
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                    wrapMode: Text.WordWrap
                    Layout.maximumWidth: 520
                }

                Rectangle {
                    Layout.preferredWidth: 560
                    Layout.topMargin: Tokens.spaceSm
                    implicitHeight: tcol.implicitHeight + 2 * Tokens.spaceLg
                    radius: Tokens.radius
                    color: Tokens.dangerSurface
                    border.width: 1
                    border.color: Tokens.dangerOutline

                    ColumnLayout {
                        id: tcol
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.leftMargin: Tokens.spaceLg
                        anchors.rightMargin: Tokens.spaceLg
                        spacing: 6
                        Text {
                            text: "TARGET DISK — WILL BE ERASED"
                            color: Tokens.danger
                            font.family: Tokens.fontMono
                            font.pixelSize: Tokens.fontCaption
                            font.letterSpacing: 1.4
                        }
                        Text {
                            text: "Samsung SSD 980 PRO · 1.0 TB"
                            color: Tokens.textPrimary
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontTitle
                        }
                        Text {
                            text: "/dev/nvme0n1 — all existing partitions and data removed"
                            color: Tokens.textSecondary
                            font.family: Tokens.fontMono
                            font.pixelSize: Tokens.fontSmall
                        }
                    }
                }

                RowLayout {
                    Layout.topMargin: Tokens.spaceSm
                    spacing: 11
                    Rectangle {
                        width: 20
                        height: 20
                        radius: Tokens.radiusSm
                        color: Tokens.dangerSurface
                        border.width: 1
                        border.color: Tokens.danger
                        Text {
                            anchors.centerIn: parent
                            text: "✓"
                            color: Tokens.danger
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                            font.weight: Font.Bold
                        }
                    }
                    Text {
                        text: "I understand everything on this disk will be erased."
                        color: Tokens.textPrimary
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontBody
                    }
                }

                Item { Layout.fillHeight: true }
            }
        }

        ActionBar {
            Layout.fillWidth: true
            backText: "Back"
            primaryText: "Erase disk & install"
            primaryKind: "danger"
            onBackClicked: Intent.back()
            onPrimaryClicked: {
                // Stage the collected intent, then advance to the install progress screen. The actual disk
                // write is the calamares deploy job (main.py -> shrek-install-target), which transplants the
                // manifest this staged.
                collector.running = true
                Intent.committed = true
                Intent.next()
            }
        }
    }
}
