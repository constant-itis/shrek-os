import QtQuick
import "../../components"
import "../../state"
import "../../theme"
import "../../services"

Item {
    id: root

    readonly property var sections: [
        { id: "overview", label: "Overview" },
        { id: "network", label: "Network" },
        { id: "audio", label: "Audio" },
        { id: "bluetooth", label: "Bluetooth" },
        { id: "power", label: "Power" },
        { id: "appearance", label: "Appearance" },
        { id: "system", label: "System" }
    ]

    Column {
        id: nav
        anchors.left: parent.left
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        width: 144
        spacing: Tokens.spaceXs

        ShrekCard {
            width: parent.width
            active: Network.online

            Text {
                width: parent.width
                text: "System"
                color: Tokens.textPrimary
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontHeadline
                font.weight: Font.DemiBold
                elide: Text.ElideRight
            }

            Text {
                width: parent.width
                text: Network.online ? "Online" : (Network.available ? "Disconnected" : "NM absent")
                color: Tokens.textSecondary
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontCaption
                elide: Text.ElideRight
            }
        }

        Item { width: 1; height: Tokens.spaceXs }

        Repeater {
            model: root.sections
            ShrekButton {
                required property var modelData
                width: nav.width
                text: modelData.label
                kind: UI.systemSection === modelData.id ? "primary" : "ghost"
                compact: true
                horizontalAlignment: Text.AlignLeft
                onActivated: UI.openSystem(modelData.id)
            }
        }
    }

    ShrekDivider {
        anchors.left: nav.right
        anchors.leftMargin: Tokens.spaceMd
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        width: 1
    }

    Loader {
        anchors.left: nav.right
        anchors.leftMargin: Tokens.spaceLg
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        sourceComponent: {
            if (UI.systemSection === "network") return networkPage
            if (UI.systemSection === "audio") return audioPage
            if (UI.systemSection === "bluetooth") return bluetoothPage
            if (UI.systemSection === "power") return powerPage
            if (UI.systemSection === "appearance") return appearancePage
            if (UI.systemSection === "system") return systemPage
            return overviewPage
        }
    }

    Component { id: overviewPage; OverviewPage {} }
    Component { id: networkPage; NetworkPage {} }
    Component { id: audioPage; AudioPage {} }
    Component { id: bluetoothPage; BluetoothPage {} }
    Component { id: powerPage; PowerPage {} }
    Component { id: appearancePage; AppearancePage {} }
    Component { id: systemPage; SystemPage {} }
}
