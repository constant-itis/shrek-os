import QtQuick
import Quickshell
import Quickshell.Io
import "../theme"

// Composition root for the installer. A single full-surface PanelWindow that shows one screen at a time.
// STATIC scaffold: the shown screen is selected by env (SHREK_INSTALLER_SCREEN) so the headless render
// harness can screenshot each; SHREK_INSTALLER_FAULT=1 drives the first-run degraded state. No navigation
// is wired — Back/Continue are visual only until the backend is connected.
ShellRoot {
    id: root
    property string screen: Quickshell.env("SHREK_INSTALLER_SCREEN") || "welcome"
    property bool fault: (Quickshell.env("SHREK_INSTALLER_FAULT") || "") === "1"

    PanelWindow {
        anchors { top: true; bottom: true; left: true; right: true }
        color: Tokens.background

        Loader {
            anchors.fill: parent
            sourceComponent: {
                switch (root.screen) {
                case "locale":   return cLocale
                case "name":     return cName
                case "disk":     return cDisk
                case "erase":    return cErase
                case "progress": return cProgress
                case "done":     return cDone
                case "firstrun": return cFirstrun
                default:         return cWelcome
                }
            }
        }

        Component { id: cWelcome;  Welcome {} }
        Component { id: cLocale;   LocaleKeymap {} }
        Component { id: cName;     OwnerName {} }
        Component { id: cDisk;     Disk {} }
        Component { id: cErase;    EraseConfirm {} }
        Component { id: cProgress; Progress {} }
        Component { id: cDone;     Done {} }
        Component { id: cFirstrun; OwnerEnroll { fault: root.fault } }

        Component.onCompleted: console.log("SHREK-INSTALLER surfaces instantiated: screen=" + root.screen + " fault=" + root.fault)
    }
}
