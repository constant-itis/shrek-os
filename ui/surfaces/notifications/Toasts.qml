import Quickshell
import Quickshell.Wayland
import Quickshell.Services.Notifications
import QtQuick
import "../../themes"
import "../../services"

// Toasts — the ATTENTION surface (Desktop Slice 1, Phase 4). A top-right stack of active notifications
// (the Notifications service tracked list). Each card auto-dismisses after a few seconds; click to
// dismiss now. Named Toasts (not Notifications) to avoid clashing with the service singleton. Only real
// notifications appear here -- no security nagging. Display + dismiss only.
PanelWindow {
    id: toasts
    WlrLayershell.layer: WlrLayer.Overlay
    anchors { top: true; right: true }
    implicitWidth: 360
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
                width: 340
                height: body.implicitHeight + 2 * Tokens.spaceMd
                radius: Tokens.radius
                color: Tokens.surface
                border.color: modelData.urgency === NotificationUrgency.Critical ? Tokens.danger : Tokens.border

                Column {
                    id: body
                    anchors { left: parent.left; right: parent.right; top: parent.top; margins: Tokens.spaceMd }
                    spacing: 2

                    Text {
                        width: parent.width
                        text: modelData.appName && modelData.appName.length > 0 ? modelData.appName : "Notification"
                        color: Tokens.textFaint
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
                    }
                }

                MouseArea {
                    anchors.fill: parent
                    cursorShape: Qt.PointingHandCursor
                    onClicked: modelData.dismiss()
                }

                Timer { interval: 5000; running: true; repeat: false; onTriggered: modelData.dismiss() }
            }
        }
    }
}
