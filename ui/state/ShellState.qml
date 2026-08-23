pragma Singleton
import QtQuick
import "../themes"

// ShellState — global UI state for the shell (Desktop Slice 1). Which drawer/overlay is open. This is
// presentation state only: it holds NO authority and reads NO gatekeeperd records. One drawer is open
// at a time (opening one closes the others) so the surfaces never fight for the screen.
QtObject {
    property bool launcherOpen:  false
    property bool workOpen:      false
    property bool systemOpen:    false
    property bool clipboardOpen: false
    property bool dashboardOpen: false

    // Right-click context menu. menuItems is an array of { label, danger?, separator?, action },
    // opened at (menuX, menuY) in screen coordinates. Rendered by surfaces/menu/ContextMenu.qml.
    property bool menuOpen:  false
    property real menuX:     0
    property real menuY:     0
    property var  menuItems: []

    // Rail hover popout. Presentation-only context routed from rail items; no actions are executed here.
    property bool railPopoutOpen: false
    property string railPopoutName: ""
    property real railPopoutY: 0

    // Edge interaction plane. Presentation-only hover state; the plane owns narrow hit regions while the
    // visual/content surfaces remain separate and the desktop centre stays pass-through.
    property bool rightEdgeHot: false

    function _closeAll() {
        launcherOpen = false; workOpen = false; systemOpen = false; clipboardOpen = false
        dashboardOpen = false; menuOpen = false; railPopoutOpen = false; rightEdgeHot = false
    }

    function toggleLauncher()  { var v = !launcherOpen;  _closeAll(); launcherOpen = v }
    function toggleWork()      { var v = !workOpen;      _closeAll(); workOpen = v }
    function toggleSystem()    { var v = !systemOpen;    _closeAll(); systemOpen = v }
    function toggleClipboard() { var v = !clipboardOpen; _closeAll(); clipboardOpen = v }
    function toggleDashboard() { var v = !dashboardOpen; _closeAll(); dashboardOpen = v }
    function openMenu(x, y, items) { _closeAll(); menuX = x; menuY = y; menuItems = items; menuOpen = true }
    function closeAll()       { _closeAll() }
    function openRailPopout(name, y) {
        if (launcherOpen || workOpen || systemOpen || clipboardOpen || dashboardOpen || menuOpen)
            return
        railPopoutName = name
        railPopoutY = y
        railPopoutOpen = true
    }
    function closeRailPopout(name) {
        if (name === undefined || railPopoutName === name)
            railPopoutOpen = false
    }
    function openRightEdge() {
        if (launcherOpen || workOpen || systemOpen || clipboardOpen || dashboardOpen || menuOpen)
            return
        rightEdgeHot = true
    }
    function closeRightEdge() { rightEdgeHot = false }

    // Theme control routed through state (surfaces never touch the Theme controller directly — the
    // check-tokens gate enforces that). Cycles the appearance mode: dynamic -> dark -> light -> high-contrast.
    function cycleTheme() { Theme.cycleAppearance() }
}
