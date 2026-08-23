import Quickshell
import Quickshell.Wayland
import QtQuick
import "../config"
import "../state"
import "../theme"

// Work — the HERO surface of shell-v2. A right-attached drawer that DISPLAYS the effective authority of
// every live agent session. gatekeeperd authors one shrek-session/1 record per constructed gVisor T2
// sandbox; the read-only SessionProvider (ported verbatim — the security seam) reads them and projects
// each into an opaque row. This surface renders that truth: identity, effective tier/trust, running
// state, and — when the provider projects them — granted capabilities and egress profile.
//
// STRICTLY read-only. It observes effective authority; it has NO grant / stop / promote / mutation
// affordance and mints nothing (semantic <= data). A row exists IFF gatekeeperd authored a live record.
//
// Full-height transparent layer surface (ExclusionMode.Ignore => overlaps windows without displacing
// them); the card animates width+opacity on UI.workOpen and the Region mask tracks it so clicks pass
// through everywhere the card isn't.
PanelWindow {
    id: work
    property var provider
    readonly property var rows: provider ? provider.sessions : []
    readonly property int count: rows.length

    anchors { top: true; right: true; bottom: true }
    implicitWidth: Config.workWidth + Config.gap
    exclusionMode: ExclusionMode.Ignore
    color: "transparent"
    mask: Region { item: card }

    Component.onCompleted: if (this.WlrLayershell != null) this.WlrLayershell.layer = WlrLayer.Top

    Rectangle {
        id: card
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        anchors.topMargin: Config.gap
        anchors.bottomMargin: Config.gap
        anchors.rightMargin: Config.gap
        width: UI.workOpen ? Config.workWidth : 0
        opacity: UI.workOpen ? 1 : 0
        clip: true
        radius: Config.frameRadius
        color: Tokens.panelBg
        border.width: 1
        border.color: Tokens.outline

        Behavior on width   { NumberAnimation { duration: Config.animMs; easing.type: Easing.OutCubic } }
        Behavior on opacity { NumberAnimation { duration: Config.animMs } }

        Column {
            anchors.fill: parent
            anchors.margins: Tokens.spaceLg
            spacing: Tokens.spaceMd
            visible: UI.workOpen

            // ── header: title + live count + the Shrek framing (each row is an isolated sandbox) ──
            Column {
                width: parent.width
                spacing: 3
                Row {
                    width: parent.width
                    spacing: Tokens.spaceSm
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        text: "Work"
                        color: Tokens.textPrimary
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontHeadline
                        font.bold: true
                    }
                    Rectangle {
                        anchors.verticalCenter: parent.verticalCenter
                        visible: work.count > 0
                        height: 18
                        width: cnt.implicitWidth + 2 * Tokens.spaceSm
                        radius: Tokens.radiusFull
                        color: Tokens.accentDim
                        Text {
                            id: cnt
                            anchors.centerIn: parent
                            text: work.count
                            color: Tokens.accentText
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontCaption
                            font.bold: true
                        }
                    }
                }
                Text {
                    width: parent.width
                    text: work.count > 0
                          ? (work.count + " isolated session" + (work.count === 1 ? "" : "s") + " · effective authority")
                          : "No sandboxed work running"
                    color: Tokens.muted
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontCaption
                    elide: Text.ElideRight
                }
            }

            // ── empty state ──
            Item {
                width: parent.width
                height: 56
                visible: work.count === 0
                Text {
                    anchors.centerIn: parent
                    text: "Nothing running"
                    color: Tokens.textSecondary
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                }
            }

            // ── one card per live session ──
            Repeater {
                model: work.rows

                Rectangle {
                    width: card.width - 2 * Tokens.spaceLg
                    height: body.implicitHeight + 2 * Tokens.spaceMd
                    radius: Tokens.radius
                    color: Tokens.surface
                    border.color: Tokens.outline

                    Column {
                        id: body
                        anchors {
                            left: parent.left; right: parent.right; verticalCenter: parent.verticalCenter
                            leftMargin: Tokens.spaceMd; rightMargin: Tokens.spaceMd
                        }
                        spacing: Tokens.spaceSm

                        // identity: live dot + session id
                        Row {
                            width: parent.width
                            spacing: Tokens.spaceSm
                            Rectangle {
                                anchors.verticalCenter: parent.verticalCenter
                                width: 7; height: 7; radius: Tokens.radiusFull
                                color: ("" + modelData.state) === "active" || ("" + modelData.state) === "running"
                                       ? Tokens.success : Tokens.muted
                            }
                            Text {
                                width: parent.width - 7 - Tokens.spaceSm
                                text: ("" + (modelData.session || "session"))
                                color: Tokens.textPrimary
                                font.family: Tokens.fontMono
                                font.pixelSize: Tokens.fontBody
                                elide: Text.ElideRight
                            }
                        }

                        // workload / subject identity
                        Text {
                            width: parent.width
                            visible: ("" + (modelData.subtitle || "")).length > 0
                            text: "subject · " + (modelData.subtitle || "")
                            color: Tokens.muted
                            font.family: Tokens.fontMono
                            font.pixelSize: Tokens.fontCaption
                            elide: Text.ElideRight
                        }

                        // effective tier / trust / state chips
                        Flow {
                            width: parent.width
                            spacing: Tokens.spaceXs

                            component Chip: Rectangle {
                                property string label: ""
                                property color tint: Tokens.work
                                visible: label.length > 0
                                radius: Tokens.radiusSm
                                height: 18
                                width: chipText.implicitWidth + 2 * Tokens.spaceSm
                                color: Qt.rgba(tint.r, tint.g, tint.b, 0.16)
                                border.color: Qt.rgba(tint.r, tint.g, tint.b, 0.45)
                                Text {
                                    id: chipText
                                    anchors.centerIn: parent
                                    text: parent.label
                                    color: Tokens.textPrimary
                                    font.family: Tokens.fontMono
                                    font.pixelSize: Tokens.fontCaption
                                }
                            }

                            Chip { label: ("" + (modelData.tier  || "")); tint: Tokens.work }
                            Chip { label: ("" + (modelData.trust || "")); tint: Tokens.attention }
                            Chip {
                                label: ("" + (modelData.state || ""))
                                tint: ("" + modelData.state) === "active" || ("" + modelData.state) === "running"
                                      ? Tokens.success : Tokens.muted
                            }
                        }

                        // granted capabilities/access (rendered only when the provider projects it)
                        Text {
                            width: parent.width
                            visible: ("" + (modelData.caps || "")).length > 0
                            text: "caps · " + (modelData.caps || "")
                            color: Tokens.textSecondary
                            font.family: Tokens.fontMono
                            font.pixelSize: Tokens.fontCaption
                            elide: Text.ElideRight
                        }

                        // egress profile / destination (rendered only when the provider projects it)
                        Text {
                            width: parent.width
                            visible: ("" + (modelData.egressProfile || modelData.egressDst || "")).length > 0
                            text: "egress · " + (modelData.egressProfile || "")
                                  + (("" + (modelData.egressDst || "")).length > 0 ? "  →  " + modelData.egressDst : "")
                            color: Tokens.textSecondary
                            font.family: Tokens.fontMono
                            font.pixelSize: Tokens.fontCaption
                            elide: Text.ElideRight
                        }
                    }
                }
            }
        }
    }
}
