import Quickshell
import Quickshell.Wayland
import QtQuick
import "../../themes"
import "../../state"
import "../../services"

// ClipboardPicker — the clipboard-history picker (Desktop Slice 7). Toggled with Super+V via IPC. A
// centered search panel over a dim scrim: type to filter captured clipboard entries (Clipboard service),
// Up/Down to move, Enter/click to copy the entry back to the clipboard, Esc/click-out to close. Copying
// is an ordinary user action (wl-copy); this surface reads no authority and mints none.
PanelWindow {
    id: picker
    WlrLayershell.layer: WlrLayer.Overlay
    WlrLayershell.keyboardFocus: WlrKeyboardFocus.Exclusive
    visible: ShellState.clipboardOpen
    anchors { top: true; bottom: true; left: true; right: true }
    exclusiveZone: 0
    color: "transparent"

    property int sel: 0
    readonly property var results: {
        var h = Clipboard.history
        var f = input.text.toLowerCase()
        if (f.length === 0) return h
        var out = []
        for (var i = 0; i < h.length; i++)
            if (("" + h[i]).toLowerCase().indexOf(f) !== -1)
                out.push(h[i])
        return out
    }

    // entrance motion (same vocab as the launcher): 0 closed -> 1 open, deferred a tick so it animates
    // from 0 on each show. Layer-shell tears the window down on hide -> entrance only.
    property real anim: 0
    Behavior on anim { NumberAnimation { duration: Tokens.animMed; easing.type: Easing.OutCubic } }
    onVisibleChanged: if (visible) open()

    function open() { sel = 0; input.text = ""; input.forceActiveFocus(); anim = 0; Qt.callLater(function () { anim = 1 }) }
    function close() { ShellState.closeAll() }
    function useSel() {
        if (sel >= 0 && sel < results.length) Clipboard.copy(results[sel])
        close()
    }
    function move(d) {
        if (results.length === 0) { sel = 0; return }
        sel = (sel + d + results.length) % results.length
        list.positionViewAtIndex(sel, ListView.Contain)
    }
    // single-line preview of a possibly-multiline entry
    function preview(t) {
        var s = ("" + t).replace(/\s+/g, " ").trim()
        return s.length > 0 ? s : "(whitespace)"
    }
    function lineCount(t) { return ("" + t).split("\n").length }

    Rectangle {
        anchors.fill: parent
        color: "#000000"
        opacity: 0.53 * picker.anim
        MouseArea { anchors.fill: parent; onClicked: picker.close() }
    }

    Rectangle {
        id: panel
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.top: parent.top
        anchors.topMargin: Math.round(parent.height * 0.14)
        width: Math.min(680, parent.width - 2 * Tokens.spaceXl)
        height: Math.min(500, Math.round(parent.height * 0.62))
        radius: Tokens.radiusLg
        color: Tokens.panelBg
        border.color: Tokens.border
        opacity: picker.anim
        transform: [
            Scale {
                origin.x: panel.width / 2; origin.y: 0
                xScale: 0.96 + 0.04 * picker.anim
                yScale: 0.96 + 0.04 * picker.anim
            },
            Translate { y: (1 - picker.anim) * 10 }
        ]

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
                    text: "≡"
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
                    onTextChanged: picker.sel = 0
                    Keys.onDownPressed: picker.move(1)
                    Keys.onUpPressed: picker.move(-1)
                    Keys.onTabPressed: picker.move(1)
                    Keys.onBacktabPressed: picker.move(-1)
                    Keys.onReturnPressed: picker.useSel()
                    Keys.onEnterPressed: picker.useSel()
                    Keys.onEscapePressed: picker.close()

                    Text {
                        anchors.fill: parent
                        verticalAlignment: Text.AlignVCenter
                        visible: input.text.length === 0
                        text: "Search clipboard history…"
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
                    model: picker.results
                    currentIndex: picker.sel
                    boundsBehavior: Flickable.StopAtBounds

                    delegate: Rectangle {
                        width: list.width
                        height: 44
                        radius: Tokens.radius
                        color: index === picker.sel ? Tokens.rowHi : "transparent"

                        Row {
                            anchors.fill: parent
                            anchors.leftMargin: Tokens.spaceMd
                            anchors.rightMargin: Tokens.spaceMd
                            spacing: Tokens.spaceMd

                            Text {
                                anchors.verticalCenter: parent.verticalCenter
                                width: parent.width - lines.width - Tokens.spaceMd
                                text: picker.preview(modelData)
                                color: Tokens.text
                                font.family: Tokens.fontMono
                                font.pixelSize: Tokens.fontBody
                                elide: Text.ElideRight
                            }
                            // multi-line hint
                            Text {
                                id: lines
                                anchors.verticalCenter: parent.verticalCenter
                                visible: picker.lineCount(modelData) > 1
                                text: "¶ " + picker.lineCount(modelData)
                                color: Tokens.textFaint
                                font.family: Tokens.fontFamily
                                font.pixelSize: Tokens.fontCaption
                            }
                        }

                        MouseArea {
                            anchors.fill: parent
                            hoverEnabled: true
                            onEntered: picker.sel = index
                            onClicked: { picker.sel = index; picker.useSel() }
                        }
                    }
                }

                Text {
                    anchors.centerIn: parent
                    visible: picker.results.length === 0
                    text: input.text.length === 0 ? "Clipboard history is empty" : "No matches"
                    color: Tokens.textFaint
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                }
            }

            // ── footer ──
            Item {
                id: hints
                width: parent.width
                height: 16
                Text {
                    anchors.left: parent.left
                    anchors.verticalCenter: parent.verticalCenter
                    text: "↑↓ select   ↵ copy   esc close"
                    color: Tokens.textFaint
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontCaption
                }
                Text {
                    anchors.right: parent.right
                    anchors.verticalCenter: parent.verticalCenter
                    text: picker.results.length + (picker.results.length === 1 ? " entry" : " entries")
                    color: Tokens.textFaint
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontCaption
                }
            }
        }
    }
}
