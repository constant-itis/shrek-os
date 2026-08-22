import Quickshell
import Quickshell.Wayland
import Quickshell.Services.Notifications
import QtQuick
import "../../themes"
import "../../services"

// Toasts — the ATTENTION surface. A top-right stack of active notifications (the Notifications service
// tracked list). Each card slides in, carries the sender's icon + action buttons, pauses its auto-dismiss
// while hovered, and closes on × or an action. Only real notifications appear here — no security nagging.
// Shrek IS the notification server; this is display + the sender's own actions, no authority.
PanelWindow {
    id: toasts
    WlrLayershell.layer: WlrLayer.Overlay
    anchors { top: true; right: true }
    implicitWidth: 372
    implicitHeight: Math.max(1, col.implicitHeight + 2 * Tokens.spaceMd)
    color: "transparent"
    exclusiveZone: 0
    visible: Notifications.list.length > 0

    Column {
        id: col
        anchors { top: parent.top; right: parent.right; margins: Tokens.spaceMd }
        spacing: Tokens.spaceSm

        Repeater {
            model: Notifications.list

            Rectangle {
                id: card
                readonly property bool critical: modelData.urgency === NotificationUrgency.Critical
                readonly property string iconSrc: {
                    if (modelData.image && ("" + modelData.image).length > 0) return "" + modelData.image
                    if (modelData.appIcon && ("" + modelData.appIcon).length > 0) return "image://icon/" + modelData.appIcon
                    return ""
                }
                width: 352
                height: body.implicitHeight + 2 * Tokens.spaceMd
                radius: Tokens.radius
                color: Tokens.surface
                border.color: critical ? Tokens.danger : Tokens.border

                // entrance: slide in from the right + fade
                property bool hovered: false
                opacity: 0
                transform: Translate { id: slide; x: 28 }
                Component.onCompleted: enter.start()
                ParallelAnimation {
                    id: enter
                    NumberAnimation { target: card; property: "opacity"; to: 1; duration: Tokens.animMed; easing.type: Easing.OutCubic }
                    NumberAnimation { target: slide; property: "x"; to: 0; duration: Tokens.animMed; easing.type: Easing.OutCubic }
                }

                // urgency accent stripe (calm — amber for critical, faint accent otherwise)
                Rectangle {
                    anchors { left: parent.left; top: parent.top; bottom: parent.bottom }
                    width: 3; radius: Tokens.radius
                    color: card.critical ? Tokens.danger : Tokens.accent
                    opacity: card.critical ? 0.9 : 0.5
                }

                MouseArea {
                    anchors.fill: parent
                    hoverEnabled: true
                    onEntered: card.hovered = true
                    onExited: card.hovered = false
                    acceptedButtons: Qt.NoButton   // hover only; clicks handled by × and action buttons
                }

                Row {
                    id: body
                    anchors { left: parent.left; right: parent.right; top: parent.top; margins: Tokens.spaceMd; leftMargin: Tokens.spaceMd + 4 }
                    spacing: Tokens.spaceMd

                    // sender icon (image or themed app icon); omitted when the sender gives neither
                    Image {
                        visible: card.iconSrc.length > 0
                        width: card.iconSrc.length > 0 ? 32 : 0
                        height: 32
                        source: card.iconSrc
                        sourceSize.width: 32; sourceSize.height: 32
                        smooth: true; asynchronous: true
                        fillMode: Image.PreserveAspectFit
                    }

                    Column {
                        width: body.width - (card.iconSrc.length > 0 ? 32 + Tokens.spaceMd : 0) - closeBtn.width - Tokens.spaceSm
                        spacing: 2

                        Text {
                            width: parent.width
                            text: modelData.appName && modelData.appName.length > 0 ? modelData.appName : "Notification"
                            color: card.critical ? Tokens.danger : Tokens.textFaint
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontCaption
                            elide: Text.ElideRight
                        }
                        Text {
                            width: parent.width
                            text: modelData.summary || ""
                            color: Tokens.text
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontBody
                            font.bold: true
                            wrapMode: Text.WordWrap
                            maximumLineCount: 2
                            elide: Text.ElideRight
                        }
                        Text {
                            width: parent.width
                            visible: ("" + (modelData.body || "")).length > 0
                            text: modelData.body || ""
                            color: Tokens.textDim
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                            wrapMode: Text.WordWrap
                            maximumLineCount: 4
                            elide: Text.ElideRight
                            textFormat: Text.PlainText
                        }

                        // the sender's action buttons (invoke → the app acts; then the toast closes)
                        Row {
                            visible: modelData.actions && modelData.actions.length > 0
                            spacing: Tokens.spaceSm
                            topPadding: 2
                            Repeater {
                                model: modelData.actions || []
                                Rectangle {
                                    height: 24
                                    width: actLabel.implicitWidth + 2 * Tokens.spaceMd
                                    radius: Tokens.radiusSm
                                    color: actMouse.containsMouse ? Tokens.accentDim : Tokens.surfaceAlt
                                    border.color: Tokens.border
                                    Text {
                                        id: actLabel
                                        anchors.centerIn: parent
                                        text: ("" + (modelData.text || "Action"))
                                        color: Tokens.text
                                        font.family: Tokens.fontFamily
                                        font.pixelSize: Tokens.fontCaption
                                    }
                                    MouseArea {
                                        id: actMouse
                                        anchors.fill: parent
                                        hoverEnabled: true
                                        cursorShape: Qt.PointingHandCursor
                                        onClicked: modelData.invoke()
                                    }
                                }
                            }
                        }
                    }

                    // close affordance
                    Rectangle {
                        id: closeBtn
                        width: 20; height: 20; radius: 999
                        color: closeMouse.containsMouse ? Tokens.surfaceAlt : "transparent"
                        Text {
                            anchors.centerIn: parent
                            text: "×"
                            color: closeMouse.containsMouse ? Tokens.text : Tokens.textFaint
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontTitle
                        }
                        MouseArea {
                            id: closeMouse
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: modelData.dismiss()
                        }
                    }
                }

                // auto-dismiss after a few seconds; PAUSES (and resets) while hovered. Critical
                // notifications persist until dismissed (never auto-close).
                Timer {
                    interval: 5000
                    running: !card.hovered && !card.critical
                    repeat: false
                    onTriggered: modelData.dismiss()
                }
            }
        }
    }
}
