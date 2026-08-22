import Quickshell
import Quickshell.Wayland
import QtQuick
import "../../themes"
import "../../state"
import "../../services"

// SystemDrawer — the SYSTEM zone (Desktop Slice 1, Phase 3). Quick controls over mature Linux services:
// audio (PipeWire), network state (read-only, systemd-networkd), Bluetooth (BlueZ), battery (UPower),
// and a power menu. Toggled with Super+S / the bar status cluster. Ordinary user actions only — no
// authority is read or minted here. Empty/absent hardware degrades honestly (no adapter, no battery).
PanelWindow {
    id: sys
    WlrLayershell.layer: WlrLayer.Top
    visible: ShellState.systemOpen
    anchors { top: true; right: true; bottom: true }
    implicitWidth: Tokens.drawerWidth
    color: Tokens.panelBg

    Rectangle {
        anchors { left: parent.left; top: parent.top; bottom: parent.bottom }
        width: 1
        color: Tokens.border
    }

    Column {
        id: top
        anchors { top: parent.top; left: parent.left; right: parent.right; margins: Tokens.spaceLg }
        spacing: Tokens.spaceLg

        Text {
            text: "System"
            color: Tokens.text
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontTitle
            font.bold: true
        }

        // ── Audio ──
        Column {
            width: parent.width
            spacing: Tokens.spaceSm
            visible: Audio.ready

            Item {
                width: parent.width
                height: labelA.height
                Text {
                    id: labelA
                    anchors.left: parent.left
                    text: "Volume"
                    color: Tokens.textDim
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontSmall
                }
                Text {
                    anchors.right: parent.right
                    text: Audio.muted ? "muted" : Math.round(Audio.volume * 100) + "%"
                    color: Audio.muted ? Tokens.notice : Tokens.text
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontSmall
                    MouseArea { anchors.fill: parent; anchors.margins: -6; cursorShape: Qt.PointingHandCursor; onClicked: Audio.toggleMute() }
                }
            }

            Rectangle {
                id: track
                width: parent.width
                height: 6
                radius: 3
                color: Tokens.surface

                Rectangle {
                    anchors { left: parent.left; top: parent.top; bottom: parent.bottom }
                    width: parent.width * Math.max(0, Math.min(1, Audio.volume))
                    radius: 3
                    color: Audio.muted ? Tokens.textFaint : Tokens.accent
                }
                MouseArea {
                    anchors.fill: parent
                    onPressed: (m) => Audio.setVolume(m.x / width)
                    onPositionChanged: (m) => Audio.setVolume(Math.max(0, Math.min(1, m.x / width)))
                }
            }
        }

        // ── Network (read-only link state) ──
        Item {
            width: parent.width
            height: 38
            Column {
                anchors.left: parent.left
                anchors.verticalCenter: parent.verticalCenter
                Text { text: "Network"; color: Tokens.textDim; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontSmall }
                Text {
                    text: Network.online ? (Network.iface + (Network.address.length > 0 ? "  " + Network.address : "")) : "Offline"
                    color: Tokens.text
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                }
            }
            Rectangle {
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                width: 10; height: 10; radius: 5
                color: Network.online ? Tokens.accent : Tokens.textFaint
            }
        }

        // ── Bluetooth ──
        Item {
            width: parent.width
            height: 38
            Column {
                anchors.left: parent.left
                anchors.verticalCenter: parent.verticalCenter
                Text { text: "Bluetooth"; color: Tokens.textDim; font.family: Tokens.fontFamily; font.pixelSize: Tokens.fontSmall }
                Text {
                    text: Bluetooth.available ? (Bluetooth.enabled ? "On" : "Off") : "No adapter"
                    color: Tokens.text
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                }
            }
            Rectangle {
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                visible: Bluetooth.available
                width: 44; height: 22; radius: 11
                color: Bluetooth.enabled ? Tokens.accent : Tokens.surface
                border.color: Tokens.border
                Rectangle {
                    width: 16; height: 16; radius: 8
                    anchors.verticalCenter: parent.verticalCenter
                    x: Bluetooth.enabled ? parent.width - width - 3 : 3
                    color: Bluetooth.enabled ? Tokens.accentText : Tokens.textDim
                    Behavior on x { NumberAnimation { duration: Tokens.animFast; easing.type: Easing.OutCubic } }
                }
                MouseArea { anchors.fill: parent; cursorShape: Qt.PointingHandCursor; onClicked: Bluetooth.toggle() }
            }
        }

        // ── Battery (only when a real battery is present) ──
        Item {
            width: parent.width
            height: 38
            visible: Power.present
            Text {
                anchors.left: parent.left
                anchors.verticalCenter: parent.verticalCenter
                text: "Battery"
                color: Tokens.textDim
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
            }
            Text {
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                text: Math.round(Power.percentage) + "%" + (Power.charging ? "  charging" : "")
                color: Tokens.text
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontBody
            }
        }
    }

    // ── Power menu (pinned to the bottom) ──
    Row {
        anchors { left: parent.left; right: parent.right; bottom: parent.bottom; margins: Tokens.spaceLg }
        spacing: Tokens.spaceSm

        component PowerBtn: Rectangle {
            property string label: ""
            property bool danger: false
            property var act
            width: (parent.width - 2 * Tokens.spaceSm) / 3
            height: 34
            radius: Tokens.radius
            color: Tokens.surface
            border.color: hover.containsMouse ? (danger ? Tokens.danger : Tokens.accent) : Tokens.border
            Text {
                anchors.centerIn: parent
                text: label
                color: danger && hover.containsMouse ? Tokens.danger : Tokens.text
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontSmall
            }
            MouseArea { id: hover; anchors.fill: parent; hoverEnabled: true; cursorShape: Qt.PointingHandCursor; onClicked: if (parent.act) parent.act() }
        }

        // Reboot/Off need root. The sealed image ships no polkit agent, so logind denies these to the
        // non-root dev user -- route through the dev user's passwordless sudo (sudo -n = non-interactive).
        // Log out exits Sway; in this single-user autologin VM (no greeter) the session just relaunches.
        PowerBtn { label: "Log out"; act: function () { ShellState.closeAll(); Sway.dispatch("exit") } }
        PowerBtn { label: "Reboot";  act: function () { Quickshell.execDetached(["sudo", "-n", "systemctl", "reboot"]) } }
        PowerBtn { label: "Off"; danger: true; act: function () { Quickshell.execDetached(["sudo", "-n", "systemctl", "poweroff"]) } }
    }
}
