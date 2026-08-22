import QtQuick
import "../../themes"
import "../../services"

// WindowTitle — the focused window's title (wlr-foreign-toplevel via the Sway service). Falls back to
// appId when the title is empty; blank when nothing is focused.
Text {
    id: root
    readonly property var tl: Sway.activeToplevel
    text: tl ? (tl.title && tl.title.length > 0 ? tl.title : (tl.appId || "")) : ""
    color: Tokens.textDim
    font.family: Tokens.fontFamily
    font.pixelSize: Tokens.fontSmall
    elide: Text.ElideRight
    horizontalAlignment: Text.AlignHCenter
}
