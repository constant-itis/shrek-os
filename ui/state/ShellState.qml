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

    // Right-click context menu. menuItems is an array of { label, danger?, separator?, action },
    // opened at (menuX, menuY) in screen coordinates. Rendered by surfaces/menu/ContextMenu.qml.
    property bool menuOpen:  false
    property real menuX:     0
    property real menuY:     0
    property var  menuItems: []

    function _closeAll() {
        launcherOpen = false; workOpen = false; systemOpen = false; clipboardOpen = false; menuOpen = false
    }

    function toggleLauncher()  { var v = !launcherOpen;  _closeAll(); launcherOpen = v }
    function toggleWork()      { var v = !workOpen;      _closeAll(); workOpen = v }
    function toggleSystem()    { var v = !systemOpen;    _closeAll(); systemOpen = v }
    function toggleClipboard() { var v = !clipboardOpen; _closeAll(); clipboardOpen = v }
    function openMenu(x, y, items) { _closeAll(); menuX = x; menuY = y; menuItems = items; menuOpen = true }
    function closeAll()       { _closeAll() }

    // Theme control routed through state (surfaces never touch the Theme controller directly — the
    // check-tokens gate enforces that). Cycles the appearance mode: dynamic -> dark -> light -> high-contrast.
    function cycleTheme() { Theme.cycleAppearance() }
}
