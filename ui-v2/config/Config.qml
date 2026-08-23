pragma Singleton
import QtQuick

// Config — LAYOUT geometry + motion tokens (px / ms) for shell-v2 composition. Colour + typography live
// in the semantic theme system (theme/Tokens.qml -> Theme.c); this file owns only the shell's spatial
// contract (rail/panel/work widths, frame insets, animation duration).
QtObject {
    // Bounded shell layout contract. Defaults preserve the stronger top-bar desktop direction while
    // surfaces/widgets consume these roles instead of assuming any permanent rail position.
    readonly property string panelEdge: "top"         // left | right | top | bottom
    readonly property string panelMode: "docked"      // docked | floating
    readonly property bool panelAutohide: false
    readonly property string monitorMode: "all"       // all | primary
    readonly property bool compactBar: true
    readonly property var barWidgets: ["panel", "work", "divider", "workspaces"]
    readonly property var statusWidgets: ["network", "audio", "bluetooth", "power", "windows", "time"]

    readonly property int railWidth: 56
    readonly property int railLength: 56
    readonly property int frameMargin: 10
    readonly property int frameRadius: 16
    readonly property int frameBorder: 0
    readonly property int panelWidth: 320
    readonly property int systemWidth: 520
    readonly property int workWidth: 360
    readonly property int gap: 10
    readonly property int animMs: 180

    readonly property bool edgeVertical: panelEdge === "left" || panelEdge === "right"
    readonly property bool edgeLeading: panelEdge === "left" || panelEdge === "top"
    readonly property bool floatingPanel: panelMode === "floating"
    readonly property bool reserveBarSpace: !floatingPanel && !panelAutohide

    function isValidEdge(edge) {
        return edge === "left" || edge === "right" || edge === "top" || edge === "bottom"
    }

    function appliesToScreen(index) {
        return monitorMode === "all" || index === 0
    }
}
