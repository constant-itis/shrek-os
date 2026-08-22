import QtQuick
import "../../themes"
import "../../services"

// WindowList — the live taskbar: one pill per open window (wlr-foreign-toplevel via the Sway service).
// The focused window glows accent; left-click focuses it, middle-click closes it. This is the bar's
// "what's running / switch windows" affordance — the desktop's core operability made visible.
Row {
    id: root
    spacing: Tokens.spaceSm

    Repeater {
        model: Sway.toplevels

        Item {
            id: pill
            readonly property bool active: (modelData && modelData.activated === true)
            readonly property string label: modelData
                ? ((modelData.title && modelData.title.length > 0) ? modelData.title : (modelData.appId || "window"))
                : ""
            implicitWidth: Math.min(190, Math.max(96, txt.implicitWidth + 2 * Tokens.spaceMd + dot.width + Tokens.spaceSm))
            implicitHeight: 24

            Rectangle {
                anchors.fill: parent
                radius: Tokens.radiusSm
                color: pill.active ? Tokens.accentDim : (ma.containsMouse ? Tokens.surfaceAlt : Tokens.surface)
                border.width: 1
                border.color: pill.active ? Tokens.accent : Tokens.border
            }

            Row {
                anchors.fill: parent
                anchors.leftMargin: Tokens.spaceMd
                anchors.rightMargin: Tokens.spaceMd
                spacing: Tokens.spaceSm

                Rectangle {
                    id: dot
                    anchors.verticalCenter: parent.verticalCenter
                    width: 6; height: 6; radius: 3
                    color: pill.active ? Tokens.accentText : Tokens.textFaint
                }

                Text {
                    id: txt
                    anchors.verticalCenter: parent.verticalCenter
                    width: pill.width - 2 * Tokens.spaceMd - dot.width - Tokens.spaceSm
                    text: pill.label
                    color: pill.active ? Tokens.accentText : Tokens.textDim
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontSmall
                    elide: Text.ElideRight
                }
            }

            MouseArea {
                id: ma
                anchors.fill: parent
                hoverEnabled: true
                acceptedButtons: Qt.LeftButton | Qt.MiddleButton
                cursorShape: Qt.PointingHandCursor
                onClicked: (m) => {
                    if (m.button === Qt.MiddleButton) { if (modelData && modelData.close) modelData.close() }
                    else { if (modelData && modelData.activate) modelData.activate() }
                }
            }
        }
    }
}
