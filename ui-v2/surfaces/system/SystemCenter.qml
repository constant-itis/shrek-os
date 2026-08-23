import QtQuick
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
        width: 132
        spacing: Tokens.spaceXs

        Text {
            width: parent.width
            text: "System"
            color: Tokens.textPrimary
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontHeadline
            font.bold: true
        }
        Text {
            width: parent.width
            text: Network.online ? "Online" : (Network.available ? "Disconnected" : "NM absent")
            color: Tokens.textSecondary
            font.family: Tokens.fontFamily
            font.pixelSize: Tokens.fontCaption
            elide: Text.ElideRight
        }

        Item { width: 1; height: Tokens.spaceSm }

        Repeater {
            model: root.sections
            Rectangle {
                required property var modelData
                width: nav.width
                height: 34
                radius: Tokens.radius
                color: UI.systemSection === modelData.id ? Tokens.accentDim : (hover.containsMouse ? Tokens.surfaceRaised : "transparent")
                border.width: UI.systemSection === modelData.id ? 1 : 0
                border.color: Tokens.outline
                Text {
                    anchors.left: parent.left
                    anchors.leftMargin: Tokens.spaceSm
                    anchors.verticalCenter: parent.verticalCenter
                    width: parent.width - 2 * Tokens.spaceSm
                    text: modelData.label
                    color: UI.systemSection === modelData.id ? Tokens.textPrimary : Tokens.textSecondary
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontBody
                    elide: Text.ElideRight
                }
                MouseArea {
                    id: hover
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: UI.openSystem(modelData.id)
                }
            }
        }
    }

    Rectangle {
        anchors.left: nav.right
        anchors.leftMargin: Tokens.spaceMd
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        width: 1
        color: Tokens.outline
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
