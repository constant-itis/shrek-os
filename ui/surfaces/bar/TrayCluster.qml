import QtQuick
import "../../themes"
import "../../services"

// TrayCluster — system-tray icons in the bar (StatusNotifier items via the Tray service). Left-click =
// primary activate, middle-click = secondary activate, right-click (or left-click on a menu-only item) =
// the item's own platform menu popped by Quickshell's display(), scroll = the item's scroll action.
// Hidden entirely when no app has registered a tray icon. Presentation + the item's own actions only —
// no authority is read or minted here.
Row {
    id: cluster

    // The bar PanelWindow, passed from Bar.qml — the parent window the platform menu anchors to.
    property var window

    spacing: Tokens.spaceSm
    visible: Tray.count > 0

    Repeater {
        model: Tray.model

        delegate: Item {
            width: 18
            height: 18
            anchors.verticalCenter: parent ? parent.verticalCenter : undefined

            Image {
                anchors.fill: parent
                source: modelData && modelData.icon ? ("" + modelData.icon) : ""
                sourceSize.width: 18
                sourceSize.height: 18
                smooth: true
                asynchronous: true
                fillMode: Image.PreserveAspectFit
            }

            MouseArea {
                id: ma
                anchors.fill: parent
                acceptedButtons: Qt.LeftButton | Qt.RightButton | Qt.MiddleButton
                hoverEnabled: true
                cursorShape: Qt.PointingHandCursor

                function popMenu() {
                    if (!modelData.hasMenu) { modelData.activate(); return }
                    var p = ma.mapToItem(null, 0, ma.height)
                    modelData.display(cluster.window, p.x, p.y)
                }

                onClicked: (m) => {
                    if (m.button === Qt.LeftButton)
                        modelData.onlyMenu ? popMenu() : modelData.activate()
                    else if (m.button === Qt.MiddleButton)
                        modelData.secondaryActivate()
                    else if (m.button === Qt.RightButton)
                        popMenu()
                }
                onWheel: (w) => modelData.scroll(w.angleDelta.y, false)
            }
        }
    }
}
