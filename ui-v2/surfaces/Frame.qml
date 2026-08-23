import Quickshell
import Quickshell.Wayland
import QtQuick
import "../config"
import "../theme"

// Frame — one per screen. A full-screen, fully click-through overlay on the Background layer that draws
// the inset rounded desktop border. ExclusionMode.Ignore so it neither reserves space nor is displaced
// by other layer surfaces; an empty Region mask makes every pixel pass clicks through to the windows
// below. (PanelWindow.color defaults to opaque white in v0.3.1 — it MUST be set transparent.)
PanelWindow {
    anchors { top: true; bottom: true; left: true; right: true }
    exclusionMode: ExclusionMode.Ignore
    color: "transparent"
    mask: Region {}   // empty region => whole surface is click-through

    // Background layer: under windows, over the wallpaper.
    Component.onCompleted: if (this.WlrLayershell != null) this.WlrLayershell.layer = WlrLayer.Background

    Rectangle {
        anchors.fill: parent
        anchors.margins: Config.frameMargin
        color: "transparent"
        radius: Config.frameRadius
        border.width: Config.frameBorder
        border.color: Tokens.outline
    }
}
