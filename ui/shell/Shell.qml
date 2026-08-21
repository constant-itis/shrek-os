import Quickshell
import QtQuick
import "../themes"
import "../providers"

// Shell.qml — Quickshell entry point (`quickshell -p .../ui/shell/Shell.qml`).
//
// Bootstrap-0: Quickshell owns ALL shell surfaces (Sway draws no bar). This root
// instantiates the bar, the launcher placeholder, and the Work-drawer host, and
// wires the single session data seam. Nothing here reads real authority.
ShellRoot {
    id: shell

    // Real read-only session view: reads gatekeeperd-authored shrek-session/1 records.
    SessionProvider { id: sessionProvider }

    Bar {}
    Launcher {}
    WorkDrawer { provider: sessionProvider }

    // Load marker the desktop smoke test greps for.
    Component.onCompleted: console.log("SHREK-DESKTOP shell surfaces instantiated")
}
