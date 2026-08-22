import Quickshell
import Quickshell.Wayland
import QtQuick
import "../../themes"
import "../../state"

// WorkDrawer — the Work zone. Driven by an injected read-only `provider` (the real SessionProvider that
// reads gatekeeperd-authored shrek-session/1 records). Now toggleable (Super+W via IPC) instead of
// always-on. Renders provider.sessions GENERICALLY: empty -> "Nothing running"; else one opaque row per
// record (title + subtitle projected by the provider). Strictly DISPLAY — NO authority badges, NO
// grant/stop actions, NO mutation. The read-only shrek-session/1 path is unchanged.
PanelWindow {
    id: drawer
    property var provider

    WlrLayershell.layer: WlrLayer.Top
    visible: ShellState.workOpen
    anchors { top: true; right: true; bottom: true }
    implicitWidth: Tokens.drawerWidth
    color: Tokens.panelBg

    // Entrance motion: 0 closed -> 1 open. Slides in from the right edge + fades. Deferred a tick so
    // the Behavior animates from 0 on each show (layer-shell windows tear down on hide -> entrance only).
    property real anim: 0
    Behavior on anim { NumberAnimation { duration: Tokens.animMed; easing.type: Easing.OutCubic } }
    onVisibleChanged: if (visible) { anim = 0; Qt.callLater(function () { anim = 1 }) }

    // left hairline separating the drawer from content
    Rectangle {
        anchors { left: parent.left; top: parent.top; bottom: parent.bottom }
        width: 1
        color: Tokens.border
    }

    Column {
        anchors.fill: parent
        anchors.margins: Tokens.spaceLg
        spacing: Tokens.spaceMd
        opacity: drawer.anim
        transform: Translate { x: (1 - drawer.anim) * 24 }

        Text {
            text: "Work"
            color: Tokens.text
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontTitle
            font.bold: true
        }

        Text {
            visible: !drawer.provider || drawer.provider.sessions.length === 0
            text: "Nothing running"
            color: Tokens.textDim
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontBody
        }

        Repeater {
            model: drawer.provider ? drawer.provider.sessions : []

            // One session card. Shows the real, read-only shrek-session/1 fields the provider projects
            // (session id, tier, trust, state) as distinct chips — NO authority badges are minted here,
            // NO grant/stop/promote; this is display of records gatekeeperd already authored.
            Rectangle {
                width: drawer.width - 2 * Tokens.spaceLg
                height: body.implicitHeight + 2 * Tokens.spaceMd
                radius: Tokens.radius
                color: Tokens.surface
                border.color: Tokens.border

                Column {
                    id: body
                    anchors {
                        left: parent.left; right: parent.right; verticalCenter: parent.verticalCenter
                        leftMargin: Tokens.spaceMd; rightMargin: Tokens.spaceMd
                    }
                    spacing: Tokens.spaceSm

                    // header row: a live dot + session id
                    Row {
                        width: parent.width
                        spacing: Tokens.spaceSm
                        Rectangle {
                            anchors.verticalCenter: parent.verticalCenter
                            width: 7; height: 7; radius: 999
                            color: ("" + modelData.state) === "running" ? Tokens.ok : Tokens.textFaint
                        }
                        Text {
                            width: parent.width - 7 - Tokens.spaceSm
                            text: ("" + (modelData.session || "session"))
                            color: Tokens.text
                            font.family: Tokens.fontMono
                            font.pixelSize: Tokens.fontBody
                            elide: Text.ElideRight
                        }
                    }

                    // chip row: tier / trust / state — only the chips that carry a value
                    Flow {
                        width: parent.width
                        spacing: Tokens.spaceXs

                        component Chip: Rectangle {
                            property string label: ""
                            property color tint: Tokens.accent
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
                                color: Tokens.text
                                font.family: Tokens.fontMono
                                font.pixelSize: Tokens.fontCaption
                            }
                        }

                        Chip { label: ("" + (modelData.tier || "")); tint: Tokens.accent }
                        Chip { label: ("" + (modelData.trust || "")); tint: Tokens.notice }
                        Chip {
                            label: ("" + (modelData.state || ""))
                            tint: ("" + modelData.state) === "running" ? Tokens.ok : Tokens.textFaint
                        }
                    }

                    Text {
                        width: parent.width
                        visible: modelData.subtitle !== undefined && ("" + modelData.subtitle).length > 0
                        text: modelData.subtitle || ""
                        color: Tokens.textFaint
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontCaption
                        elide: Text.ElideRight
                    }
                }
            }
        }
    }
}
