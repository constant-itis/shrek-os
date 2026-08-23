import Quickshell
import Quickshell.Wayland
import QtQuick
import "../config"
import "../state"

// Rail — one per screen. A left-anchored vertical bar that RESERVES its width: exactly three connected
// anchors (left+top+bottom) trigger ExclusionMode.Auto, which reserves precisely implicitWidth. Fully
// interactive (default mask = whole window clickable). The trigger toggles the attached panel; every
// module highlights on hover.
PanelWindow {
    anchors { left: true; top: true; bottom: true }
    implicitWidth: Config.railWidth
    color: Swatch.bg

    Component.onCompleted: if (this.WlrLayershell != null) this.WlrLayershell.layer = WlrLayer.Top

    Column {
        anchors.top: parent.top
        anchors.topMargin: Config.gap
        anchors.horizontalCenter: parent.horizontalCenter
        spacing: Config.gap

        // Panel trigger — click toggles; hover highlights.
        Rectangle {
            width: 36; height: 36; radius: 18
            color: (trig.containsMouse || UI.panelOpen) ? Swatch.accent : Swatch.surface
            Behavior on color { ColorAnimation { duration: Config.animMs } }
            Text {
                anchors.centerIn: parent
                text: "≡"   // ≡
                font.pixelSize: 20
                color: (trig.containsMouse || UI.panelOpen) ? Swatch.bg : Swatch.text
            }
            MouseArea {
                id: trig
                anchors.fill: parent
                hoverEnabled: true
                onClicked: UI.togglePanel()
            }
        }

        // Placeholder modules — prove hover state + rail composition (no services yet).
        Repeater {
            model: ["◐", "◔", "◑"]   // ◐ ◔ ◑
            Rectangle {
                required property string modelData
                width: 36; height: 36; radius: 18
                color: mod.containsMouse ? Swatch.border : Swatch.surface
                Behavior on color { ColorAnimation { duration: Config.animMs } }
                Text { anchors.centerIn: parent; text: parent.modelData; font.pixelSize: 16; color: Swatch.textDim }
                MouseArea { id: mod; anchors.fill: parent; hoverEnabled: true }
            }
        }
    }
}
