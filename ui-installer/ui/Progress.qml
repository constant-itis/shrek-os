import QtQuick
import QtQuick.Layouts
import Quickshell
import Quickshell.Io
import "../theme"
import "../state"

// Install progress (must-fix #4, slice 2). On entry this is the point of no return: EraseConfirm has
// already staged the provisioning manifest, so we spawn the privileged orchestrator and drive the
// progress rows from its line-framed protocol. No polling, no socket — we read shrek-install-run's
// stdout directly (both ends are the same live session).
//   SHREK-INSTALL STEP <id> begin|done   -> row now/done   (id: verify|write|layout|firstboot)
//   SHREK-INSTALL DONE                    -> advance to the Done screen
//   SHREK-INSTALL FAIL <id> <msg>         -> fault state
// In the render harness (SHREK_INSTALLER_SCREEN pinned) we don't spawn anything; we push a few synthetic
// frames through applyFrame() so the screen renders a representative mid-install state AND the parser runs.
Item {
    id: root
    readonly property bool preview: (Quickshell.env("SHREK_INSTALLER_SCREEN") || "") !== ""

    // Per-phase state: pending | now | done (the tokens ProgressRow renders).
    property string sVerify:    "pending"
    property string sWrite:     "pending"
    property string sLayout:    "pending"
    property string sFirstboot: "pending"
    property bool   faulted:    false
    property string failText:   ""

    function setPhase(id, v) {
        if (id === "verify")         sVerify = v
        else if (id === "write")     sWrite = v
        else if (id === "layout")    sLayout = v
        else if (id === "firstboot") sFirstboot = v
    }

    function applyFrame(line) {
        var s = (line || "").trim()
        if (s.indexOf("SHREK-INSTALL ") !== 0) return
        var t = s.split(" ")
        var kind = t[1] || ""
        if (kind === "STEP") {
            root.setPhase(t[2] || "", (t[3] || "") === "begin" ? "now" : "done")
        } else if (kind === "DONE") {
            Intent.next()
        } else if (kind === "FAIL") {
            root.faulted = true
            root.failText = "Install failed while " + (t[2] && t[2] !== "-" ? t[2] : "working") +
                            ". Go Back and try again."
        }
    }

    Process {
        id: installer
        command: ["sudo", "-n", "/usr/libexec/shrek/shrek-install-run", "--target-disk", Intent.diskPath]
        running: !root.preview && Intent.diskPath !== ""
        stdout: SplitParser { onRead: (line) => root.applyFrame(line) }
        onExited: (code, status) => {
            // Belt-and-suspenders: a crash before any DONE/FAIL frame still faults, so the bar never
            // stalls silently.
            if (code !== 0 && !root.faulted) {
                root.faulted = true
                root.failText = "Install exited unexpectedly (code " + code + "). Go Back and try again."
            }
        }
    }

    Component.onCompleted: {
        if (root.preview) {
            root.applyFrame("SHREK-INSTALL STEP verify begin")
            root.applyFrame("SHREK-INSTALL STEP verify done")
            root.applyFrame("SHREK-INSTALL STEP write begin")
        }
    }

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
                    color: root.faulted ? Tokens.danger : Tokens.accent
                    font.family: Tokens.fontMono
                    font.pixelSize: Tokens.fontCaption
                    font.letterSpacing: 1.6
                }
                Text {
                    text: root.faulted ? "Install didn't finish" : "Installing Shrek OS"
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

                ProgressRow { state_: root.sVerify;    index_: "1"; title: "Verify install media"; sub: "checking the sealed image before writing" }
                ProgressRow { state_: root.sWrite;     index_: "2"; title: "Write system";         sub: "copying the sealed image to disk" }
                ProgressRow { state_: root.sLayout;    index_: "3"; title: "Prepare disk";          sub: "partitions & the shrek-data filesystem" }
                ProgressRow { state_: root.sFirstboot; index_: "4"; title: "Prepare first boot";    sub: "stage language, keyboard & name to the new home" }

                Text {
                    visible: root.faulted
                    text: root.failText
                    color: Tokens.danger
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontSmall
                    wrapMode: Text.WordWrap
                    Layout.topMargin: Tokens.spaceSm
                    Layout.maximumWidth: 540
                }

                Item { Layout.fillHeight: true }
            }
        }

        ActionBar {
            Layout.fillWidth: true
            backText: root.faulted ? "Back" : ""
            asideText: root.faulted ? "Install did not complete" : "Do not remove power or media"
            primaryText: "Continue"
            primaryEnabled: false
            onBackClicked: Intent.back()
        }
    }
}
