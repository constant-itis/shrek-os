import QtQuick
import QtQuick.Layouts
import Quickshell.Io
import "../theme"
import "../state"

Item {
    id: root

    // staging: collect is running; faulted: collect exited non-zero (staging failed).
    property bool staging: false
    property bool faulted: false

    // The file-legible collect bridge (ADR-005 §6): at the point of no return, hand the collected intent to
    // shrek-provision-collect as ARGV (never a shell string — the owner name is untrusted). It writes the
    // collect file and runs shrek-provision-stage, producing the staged manifest for the target transplant.
    // Run via `sudo -n`: staging is a ROOT helper (ADR-005 §6.3) — its output dir is root-owned /run/shrek,
    // which the live installer's `dev` session cannot create unprivileged (the live medium grants dev
    // NOPASSWD sudo). Progression is GATED on the exit code: a failed collect (non-secret intent) fails
    // open-to-safe-default per §6/§10.5 — surfaced here, never silently advanced as if it succeeded.
    Process {
        id: collector
        command: ["sudo", "-n", "/usr/libexec/shrek/shrek-provision-collect",
                  "--schema", String(Intent.schemaVersion),
                  "--locale", Intent.locale,
                  "--keymap", Intent.keymap,
                  "--name",   Intent.ownerName]
        onExited: (exitCode, exitStatus) => {
            root.staging = false
            if (exitCode === 0) {
                Intent.committed = true
                Intent.next()
            } else {
                // Staging failed — do NOT silently advance. The install can still proceed with baked
                // defaults (locale/keymap/name), but only on an explicit second confirmation.
                root.faulted = true
            }
        }
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
                            text: (Intent.diskModel.length > 0 ? Intent.diskModel : "Selected disk") +
                                  (Intent.diskSize.length > 0 ? " · " + Intent.diskSize : "")
                            color: Tokens.textPrimary
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontTitle
                        }
                        Text {
                            text: (Intent.diskPath.length > 0 ? Intent.diskPath : "—") +
                                  " — all existing partitions and data removed"
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

                Text {
                    visible: root.faulted
                    text: "⚠  Couldn't save your locale, keyboard, and name choices — the installed system " +
                          "will use defaults you can change later in Settings. You can install anyway."
                    color: Tokens.danger
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontSmall
                    wrapMode: Text.WordWrap
                    Layout.topMargin: Tokens.spaceSm
                    Layout.maximumWidth: 560
                }

                Item { Layout.fillHeight: true }
            }
        }

        ActionBar {
            Layout.fillWidth: true
            backText: "Back"
            // Gated on the collect exit code: stage the intent first, advance only when it (or the operator's
            // explicit "install with defaults" after a fault) says so — never advance while a silent failure
            // leaves the manifest unstaged.
            primaryText: root.faulted ? "Install with defaults"
                                      : (root.staging ? "Saving your choices…" : "Erase disk & install")
            primaryKind: "danger"
            primaryEnabled: !root.staging
            onBackClicked: Intent.back()
            onPrimaryClicked: {
                if (root.faulted) {
                    // Explicit proceed with baked defaults (intent was not staged). The disk write is the
                    // deploy job (main.py -> shrek-install-target); with no staged manifest the target
                    // first-boot-defaults every key (ADR-005 fail-open-to-safe-default).
                    Intent.committed = true
                    Intent.next()
                    return
                }
                // Stage the collected intent; the collector's onExited advances on success or faults visibly.
                root.staging = true
                collector.running = true
            }
        }
    }
}
