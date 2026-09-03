import QtQuick
import QtQuick.Layouts
import Quickshell
import Quickshell.Io
import "../theme"
import "../state"

// Disk picker (must-fix #4, slice 2). Enumerates erasable whole disks live from shrek-list-disks (the
// same Shrek-owned lister the retired Calamares launcher used) and writes the chosen target into Intent.
// In the render harness (SHREK_INSTALLER_SCREEN pinned) the lister binary isn't present, so we feed the
// parser a couple of synthetic rows instead — this both renders a representative screen AND exercises the
// real addDisk() parse path.
Item {
    id: root
    readonly property bool preview: (Quickshell.env("SHREK_INSTALLER_SCREEN") || "") !== ""
    property bool enumerated: false

    // shrek-list-disks emits one eligible whole disk per line as: /dev/NAME<TAB>HUMAN_SIZE<TAB>MODEL.
    function addDisk(line) {
        var s = (line || "").trim()
        if (s.length === 0) return
        var f = s.split("\t")
        var path = f[0] || ""
        if (path.indexOf("/dev/") !== 0) return
        disks.append({ path: path, size: (f[1] || ""), name: (f[2] || "Disk") })
    }

    ListModel { id: disks }

    Process {
        id: lister
        command: ["/usr/libexec/shrek/shrek-list-disks"]
        stdout: SplitParser { onRead: (line) => root.addDisk(line) }
        onExited: (code, status) => root.enumerated = true
    }

    Component.onCompleted: {
        if (root.preview) {
            root.addDisk("/dev/nvme0n1\t1.0TB\tSamsung SSD 980 PRO")
            root.addDisk("/dev/sda\t1.0TB\tWDC WD10EZEX")
            root.enumerated = true
        } else {
            lister.running = true
        }
    }

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

                Repeater {
                    model: disks
                    DiskRow {
                        Layout.preferredWidth: 640
                        model_: model.name
                        dev: model.path
                        size: model.size
                        selected: Intent.diskPath === model.path
                        onClicked: {
                            Intent.diskPath = model.path
                            Intent.diskModel = model.name
                            Intent.diskSize = model.size
                        }
                    }
                }

                // Empty state: enumeration finished and nothing was eligible (all disks are the live
                // medium / already-Shrek / read-only). Never guess — tell the user to attach a disk.
                Rectangle {
                    visible: root.enumerated && disks.count === 0
                    Layout.preferredWidth: 640
                    implicitHeight: emptyRow.implicitHeight + 22
                    radius: Tokens.radius
                    color: Tokens.noticeSurface
                    border.width: 1
                    border.color: Tokens.noticeOutline
                    RowLayout {
                        id: emptyRow
                        anchors.fill: parent
                        anchors.leftMargin: 14
                        anchors.rightMargin: 14
                        spacing: 10
                        Text { text: "!"; color: Tokens.notice; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontBody; font.weight: Font.Bold }
                        Text {
                            text: "No eligible disk found. Attach a blank disk to install onto, then go Back and return to this step."
                            color: Tokens.notice
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                            wrapMode: Text.WordWrap
                            Layout.fillWidth: true
                        }
                    }
                }

                Item { height: Tokens.spaceXs }

                Rectangle {
                    visible: disks.count > 0
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
            primaryEnabled: Intent.diskPath !== ""
            onBackClicked: Intent.back()
            onPrimaryClicked: Intent.next()
        }
    }
}
