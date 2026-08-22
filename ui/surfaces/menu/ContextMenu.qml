import Quickshell
import Quickshell.Wayland
import QtQuick
import "../../themes"
import "../../state"

// ContextMenu — the right-click menu surface (Desktop Slice 1). Renders ShellState.menuItems as a small
// panel at the click point (clamped on-screen). Click an item to run its action; click-out or Esc
// dismisses. Presentation only — the actions it runs are built by the Menus singleton; no authority here.
PanelWindow {
    id: menu
    WlrLayershell.layer: WlrLayer.Overlay
    WlrLayershell.keyboardFocus: WlrKeyboardFocus.OnDemand
    visible: ShellState.menuOpen
    anchors { top: true; bottom: true; left: true; right: true }
    exclusiveZone: 0
    color: "transparent"

    readonly property var items: ShellState.menuItems
    readonly property int rowH: 30
    readonly property int sepH: 9
    readonly property int menuW: 224

    // click anywhere outside the panel dismisses
    MouseArea {
        anchors.fill: parent
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        onClicked: ShellState.closeAll()
    }

    // Esc dismiss (best-effort; click-out is the primary path)
    Item {
        anchors.fill: parent
        focus: ShellState.menuOpen
        Keys.onEscapePressed: ShellState.closeAll()
    }

    Rectangle {
        id: panel
        width: menu.menuW
        height: col.implicitHeight + 2 * Tokens.spaceXs
        // clamp the panel fully on-screen relative to the click point
        x: Math.max(Tokens.spaceSm, Math.min(ShellState.menuX, menu.width - width - Tokens.spaceSm))
        y: Math.max(Tokens.spaceSm, Math.min(ShellState.menuY, menu.height - height - Tokens.spaceSm))
        radius: Tokens.radius
        color: Tokens.panelBg
        border.color: Tokens.borderStrong
        border.width: 1

        // eat clicks inside the panel so they don't fall through to the dismiss scrim
        MouseArea { anchors.fill: parent; acceptedButtons: Qt.AllButtons }

        Column {
            id: col
            anchors { left: parent.left; right: parent.right; top: parent.top; margins: Tokens.spaceXs }
            spacing: 0

            Repeater {
                model: menu.items

                delegate: Item {
                    id: entry
                    width: col.width
                    readonly property bool isSep:  (modelData && modelData.separator === true)
                    readonly property bool danger: (modelData && modelData.danger === true)
                    height: entry.isSep ? menu.sepH : menu.rowH

                    // separator
                    Rectangle {
                        visible: entry.isSep
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.left: parent.left; anchors.right: parent.right
                        anchors.leftMargin: Tokens.spaceSm; anchors.rightMargin: Tokens.spaceSm
                        height: 1
                        color: Tokens.border
                    }

                    // action row
                    Rectangle {
                        visible: !entry.isSep
                        anchors.fill: parent
                        radius: Tokens.radiusSm
                        color: rowMa.containsMouse ? (entry.danger ? "#3a1e22" : Tokens.surfaceAlt) : "transparent"

                        Text {
                            anchors.left: parent.left
                            anchors.leftMargin: Tokens.spaceSm
                            anchors.verticalCenter: parent.verticalCenter
                            text: modelData ? (modelData.label || "") : ""
                            color: entry.danger ? Tokens.danger : Tokens.text
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontBody
                        }

                        MouseArea {
                            id: rowMa
                            anchors.fill: parent
                            hoverEnabled: true
                            cursorShape: Qt.PointingHandCursor
                            onClicked: {
                                var it = modelData
                                ShellState.closeAll()
                                if (it && it.action) it.action()
                            }
                        }
                    }
                }
            }
        }
    }
}
