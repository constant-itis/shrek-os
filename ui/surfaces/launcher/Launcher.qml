import Quickshell
import Quickshell.Wayland
import QtQuick
import "../../themes"
import "../../state"
import "../../services"

// Launcher — the app/action launcher (Desktop Slice 1, Phase 2). Toggled with Super+D via IPC. A
// centered search panel over a dim scrim: type to filter installed apps (Applications service, fuzzy-
// ranked), Up/Down to move, Enter/click to launch, Esc/click-out to close. Launching is an ordinary
// user action (DesktopEntry.execute); the launcher reads no authority and mints none.
PanelWindow {
    id: launcher
    WlrLayershell.layer: WlrLayer.Overlay
    WlrLayershell.keyboardFocus: WlrKeyboardFocus.Exclusive
    visible: ShellState.launcherOpen
    anchors { top: true; bottom: true; left: true; right: true }
    exclusiveZone: 0
    color: "transparent"   // scrim is an animated child so it can fade in

    property int sel: 0
    readonly property var results: Applications.results

    // Entrance motion: 0 = closed, 1 = open. Deferred to the next tick (Qt.callLater) so the Behavior
    // animates from 0 on each show instead of snapping to the bound value. Layer-shell windows are torn
    // down on hide, so there is no exit animation to drive — entrance only, per the calm motion vocab.
    property real anim: 0
    Behavior on anim { NumberAnimation { duration: Tokens.animMed; easing.type: Easing.OutCubic } }

    onVisibleChanged: if (visible) open()

    function open() {
        Applications.query = ""
        sel = 0
        input.text = ""
        input.forceActiveFocus()
        anim = 0
        Qt.callLater(function () { anim = 1 })
    }
    function close() { ShellState.closeAll() }
    function launchSel() {
        if (sel >= 0 && sel < results.length)
            Applications.launch(results[sel])
        close()
    }
    function move(d) {
        if (results.length === 0) { sel = 0; return }
        sel = (sel + d + results.length) % results.length
        list.positionViewAtIndex(sel, ListView.Contain)
    }

    // dim scrim — fades in with the launcher; click-out closes
    Rectangle {
        anchors.fill: parent
        color: "#000000"
        opacity: 0.53 * launcher.anim
        MouseArea { anchors.fill: parent; onClicked: launcher.close() }
    }

    Rectangle {
        id: panel
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.top: parent.top
        anchors.topMargin: Math.round(parent.height * 0.14)
        width: Math.min(640, parent.width - 2 * Tokens.spaceXl)
        height: Math.min(480, Math.round(parent.height * 0.6))
        radius: Tokens.radiusLg
        color: Tokens.panelBg
        border.color: Tokens.border
        // scale up + fade + rise slightly as it opens
        opacity: launcher.anim
        transform: [
            Scale {
                origin.x: panel.width / 2; origin.y: 0
                xScale: 0.96 + 0.04 * launcher.anim
                yScale: 0.96 + 0.04 * launcher.anim
            },
            Translate { y: (1 - launcher.anim) * 10 }
        ]

        // eat clicks inside the panel so they don't fall through to the close scrim
        MouseArea { anchors.fill: parent }

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
                    id: prompt
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
                    onTextChanged: { Applications.query = text; launcher.sel = 0 }
                    Keys.onDownPressed: launcher.move(1)
                    Keys.onUpPressed: launcher.move(-1)
                    Keys.onReturnPressed: launcher.launchSel()
                    Keys.onEnterPressed: launcher.launchSel()
                    Keys.onEscapePressed: launcher.close()

                    Text {
                        anchors.fill: parent
                        verticalAlignment: Text.AlignVCenter
                        visible: input.text.length === 0
                        text: "Search apps…"
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
                    model: launcher.results
                    currentIndex: launcher.sel
                    boundsBehavior: Flickable.StopAtBounds

                    delegate: Rectangle {
                        width: list.width
                        height: 48
                        radius: Tokens.radius
                        color: index === launcher.sel ? Tokens.rowHi : "transparent"

                        Row {
                            anchors.fill: parent
                            anchors.leftMargin: Tokens.spaceMd
                            anchors.rightMargin: Tokens.spaceMd
                            spacing: Tokens.spaceMd

                            // real freedesktop icon (Quickshell image://icon -> Papirus); letter-avatar
                            // fallback only when the entry declares no icon name.
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
                            onEntered: launcher.sel = index
                            onClicked: { launcher.sel = index; launcher.launchSel() }
                        }
                    }
                }

                Text {
                    anchors.centerIn: parent
                    visible: launcher.results.length === 0
                    text: input.text.length === 0 ? "No applications found" : "No matches"
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
                    text: launcher.results.length + (launcher.results.length === 1 ? " app" : " apps")
                    color: Tokens.textFaint
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontCaption
                }
            }
        }
    }
}
