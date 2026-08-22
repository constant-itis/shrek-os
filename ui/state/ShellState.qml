pragma Singleton
import QtQuick

// ShellState — global UI state for the shell (Desktop Slice 1). Which drawer/overlay is open. This is
// presentation state only: it holds NO authority and reads NO gatekeeperd records. One drawer is open
// at a time (opening one closes the others) so the surfaces never fight for the screen.
QtObject {
    property bool launcherOpen: false
    property bool workOpen:     false
    property bool systemOpen:   false

    function _closeAll() { launcherOpen = false; workOpen = false; systemOpen = false }

    function toggleLauncher() { var v = !launcherOpen; _closeAll(); launcherOpen = v }
    function toggleWork()     { var v = !workOpen;     _closeAll(); workOpen = v }
    function toggleSystem()   { var v = !systemOpen;   _closeAll(); systemOpen = v }
    function closeAll()       { _closeAll() }
}
