import QtQuick
import "../../themes"

// Rail — Shrek's left spine content as an Item, so the Blob-backed shell content window can own the
// frame material and the rail controls together. Bar.qml keeps using this for compatibility, but G1 mounts
// it directly inside ScreenFrame to avoid a separate bar PanelWindow.
Item {
    id: rail

    property var session
    property var window

    implicitWidth: Tokens.railWidth

    // top cluster — identity/actions + theme toggle + workspaces
    Column {
        anchors { top: parent.top; horizontalCenter: parent.horizontalCenter; topMargin: Tokens.spaceMd }
        spacing: Tokens.spaceMd
        BarActions { anchors.horizontalCenter: parent.horizontalCenter }
        ThemeToggle { anchors.horizontalCenter: parent.horizontalCenter }
        Workspaces { anchors.horizontalCenter: parent.horizontalCenter }
    }

    // bottom cluster — system glances + tray + work + clock + power. Tray hides itself when empty, so
    // the common case is status + work + clock + power (no gap reserved).
    Column {
        anchors { bottom: parent.bottom; horizontalCenter: parent.horizontalCenter; bottomMargin: Tokens.spaceMd }
        spacing: Tokens.spaceMd
        StatusCluster { anchors.horizontalCenter: parent.horizontalCenter }
        TrayCluster { anchors.horizontalCenter: parent.horizontalCenter; window: rail.window }
        WorkPill { anchors.horizontalCenter: parent.horizontalCenter; session: rail.session }
        Clock { anchors.horizontalCenter: parent.horizontalCenter }
        PowerButton { anchors.horizontalCenter: parent.horizontalCenter }
    }
}
