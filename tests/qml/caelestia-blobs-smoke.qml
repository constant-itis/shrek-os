import Quickshell
import Quickshell.Wayland
import QtQuick
import Caelestia.Blobs

ShellRoot {
    PanelWindow {
        anchors { top: true; bottom: true; left: true; right: true }
        color: "transparent"
        WlrLayershell.layer: WlrLayer.Top
        mask: Region {}

        BlobGroup {
            id: group
            color: "#224c26"
            smoothing: 24
        }

        BlobInvertedRect {
            anchors.fill: parent
            group: group
            radius: 18
            borderLeft: 64
            borderRight: 16
            borderTop: 16
            borderBottom: 16
        }

        BlobRect {
            group: group
            x: 64
            y: 64
            width: 180
            height: 96
            radius: 18
            deformScale: 0.0005
        }
    }

    Component.onCompleted: console.log("SHREK-BLOBS-SMOKE loaded")
}
