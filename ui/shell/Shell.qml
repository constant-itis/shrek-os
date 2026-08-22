import Quickshell
import Quickshell.Io
import QtQuick
import "../providers"
import "../state"
import "../surfaces/desktop"
import "../surfaces/bar"
import "../surfaces/launcher"
import "../surfaces/work"
import "../surfaces/system"
import "../surfaces/notifications"
import "../surfaces/osd"

// Shell.qml — composition root (loaded by the config-folder entry ui/shell.qml).
//
// Instantiates the single read-only session data seam and mounts the shell surfaces. Sway keybinds
// toggle drawers by asking Quickshell over its IPC socket (`quickshell ipc call <target> toggle`), so
// keybinds and shell state stay in one place. Nothing here reads or mints authority — the shell is
// presentation/orchestration, not an authority root.
ShellRoot {
    id: shell

    // Real read-only session view: reads gatekeeperd-authored shrek-session/1 records. UNCHANGED from
    // Phase-8 Slice-1 — the proven read-only Work path.
    SessionProvider { id: sessionProvider }

    // Surfaces
    Desktop {}
    Bar { session: sessionProvider }
    Launcher {}
    WorkDrawer { provider: sessionProvider }
    SystemDrawer {}
    Toasts {}
    Osd {}

    // IPC seam for Sway keybinds. `system` is wired here so Super+S is inert-safe until the SYSTEM
    // drawer lands; its toggle just flips ShellState (no surface renders it yet).
    IpcHandler { target: "launcher"; function toggle(): void { ShellState.toggleLauncher() } }
    IpcHandler { target: "work";     function toggle(): void { ShellState.toggleWork() } }
    IpcHandler { target: "system";   function toggle(): void { ShellState.toggleSystem() } }

    // Load marker the desktop smoke test greps for.
    Component.onCompleted: console.log("SHREK-DESKTOP shell surfaces instantiated")
}
