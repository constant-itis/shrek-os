import Quickshell
import Quickshell.Wayland
import QtQuick
import "../../themes"
import "../../state"
import "../../services"
import "../../geometry"

// Dashboard — a top-centre overview panel (Super+A / click the rail clock). Deliberately PURPOSE-DRIVEN
// for Shrek OS rather than a generic system monitor: the default tab is WORK (the live isolated-sandbox
// sessions this OS exists to run), with System health, Media and Spaces as supporting glances. Full-screen
// scrim overlay, click-out / Esc closes. Strictly display — reads services + the read-only session
// provider, mints no authority.
PanelWindow {
    id: dash
    WlrLayershell.layer: WlrLayer.Top
    WlrLayershell.keyboardFocus: WlrKeyboardFocus.OnDemand
    visible: ShellState.dashboardOpen
    anchors { top: true; left: true; right: true; bottom: true }
    color: "transparent"

    // read-only session view injected from Shell.qml (the Work tab's data)
    property var provider
    readonly property int sessionCount: provider ? provider.sessions.length : 0
    readonly property int runningCount: {
        if (!provider) return 0
        var n = 0
        for (var i = 0; i < provider.sessions.length; i++)
            if (("" + provider.sessions[i].state) === "running") n++
        return n
    }

    property int tab: 0   // 0 Work · 1 System · 2 Media · 3 Spaces

    // entrance motion
    property real anim: 0
    Behavior on anim { NumberAnimation { duration: Tokens.animMed; easing.type: Easing.OutCubic } }
    onVisibleChanged: if (visible) { tab = 0; anim = 0; Qt.callLater(function () { anim = 1 }); scope.forceActiveFocus() }

    // scrim — click-out closes
    Rectangle {
        anchors.fill: parent
        color: Tokens.scrim
        opacity: 0.5 * dash.anim
        MouseArea { anchors.fill: parent; onClicked: ShellState.closeAll() }
    }

    FocusScope {
        id: scope
        anchors.fill: parent
        focus: true
        Keys.onEscapePressed: ShellState.closeAll()

        Item {
            id: panel
            anchors.horizontalCenter: parent.horizontalCenter
            anchors.top: parent.top
            anchors.topMargin: Tokens.spaceSm
            width: 660
            height: col.implicitHeight + 2 * Tokens.spaceLg
            opacity: dash.anim
            transform: Translate { y: (1 - dash.anim) * -16 }

            EdgePanelShape {
                anchors.fill: parent
                edge: "top"
            }

            Column {
                id: col
                anchors { left: parent.left; right: parent.right; top: parent.top; margins: Tokens.spaceLg }
                spacing: Tokens.spaceLg

                // ── tab strip ──────────────────────────────────────────────────────────────────────
                Row {
                    anchors.horizontalCenter: parent.horizontalCenter
                    spacing: Tokens.spaceXl

                    component Tab: Item {
                        property string label: ""
                        property int index: 0
                        readonly property bool active: dash.tab === index
                        width: tl.implicitWidth
                        height: 34
                        Column {
                            anchors.centerIn: parent
                            spacing: 4
                            Text {
                                id: tl
                                anchors.horizontalCenter: parent.horizontalCenter
                                text: label
                                color: active ? Tokens.text : Tokens.textDim
                                font.family: Tokens.fontFamily
                                font.pixelSize: Tokens.fontBody
                                font.bold: active
                            }
                            Rectangle {
                                anchors.horizontalCenter: parent.horizontalCenter
                                width: tl.implicitWidth; height: 2; radius: 1
                                color: active ? Tokens.accent : "transparent"
                            }
                        }
                        MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: dash.tab = index }
                    }

                    Tab { label: "Work";   index: 0 }
                    Tab { label: "System"; index: 1 }
                    Tab { label: "Media";  index: 2 }
                    Tab { label: "Spaces"; index: 3 }
                }

                Rectangle { width: parent.width; height: 1; color: Tokens.border }

                // ── WORK (default) — the isolated-sandbox sessions this OS runs ──────────────────────
                Column {
                    width: parent.width
                    visible: dash.tab === 0
                    spacing: Tokens.spaceMd

                    Row {
                        spacing: Tokens.spaceMd
                        Text {
                            anchors.verticalCenter: parent.verticalCenter
                            text: dash.sessionCount
                            color: Tokens.text
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontDisplay
                            font.bold: true
                        }
                        Column {
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 1
                            Text {
                                text: "isolated session" + (dash.sessionCount === 1 ? "" : "s")
                                color: Tokens.textDim
                                font.family: Tokens.fontFamily
                                font.pixelSize: Tokens.fontBody
                            }
                            Text {
                                text: dash.runningCount + " running · gVisor T2 sandboxes"
                                color: Tokens.textFaint
                                font.family: Tokens.fontFamily
                                font.pixelSize: Tokens.fontCaption
                            }
                        }
                    }

                    Text {
                        width: parent.width
                        visible: dash.sessionCount === 0
                        text: "No sandboxed work running."
                        color: Tokens.textDim
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontBody
                    }

                    // one card per live session — tier / trust / state chips (read-only records)
                    Flow {
                        width: parent.width
                        spacing: Tokens.spaceSm
                        Repeater {
                            model: dash.provider ? dash.provider.sessions : []
                            Rectangle {
                                width: (col.width - Tokens.spaceSm) / 2
                                height: cbody.implicitHeight + 2 * Tokens.spaceMd
                                radius: Tokens.radius
                                color: Tokens.surface
                                border.color: Tokens.border
                                Column {
                                    id: cbody
                                    anchors { left: parent.left; right: parent.right; verticalCenter: parent.verticalCenter; leftMargin: Tokens.spaceMd; rightMargin: Tokens.spaceMd }
                                    spacing: Tokens.spaceSm
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
                                            font.pixelSize: Tokens.fontSmall
                                            elide: Text.ElideRight
                                        }
                                    }
                                    Flow {
                                        width: parent.width
                                        spacing: Tokens.spaceXs
                                        component Chip: Rectangle {
                                            property string label: ""
                                            property color tint: Tokens.accent
                                            visible: label.length > 0
                                            radius: Tokens.radiusSm
                                            height: 18
                                            width: ct.implicitWidth + 2 * Tokens.spaceSm
                                            color: Qt.rgba(tint.r, tint.g, tint.b, 0.16)
                                            border.color: Qt.rgba(tint.r, tint.g, tint.b, 0.45)
                                            Text {
                                                id: ct
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
                                }
                            }
                        }
                    }
                }

                // ── SYSTEM — general-usage health (the slick gauges, where ratios belong) ────────────
                Row {
                    width: parent.width
                    visible: dash.tab === 1
                    spacing: Tokens.spaceLg
                    Item { width: (parent.width - 2 * Tokens.spaceLg) / 3; height: 128
                        RingGauge {
                            anchors.centerIn: parent
                            value: SysMon.cpuPct / 100
                            primaryText: SysMon.cpuPct + "%"
                            labelText: "CPU"
                            subText: SysMon.cpuTemp >= 0 ? Math.round(SysMon.cpuTemp) + "°C" : ""
                            arcColor: Tokens.accent
                        }
                    }
                    Item { width: (parent.width - 2 * Tokens.spaceLg) / 3; height: 128
                        RingGauge {
                            anchors.centerIn: parent
                            value: SysMon.memPct
                            primaryText: SysMon.memUsedGiB.toFixed(1) + "G"
                            labelText: "Memory"
                            subText: SysMon.memTotalGiB.toFixed(0) + "G total"
                            arcColor: Tokens.notice
                        }
                    }
                    Item { width: (parent.width - 2 * Tokens.spaceLg) / 3; height: 128
                        RingGauge {
                            anchors.centerIn: parent
                            value: SysMon.gpuTemp >= 0 ? SysMon.gpuTemp / 100 : 0
                            primaryText: SysMon.gpuTemp >= 0 ? Math.round(SysMon.gpuTemp) + "°" : "n/a"
                            labelText: "GPU temp"
                            subText: SysMon.gpuTemp >= 0 ? "" : "no sensor"
                            arcColor: Tokens.ok
                        }
                    }
                }

                // ── MEDIA — reuse the Mpris seam ─────────────────────────────────────────────────────
                Item {
                    width: parent.width
                    height: 88
                    visible: dash.tab === 2
                    Text {
                        anchors.centerIn: parent
                        visible: !Mpris.hasPlayer
                        text: "Nothing playing."
                        color: Tokens.textDim
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontBody
                    }
                    Row {
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.left: parent.left
                        spacing: Tokens.spaceLg
                        visible: Mpris.hasPlayer
                        Column {
                            anchors.verticalCenter: parent.verticalCenter
                            spacing: 2
                            Text { text: Mpris.title; color: Tokens.text; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontTitle; elide: Text.ElideRight; width: 360 }
                            Text { text: Mpris.artist; color: Tokens.textDim; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontSmall; elide: Text.ElideRight; width: 360 }
                            Text { text: Mpris.identity; color: Tokens.textFaint; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontCaption }
                        }
                    }
                    Row {
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.right: parent.right
                        spacing: Tokens.spaceMd
                        visible: Mpris.hasPlayer
                        component TBtn: Text {
                            property var act
                            color: hovered ? Tokens.accent : Tokens.text
                            property bool hovered: tma.containsMouse
                            font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontHeadline
                            MouseArea { id: tma; anchors.fill: parent; anchors.margins: -6; hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: if (parent.act) parent.act() }
                        }
                        TBtn { text: "◀◀"; act: function () { Mpris.previous() } }
                        TBtn { text: Mpris.playing ? "❚❚" : "▶"; act: function () { Mpris.playPause() } }
                        TBtn { text: "▶▶"; act: function () { Mpris.next() } }
                    }
                }

                // ── SPACES — live Sway workspaces ────────────────────────────────────────────────────
                Flow {
                    width: parent.width
                    visible: dash.tab === 3
                    spacing: Tokens.spaceSm
                    Repeater {
                        model: Sway.workspaces
                        Rectangle {
                            width: Math.max(40, wl.implicitWidth + 2 * Tokens.spaceMd)
                            height: 40
                            radius: Tokens.radius
                            color: modelData.focused ? Tokens.accent : Tokens.surface
                            border.color: modelData.urgent ? Tokens.notice : Tokens.border
                            Text {
                                id: wl
                                anchors.centerIn: parent
                                text: modelData.name
                                color: modelData.focused ? Tokens.accentText : Tokens.textDim
                                font.family: Tokens.fontFamily
                                font.pixelSize: Tokens.fontBody
                            }
                            MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: { modelData.activate(); ShellState.closeAll() } }
                        }
                    }
                }
            }
        }
    }
}
