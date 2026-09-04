import QtQuick
import QtQuick.Layouts
import Quickshell
import "../theme"

// First-run Connectivity onboarding (ADR-007 §8, S3). Runs on the installed, sealed system right after
// owner enroll: "your desktop starts sealed — choose what it may reach." The baseline (time + updates)
// is shown as already-on and explained; weather is the low-friction one-click opt-in (Tier-B); web
// browsing is deferred to the console ceremony (S4). Deliberately LEAN — a one-time setup card, not a
// live dashboard: it does not poll status. On finish it fires the SAME unprivileged `egressd ask bless
// weather` the DMS panel uses (fixed argv, no shell); if the supervisor is unreachable, or the clock is
// not yet set so the pin can't resolve, the bless still persists as intent (the daemon is intent-first
// and re-resolves at boot) — so the promise is kept without this screen having to wait or interpret a
// result. Reachable in the render harness via SHREK_INSTALLER_SCREEN=firstrun-connectivity.
Item {
    id: screen
    signal back()
    signal done()

    property bool weatherOn: false
    readonly property string egressBin: Quickshell.env("SHREK_EGRESS_BIN") || "/usr/libexec/shrek/egressd"

    function finish() {
        if (screen.weatherOn)
            Quickshell.execDetached([screen.egressBin, "ask", "bless", "weather"])
        // Load-bearing marker for the render/flow proof.
        console.log("SHREK-INSTALLER firstrun connectivity finished: weather="
            + (screen.weatherOn ? "bless" : "skip"))
        screen.done()
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        Chrome { Layout.fillWidth: true; step: 0; ctx: "First run" }

        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true

            ColumnLayout {
                anchors.centerIn: parent
                width: 460
                spacing: Tokens.spaceMd

                Text {
                    text: "STAY IN CONTROL"
                    color: Tokens.accent
                    font.family: Tokens.fontMono
                    font.pixelSize: Tokens.fontCaption
                    font.letterSpacing: 1.6
                    horizontalAlignment: Text.AlignHCenter
                    Layout.alignment: Qt.AlignHCenter
                }
                Text {
                    text: "Choose what your desktop can reach"
                    color: Tokens.textPrimary
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontHero
                    font.weight: Font.Bold
                    horizontalAlignment: Text.AlignHCenter
                    Layout.alignment: Qt.AlignHCenter
                }
                Text {
                    text: "Your desktop starts sealed — it reaches nothing on its own. Pick what to allow; each choice opens one specific destination, and you can change it anytime in Settings."
                    color: Tokens.textSecondary
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.WordWrap
                    Layout.fillWidth: true
                    Layout.bottomMargin: Tokens.spaceSm
                }

                // Baseline: on out of the box, explained — no control (revoke is console-only).
                Rectangle {
                    Layout.fillWidth: true
                    implicitHeight: baseRow.implicitHeight + 2 * Tokens.spaceMd
                    radius: Tokens.radius
                    color: Tokens.sealSurface
                    border.width: 1
                    border.color: Tokens.sealOutline
                    RowLayout {
                        id: baseRow
                        anchors.left: parent.left; anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.leftMargin: 14; anchors.rightMargin: 14
                        spacing: 10
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 2
                            Text {
                                text: "Time & system updates"
                                color: Tokens.textPrimary
                                font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontBody; font.weight: Font.DemiBold
                            }
                            Text {
                                text: "On by default — keeps the clock correct and the system patched."
                                color: Tokens.textSecondary
                                font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontCaption
                                wrapMode: Text.WordWrap; Layout.fillWidth: true
                            }
                        }
                        Text {
                            text: "On"
                            color: Tokens.ok
                            font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontSmall; font.weight: Font.DemiBold
                        }
                    }
                }

                // Weather: one-click opt-in (Tier-B). A simple selectable row toggling `weatherOn`.
                Rectangle {
                    Layout.fillWidth: true
                    implicitHeight: wxRow.implicitHeight + 2 * Tokens.spaceMd
                    radius: Tokens.radius
                    color: screen.weatherOn ? Tokens.rowHighlight : Tokens.surface
                    border.width: 1
                    border.color: screen.weatherOn ? Tokens.accent : Tokens.outline
                    Behavior on color { ColorAnimation { duration: Tokens.animFast } }

                    RowLayout {
                        id: wxRow
                        anchors.left: parent.left; anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.leftMargin: 14; anchors.rightMargin: 14
                        spacing: 10
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 2
                            Text {
                                text: "Weather"
                                color: Tokens.textPrimary
                                font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontBody; font.weight: Font.DemiBold
                            }
                            Text {
                                text: "Lets the weather widget reach its forecast service (open-meteo)."
                                color: Tokens.textSecondary
                                font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontCaption
                                wrapMode: Text.WordWrap; Layout.fillWidth: true
                            }
                        }
                        // checkbox indicator
                        Rectangle {
                            width: 22; height: 22; radius: Tokens.radiusSm
                            color: screen.weatherOn ? Tokens.accent : "transparent"
                            border.width: 1
                            border.color: screen.weatherOn ? Tokens.accent : Tokens.outlineStrong
                            Text {
                                anchors.centerIn: parent
                                visible: screen.weatherOn
                                text: "✓"
                                color: Tokens.accentText
                                font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontBody; font.weight: Font.Bold
                            }
                        }
                    }
                    MouseArea {
                        anchors.fill: parent
                        cursorShape: Qt.PointingHandCursor
                        onClicked: screen.weatherOn = !screen.weatherOn
                    }
                }

                // Web browsing: broad grant — deferred to the console ceremony (S4), not a one-click here.
                Rectangle {
                    Layout.fillWidth: true
                    implicitHeight: wbRow.implicitHeight + 2 * Tokens.spaceMd
                    radius: Tokens.radius
                    color: Tokens.surface
                    border.width: 1
                    border.color: Tokens.outline
                    RowLayout {
                        id: wbRow
                        anchors.left: parent.left; anchors.right: parent.right
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.leftMargin: 14; anchors.rightMargin: 14
                        spacing: 10
                        ColumnLayout {
                            Layout.fillWidth: true
                            spacing: 2
                            Text {
                                text: "Web browsing"
                                color: Tokens.textPrimary
                                font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontBody; font.weight: Font.DemiBold
                            }
                            Text {
                                text: "Opens broad internet access — set this up later in Settings, with a confirmation."
                                color: Tokens.textSecondary
                                font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontCaption
                                wrapMode: Text.WordWrap; Layout.fillWidth: true
                            }
                        }
                        Text {
                            text: "Later"
                            color: Tokens.muted
                            font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontSmall; font.weight: Font.DemiBold
                        }
                    }
                }
            }
        }

        ActionBar {
            Layout.fillWidth: true
            backText: "Back"
            asideText: "You can change these anytime in Settings"
            primaryText: "Finish setup"
            onBackClicked: screen.back()
            onPrimaryClicked: screen.finish()
        }
    }
}
