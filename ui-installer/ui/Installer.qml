import QtQuick
import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import "../theme"
import "../state"

// Composition root for the installer. A single full-surface PanelWindow showing one screen at a time,
// LIVE-DRIVEN by the Intent singleton's linear flow (Back/Continue call Intent.back()/next()). The render
// harness still pins a single screen via SHREK_INSTALLER_SCREEN (jumped to at startup); firstrun is a
// separate surface reachable only via that env (it is the first-BOOT owner enroll, not part of the install
// flow). SHREK_INSTALLER_FAULT=1 drives the first-run degraded state.
ShellRoot {
    id: root
    property bool fault: (Quickshell.env("SHREK_INSTALLER_FAULT") || "") === "1"
    property bool firstrunMode: (Quickshell.env("SHREK_INSTALLER_SCREEN") || "") === "firstrun"

    Component.onCompleted: {
        var e = Quickshell.env("SHREK_INSTALLER_SCREEN") || ""
        if (e.length > 0 && e !== "firstrun") Intent.jumpTo(e)   // harness: pin a flow screen; real run starts at welcome
        console.log("SHREK-INSTALLER surfaces instantiated: screen=" + (root.firstrunMode ? "firstrun" : Intent.screen) + " fault=" + root.fault)
    }

    PanelWindow {
        anchors { top: true; bottom: true; left: true; right: true }
        color: Tokens.background
        // The installer is a layer-shell surface; without this it never receives keyboard input, so text
        // fields (owner name, keyboard test) silently ignore every keystroke while the mouse still works.
        // Exclusive grabs the keyboard for the full-screen installer — it is the only thing on screen.
        WlrLayershell.keyboardFocus: WlrKeyboardFocus.Exclusive

        Loader {
            anchors.fill: parent
            sourceComponent: {
                if (root.firstrunMode) return cFirstrun
                switch (Intent.screen) {
                case "locale":   return cLocale
                case "name":     return cName
                case "disk":     return cDisk
                case "erase":    return cErase
                case "progress": return cProgress
                case "done":     return cDone
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
    }
}
