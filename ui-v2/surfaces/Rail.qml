import Quickshell
import Quickshell.Wayland
import QtQuick
import "../components"
import "../config"
import "../state"
import "../theme"
import "../services"

// Edge bar. Current default is the historical left compact rail, but geometry and widget ordering come
// from Config so ordinary widgets do not encode that layout as permanent.
PanelWindow {
    id: rail

    readonly property bool vertical: Config.edgeVertical
    readonly property bool leading: Config.edgeLeading
    property string clockText: ""

    anchors {
        left: Config.panelEdge === "left" || !rail.vertical
        right: Config.panelEdge === "right" || !rail.vertical
        top: Config.panelEdge === "top" || rail.vertical
        bottom: Config.panelEdge === "bottom" || rail.vertical
    }

    implicitWidth: rail.vertical ? Config.railWidth : 1
    implicitHeight: rail.vertical ? 1 : Config.railWidth
    exclusionMode: Config.reserveBarSpace ? ExclusionMode.Auto : ExclusionMode.Ignore
    color: "transparent"

    Component.onCompleted: if (this.WlrLayershell != null) this.WlrLayershell.layer = WlrLayer.Top

    Rectangle {
        anchors.fill: parent
        anchors.leftMargin: rail.vertical ? Config.gap : Tokens.spaceLg
        anchors.rightMargin: rail.vertical ? Config.gap : Tokens.spaceLg
        anchors.topMargin: rail.vertical ? Tokens.spaceLg : Config.gap
        anchors.bottomMargin: rail.vertical ? Tokens.spaceLg : Config.gap
        radius: Tokens.radiusLg
        color: Tokens.surface
        border.width: 1
        border.color: Tokens.outline
    }

    Timer {
        interval: 15000
        running: true
        repeat: true
        triggeredOnStart: true
        onTriggered: {
            var d = new Date()
            rail.clockText = Qt.formatTime(d, "h:mm")
        }
    }

    function activateWidget(id) {
        if (id === "panel")
            UI.openControl("overview")
        else if (id === "work")
            UI.toggleWork()
    }

    function widgetLabel(id) {
        if (id === "panel")
            return "≡"
        if (id === "work")
            return "W"
        return id
    }

    function widgetActive(id) {
        if (id === "panel")
            return UI.panelOpen
        if (id === "work")
            return UI.workOpen
        return false
    }

    function openStatus(id) {
        if (id === "network")
            UI.openControl("network")
        else if (id === "audio")
            UI.openControl("overview")
        else if (id === "bluetooth")
            UI.openControl("overview")
        else if (id === "power")
            UI.openControl("overview")
        else if (id === "windows")
            UI.openSystem("system")
        else if (id === "time")
            UI.openControl("overview")
    }

    function statusVisible(id) {
        if (id === "audio")
            return Audio.ready
        if (id === "bluetooth")
            return Bluetooth.available
        if (id === "power")
            return Power.present
        if (id === "windows")
            return CompositorService.windowCount > 0
        return true
    }

    function statusLabel(id) {
        if (id === "network")
            return Network.online ? "net" : "--"
        if (id === "audio")
            return Audio.muted ? "mut" : "" + Math.round(Audio.volume * 100)
        if (id === "bluetooth")
            return "bt"
        if (id === "power")
            return "" + Math.round(Power.percentage)
        if (id === "windows")
            return "" + CompositorService.windowCount
        if (id === "time")
            return rail.clockText
        return id
    }

    function statusTint(id) {
        if (id === "network")
            return Network.online ? Tokens.accent : Tokens.muted
        if (id === "audio")
            return Audio.muted ? Tokens.warning : Tokens.textSecondary
        if (id === "bluetooth")
            return Bluetooth.enabled ? Tokens.textSecondary : Tokens.muted
        if (id === "power")
            return Power.onBattery ? Tokens.warning : Tokens.textSecondary
        return Tokens.textSecondary
    }

    component PrimaryStack: Column {
        width: Config.railWidth
        spacing: Config.gap

        Repeater {
            model: Config.barWidgets

            Loader {
                required property string modelData
                width: modelData === "workspaces" ? parent.width : (modelData === "divider" ? parent.width : (Config.compactBar ? 36 : 44))
                active: true
                anchors.horizontalCenter: parent.horizontalCenter
                sourceComponent: modelData === "divider" ? dividerComponent :
                                 modelData === "workspaces" ? workspacesComponent : buttonComponent
                property string widgetId: modelData
            }
        }
    }

    component PrimaryRow: Row {
        height: Config.railWidth
        spacing: Config.gap

        Repeater {
            model: Config.barWidgets

            Loader {
                required property string modelData
                height: modelData === "divider" ? 24 : 36
                width: modelData === "workspaces" ? implicitWidth : (modelData === "divider" ? 1 : (Config.compactBar ? 44 : 104))
                active: true
                anchors.verticalCenter: parent.verticalCenter
                sourceComponent: modelData === "divider" ? dividerComponent :
                                 modelData === "workspaces" ? workspacesComponent : buttonComponent
                property string widgetId: modelData
            }
        }
    }

    component StatusStack: Column {
        width: Config.railWidth
        spacing: Tokens.spaceXs

        Repeater {
            model: Config.statusWidgets

            ShrekStatusCell {
                required property string modelData
                visible: rail.statusVisible(modelData)
                label: rail.statusLabel(modelData)
                tint: rail.statusTint(modelData)
                compact: Config.compactBar
                vertical: rail.vertical
                onActivated: rail.openStatus(modelData)
            }
        }
    }

    component StatusRow: Row {
        height: Config.railWidth
        spacing: Tokens.spaceXs

        Repeater {
            model: Config.statusWidgets

            ShrekStatusCell {
                required property string modelData
                visible: rail.statusVisible(modelData)
                label: rail.statusLabel(modelData)
                tint: rail.statusTint(modelData)
                compact: Config.compactBar
                vertical: rail.vertical
                onActivated: rail.openStatus(modelData)
            }
        }
    }

    Component {
        id: buttonComponent

        ShrekBarButton {
            label: rail.widgetLabel(widgetId)
            active: rail.widgetActive(widgetId)
            compact: Config.compactBar
            vertical: rail.vertical
            anchors.horizontalCenter: rail.vertical ? parent.horizontalCenter : undefined
            anchors.verticalCenter: rail.vertical ? undefined : parent.verticalCenter
            onActivated: rail.activateWidget(widgetId)
        }
    }

    Component {
        id: dividerComponent

        ShrekDivider {
            width: rail.vertical ? 24 : 1
            height: rail.vertical ? 1 : 24
            anchors.horizontalCenter: rail.vertical ? parent.horizontalCenter : undefined
            anchors.verticalCenter: rail.vertical ? undefined : parent.verticalCenter
        }
    }

    Component {
        id: workspacesComponent

        Loader {
            sourceComponent: rail.vertical ? workspaceStackComponent : workspaceRowComponent
        }
    }

    Component {
        id: workspaceStackComponent

        Column {
            width: Config.railWidth
            spacing: Config.gap

            Repeater {
                model: CompositorService.workspaces

                ShrekBarButton {
                    required property var modelData
                    readonly property bool here: modelData.focused || modelData.active
                    label: "" + (modelData.number > 0 ? modelData.number : modelData.name)
                    active: here
                    compact: Config.compactBar
                    vertical: true
                    anchors.horizontalCenter: parent.horizontalCenter
                    border.width: modelData.urgent ? 2 : (here ? 1 : 0)
                    border.color: modelData.urgent ? Tokens.attention : Tokens.accent
                    onActivated: CompositorService.focusWorkspace(modelData.number)
                }
            }
        }
    }

    Component {
        id: workspaceRowComponent

        Row {
            height: Config.railWidth
            spacing: Config.gap

            Repeater {
                model: CompositorService.workspaces

                ShrekBarButton {
                    required property var modelData
                    readonly property bool here: modelData.focused || modelData.active
                    label: "" + (modelData.number > 0 ? modelData.number : modelData.name)
                    active: here
                    compact: Config.compactBar
                    vertical: false
                    anchors.verticalCenter: parent.verticalCenter
                    border.width: modelData.urgent ? 2 : (here ? 1 : 0)
                    border.color: modelData.urgent ? Tokens.attention : Tokens.accent
                    onActivated: CompositorService.focusWorkspace(modelData.number)
                }
            }
        }
    }

    PrimaryStack {
        visible: rail.vertical
        anchors.top: parent.top
        anchors.topMargin: Config.gap
        anchors.horizontalCenter: parent.horizontalCenter
    }

    PrimaryRow {
        visible: !rail.vertical
        anchors.left: parent.left
        anchors.leftMargin: Tokens.spaceXl
        anchors.verticalCenter: parent.verticalCenter
    }

    Text {
        visible: !rail.vertical
        anchors.centerIn: parent
        text: "shrek-os  ·  ~/projects/shrek-os"
        color: Tokens.textSecondary
        font.family: Tokens.fontFamily
        font.pixelSize: Tokens.fontSmall
        elide: Text.ElideRight
    }

    StatusStack {
        visible: rail.vertical
        anchors.bottom: parent.bottom
        anchors.bottomMargin: Config.gap
        anchors.horizontalCenter: parent.horizontalCenter
    }

    StatusRow {
        visible: !rail.vertical
        anchors.right: parent.right
        anchors.rightMargin: Tokens.spaceXl
        anchors.verticalCenter: parent.verticalCenter
    }
}
