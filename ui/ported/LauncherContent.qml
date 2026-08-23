// SPDX-License-Identifier: GPL-3.0-only
//
// LauncherContent — the launcher's search + results, as a content-only Item mounted inside the
// Caelestia-derived ContentWindow launcher panel slot. The blob panel behind it (ContentWindow's
// launcherBg BlobRect) provides the surface, rounding, deformation, and entrance animation, so this
// carries ONLY the interactive content: a search field over the Applications service, a results list
// (real freedesktop icons via image://icon), keyboard nav, and `>`-prefix run-command mode. This is
// the same behaviour as the standalone launcher/Launcher.qml, refactored to live in the blob frame.

import QtQuick
import "../themes"
import "../state"
import "../services"

Item {
    id: root

    // driven by the panel slot: becomes true when the launcher is open on this screen
    property bool active: false

    property int sel: 0
    readonly property bool runMode: input.text.charAt(0) === ">"
    readonly property string runCmd: runMode ? input.text.substring(1).trim() : ""
    readonly property var results: runMode
        ? (runCmd.length > 0 ? [{ run: true, name: runCmd, genericName: "Run command" }] : [])
        : Applications.results

    function reset() {
        Applications.query = ""
        sel = 0
        input.text = ""
        input.forceActiveFocus()
    }
    function close() { ShellState.closeAll() }
    function launchSel() {
        if (runMode) { if (runCmd.length > 0) Applications.run(runCmd); close(); return }
        if (sel >= 0 && sel < results.length)
            Applications.launch(results[sel])
        close()
    }
    function move(d) {
        if (results.length === 0) { sel = 0; return }
        sel = (sel + d + results.length) % results.length
        list.positionViewAtIndex(sel, ListView.Contain)
    }

    // grab focus + clear the query each time the launcher opens
    onActiveChanged: if (active) Qt.callLater(reset)

    Column {
        anchors.fill: parent
        anchors.margins: Tokens.spaceLg
        spacing: Tokens.spaceMd

        // ── search field ──
        Rectangle {
            width: parent.width
            height: 40
            radius: Tokens.radius
            color: Tokens.surface
            border.color: input.activeFocus ? Tokens.accent : Tokens.border

            Text {
                anchors.left: parent.left
                anchors.leftMargin: Tokens.spaceMd
                anchors.verticalCenter: parent.verticalCenter
                text: "›"
                color: Tokens.accent
                font.family: Tokens.fontMono
                font.pixelSize: Tokens.fontHeadline
                font.bold: true
            }

            TextInput {
                id: input
                anchors.fill: parent
                anchors.leftMargin: Tokens.spaceMd + 20
                anchors.rightMargin: Tokens.spaceMd
                verticalAlignment: TextInput.AlignVCenter
                color: Tokens.text
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontBody
                clip: true
                onTextChanged: { Applications.query = root.runMode ? "" : text; root.sel = 0 }
                Keys.onDownPressed: root.move(1)
                Keys.onUpPressed: root.move(-1)
                Keys.onTabPressed: root.move(1)
                Keys.onBacktabPressed: root.move(-1)
                Keys.onReturnPressed: root.launchSel()
                Keys.onEnterPressed: root.launchSel()
                Keys.onEscapePressed: root.close()

                Text {
                    anchors.fill: parent
                    verticalAlignment: Text.AlignVCenter
                    visible: input.text.length === 0
                    text: "Search apps…   › to run a command"
                    color: Tokens.textFaint
                    font: input.font
                }
            }
        }

        // ── results ──
        Item {
            width: parent.width
            height: parent.height - 40 - hints.height - 2 * Tokens.spaceMd

            ListView {
                id: list
                anchors.fill: parent
                clip: true
                model: root.results
                currentIndex: root.sel
                boundsBehavior: Flickable.StopAtBounds

                delegate: Rectangle {
                    width: list.width
                    height: 48
                    radius: Tokens.radius
                    color: index === root.sel ? Tokens.rowHi : "transparent"

                    Row {
                        anchors.fill: parent
                        anchors.leftMargin: Tokens.spaceMd
                        anchors.rightMargin: Tokens.spaceMd
                        spacing: Tokens.spaceMd

                        Item {
                            anchors.verticalCenter: parent.verticalCenter
                            width: 32; height: 32
                            readonly property string iconName: modelData.icon !== undefined ? ("" + modelData.icon) : ""

                            Rectangle {
                                anchors.fill: parent
                                visible: parent.iconName.length === 0
                                radius: Tokens.radiusSm
                                color: Tokens.accentDim
                                Text {
                                    anchors.centerIn: parent
                                    text: ("" + (modelData.name || "?")).charAt(0).toUpperCase()
                                    color: Tokens.accentText
                                    font.family: Tokens.fontFamily
                                    font.pixelSize: Tokens.fontTitle
                                    font.bold: true
                                }
                            }
                            Image {
                                anchors.fill: parent
                                visible: parent.iconName.length > 0
                                source: parent.iconName.length > 0 ? ("image://icon/" + parent.iconName) : ""
                                sourceSize.width: 32
                                sourceSize.height: 32
                                smooth: true
                                asynchronous: true
                            }
                        }

                        Column {
                            anchors.verticalCenter: parent.verticalCenter
                            width: parent.width - 32 - Tokens.spaceMd
                            Text {
                                width: parent.width
                                text: modelData.name || ""
                                color: Tokens.text
                                font.family: Tokens.fontFamily
                                font.pixelSize: Tokens.fontBody
                                elide: Text.ElideRight
                            }
                            Text {
                                width: parent.width
                                visible: ("" + (modelData.genericName || "")).length > 0
                                text: modelData.genericName || ""
                                color: Tokens.textDim
                                font.family: Tokens.fontFamily
                                font.pixelSize: Tokens.fontCaption
                                elide: Text.ElideRight
                            }
                        }
                    }

                    MouseArea {
                        anchors.fill: parent
                        hoverEnabled: true
                        onEntered: root.sel = index
                        onClicked: { root.sel = index; root.launchSel() }
                    }
                }
            }

            Text {
                anchors.centerIn: parent
                visible: root.results.length === 0
                text: root.runMode ? "Type a command to run…"
                      : (input.text.length === 0 ? "No applications found" : "No matches")
                color: Tokens.textFaint
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontBody
            }
        }

        // ── footer: keyboard hints + live result count ──
        Item {
            id: hints
            width: parent.width
            height: 16
            Text {
                anchors.left: parent.left
                anchors.verticalCenter: parent.verticalCenter
                text: "↑↓ select   ↵ open   esc close"
                color: Tokens.textFaint
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontCaption
            }
            Text {
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                text: root.runMode
                      ? "↵ runs a command"
                      : (input.text.length === 0
                         ? (root.results.length + " apps · recents first")
                         : (root.results.length + (root.results.length === 1 ? " result" : " results")))
                color: Tokens.textFaint
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontCaption
            }
        }
    }
}
