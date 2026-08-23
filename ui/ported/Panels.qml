// SPDX-License-Identifier: GPL-3.0-only
//
// Shrek Shell derivative port of Caelestia Shell's modules/drawers/Panels.qml.
// The panel objects are intentionally Items, not PanelWindows: ContentWindow owns
// their background geometry, deformation, input regions, and animation.

pragma ComponentBehavior: Bound

import QtQuick
import "../themes"
import "../state"
import "../services"

Item {
    id: root

    property var screen
    property var session
    property bool active: true
    required property Item bar
    required property real borderThickness

    readonly property alias dashboard: dashboard
    readonly property alias launcher: launcher
    readonly property alias sidePanel: sidePanel
    readonly property alias popoutsWrapper: popoutsWrapper
    readonly property alias popouts: popoutsWrapper
    readonly property alias edgeDock: edgeDock

    readonly property bool sideOpen: active && (ShellState.workOpen || ShellState.systemOpen)
    readonly property int sessionCount: session ? session.sessions.length : 0

    anchors.fill: parent
    anchors.margins: borderThickness
    anchors.leftMargin: bar.implicitWidth

    PanelSlot {
        id: dashboard
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.top: parent.top
        width: Math.min(680, parent.width - 2 * Tokens.spaceXl)
        height: 246
        open: root.active && ShellState.dashboardOpen
        edge: "top"

        Column {
            anchors.fill: parent
            anchors.margins: Tokens.spaceLg
            spacing: Tokens.spaceMd

            Row {
                width: parent.width
                spacing: Tokens.spaceMd
                Text {
                    text: root.sessionCount
                    color: Tokens.text
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontDisplay
                    font.bold: true
                }
                Column {
                    anchors.verticalCenter: parent.verticalCenter
                    Text {
                        text: "isolated sessions"
                        color: Tokens.text
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontTitle
                        font.bold: true
                    }
                    Text {
                        text: "Work, media, system, and spaces stay Shrek-owned."
                        color: Tokens.textDim
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontCaption
                    }
                }
            }

            Rectangle { width: parent.width; height: 1; color: Tokens.border }

            Text {
                width: parent.width
                text: root.sessionCount === 0 ? "No sandboxed work running." : "Live work records are read from shrek-session/1."
                color: Tokens.textDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontBody
                wrapMode: Text.WordWrap
            }
        }
    }

    PanelSlot {
        id: launcher
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom: parent.bottom
        width: Math.min(640, parent.width - 2 * Tokens.spaceXl)
        height: 480
        open: root.active && ShellState.launcherOpen
        edge: "bottom"

        LauncherContent {
            anchors.fill: parent
            active: launcher.open
        }
    }

    PanelSlot {
        id: sidePanel
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        width: Tokens.drawerWidth
        open: root.sideOpen
        edge: "right"

        Column {
            id: sideBody
            anchors.fill: parent
            anchors.margins: Tokens.spaceLg
            spacing: Tokens.spaceLg

            Text {
                text: ShellState.systemOpen ? "System" : "Work"
                color: Tokens.text
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontTitle
                font.bold: true
            }

            Column {
                width: parent.width
                spacing: Tokens.spaceSm
                visible: ShellState.workOpen

                Text {
                    width: parent.width
                    text: root.sessionCount === 0 ? "No sandboxed work running" : root.sessionCount + " isolated session" + (root.sessionCount === 1 ? "" : "s")
                    color: Tokens.textDim
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                    wrapMode: Text.WordWrap
                }

                Repeater {
                    model: root.session ? root.session.sessions : []

                    Rectangle {
                        width: sideBody.width
                        height: rowBody.implicitHeight + 2 * Tokens.spaceMd
                        radius: Tokens.radius
                        color: Tokens.surface
                        border.color: Tokens.border

                        Column {
                            id: rowBody
                            anchors {
                                left: parent.left
                                right: parent.right
                                verticalCenter: parent.verticalCenter
                                leftMargin: Tokens.spaceMd
                                rightMargin: Tokens.spaceMd
                            }
                            spacing: Tokens.spaceXs

                            Text {
                                width: parent.width
                                text: "" + (modelData.session || "session")
                                color: Tokens.text
                                font.family: Tokens.fontMono
                                font.pixelSize: Tokens.fontBody
                                elide: Text.ElideRight
                            }
                            Text {
                                width: parent.width
                                text: (modelData.tier || "") + "  " + (modelData.trust || "") + "  " + (modelData.state || "")
                                color: Tokens.textFaint
                                font.family: Tokens.fontMono
                                font.pixelSize: Tokens.fontCaption
                                elide: Text.ElideRight
                            }
                        }
                    }
                }
            }

            Column {
                width: parent.width
                spacing: Tokens.spaceMd
                visible: ShellState.systemOpen

                StatusLine { label: "Network"; value: Network.online ? Network.iface : "Offline"; active: Network.online }
                StatusLine { label: "Audio"; value: Audio.ready ? (Audio.muted ? "Muted" : Math.round(Audio.volume * 100) + "%") : "Unavailable"; active: Audio.ready && !Audio.muted }
                StatusLine { label: "Bluetooth"; value: Bluetooth.available ? (Bluetooth.enabled ? "On" : "Off") : "No adapter"; active: Bluetooth.available && Bluetooth.enabled }
                StatusLine { label: "Focus"; value: Notifications.dnd ? "On" : "Off"; active: Notifications.dnd }
            }
        }
    }

    PanelSlot {
        id: popoutsWrapper
        x: 0
        y: Math.max(0, Math.min(ShellState.railPopoutY - height / 2, parent.height - height))
        width: 250
        height: popoutBody.implicitHeight + 2 * Tokens.spaceMd
        open: root.active && ShellState.railPopoutOpen
        edge: "left"

        Behavior on y { NumberAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }

        Column {
            id: popoutBody
            anchors.fill: parent
            anchors.margins: Tokens.spaceMd
            spacing: Tokens.spaceXs

            Text {
                width: parent.width
                text: ShellState.railPopoutName === "system" ? "System"
                    : ShellState.railPopoutName === "work" ? "Work"
                    : ShellState.railPopoutName === "clock" ? Qt.formatDateTime(new Date(), "dddd, MMM d")
                    : ShellState.railPopoutName === "theme" ? "Theme"
                    : ShellState.railPopoutName === "power" ? "Session"
                    : "Shell"
                color: Tokens.text
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontBody
                font.bold: true
                elide: Text.ElideRight
            }
            Text {
                width: parent.width
                text: ShellState.railPopoutName === "work"
                    ? root.sessionCount + " isolated session" + (root.sessionCount === 1 ? "" : "s")
                    : ShellState.railPopoutName === "system"
                      ? (Network.online ? "Online on " + Network.iface : "Offline")
                    : "Context routed through the bar wrapper."
                color: Tokens.textDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
                elide: Text.ElideRight
            }
        }
    }

    PanelSlot {
        id: edgeDock
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        width: 58
        height: 146
        open: root.active && ShellState.rightEdgeHot
        edge: "right"

        Column {
            anchors.centerIn: parent
            spacing: Tokens.spaceSm

            Rectangle { width: 34; height: 34; radius: Tokens.radiusFull; color: Tokens.accentDim; border.color: Tokens.border }
            Rectangle { width: 34; height: 34; radius: Tokens.radiusFull; color: Tokens.surface; border.color: Tokens.border }
            Rectangle { width: 34; height: 34; radius: Tokens.radiusFull; color: Tokens.surface; border.color: Tokens.border }
        }
    }

    component StatusLine: Item {
        property string label: ""
        property string value: ""
        property bool active: false

        width: parent ? parent.width : 260
        height: 38

        Column {
            anchors.left: parent.left
            anchors.verticalCenter: parent.verticalCenter
            Text { text: label; color: Tokens.textDim; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontSmall }
            Text { text: value; color: Tokens.text; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontBody }
        }
        Rectangle {
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            width: 10
            height: 10
            radius: 5
            color: active ? Tokens.accent : Tokens.textFaint
        }
    }

    component PanelSlot: Item {
        property bool open: false
        property string edge: ""
        property real progress: open ? 1 : 0
        readonly property real offsetScale: 1 - progress

        visible: progress > 0.001
        opacity: progress
        scale: 0.985 + 0.015 * progress

        transform: Translate {
            x: edge === "right" ? (1 - progress) * 22 : edge === "left" ? -(1 - progress) * 14 : 0
            y: edge === "top" ? -(1 - progress) * 16 : edge === "bottom" ? (1 - progress) * 16 : 0
        }

        Behavior on progress { NumberAnimation { duration: Tokens.animMed; easing.type: Easing.OutCubic } }
        Behavior on opacity { NumberAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }
    }
}
