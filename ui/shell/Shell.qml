import Quickshell
import Quickshell.Io
import QtQuick
import "../providers"
import "../services"
import "../state"
import "../themes"
import "../surfaces/desktop"
import "../surfaces/frame"
import "../surfaces/bar"
import "../surfaces/dashboard"
import "../surfaces/dock"
import "../surfaces/launcher"
import "../surfaces/clipboard"
import "../surfaces/work"
import "../surfaces/system"
import "../surfaces/notifications"
import "../surfaces/osd"
import "../surfaces/menu"
import "../surfaces/popout"

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

    // The output that follows focus, used by the single-instance overlays so a drawer/launcher/menu opens
    // on the monitor you're working on rather than being stranded on output 0. Falls back to the first
    // screen before the Sway IPC reports a focused monitor (and on single-monitor / headless preview).
    readonly property var activeScreen: Sway.focusedScreen
        || (Quickshell.screens.length > 0 ? Quickshell.screens[0] : null)

    // Per-monitor surfaces — one Bar and one Desktop backdrop on every screen (Variants instantiates the
    // delegate once per Quickshell.screens entry and injects `modelData` = that ShellScreen).
    // Rounded desktop frame — created FIRST so it layers under the shell surfaces (and over windows).
    Variants {
        model: Quickshell.screens
        ScreenFrame {
            required property var modelData
            screen: modelData
            activeScreen: shell.activeScreen
            session: sessionProvider
        }
    }
    Variants {
        model: Quickshell.screens
        ShellExclusions {
            required property var modelData
            screen: modelData
        }
    }
    Variants {
        model: Quickshell.screens
        Desktop {
            required property var modelData
            screen: modelData
        }
    }

    // Single-instance overlays — rendered on the focused output.
    Launcher { screen: shell.activeScreen }
    ClipboardPicker { screen: shell.activeScreen }
    WorkDrawer { provider: sessionProvider; screen: shell.activeScreen }
    SystemDrawer { screen: shell.activeScreen }
    Dashboard { provider: sessionProvider; screen: shell.activeScreen }
    QuickDock { screen: shell.activeScreen }
    RailPopout { provider: sessionProvider; screen: shell.activeScreen }
    Toasts { screen: shell.activeScreen }
    Osd { screen: shell.activeScreen }
    ContextMenu { screen: shell.activeScreen }

    // IPC seam for Sway keybinds. `system` is wired here so Super+S is inert-safe until the SYSTEM
    // drawer lands; its toggle just flips ShellState (no surface renders it yet).
    IpcHandler { target: "launcher";  function toggle(): void { ShellState.toggleLauncher() } }
    IpcHandler { target: "work";      function toggle(): void { ShellState.toggleWork() } }
    IpcHandler { target: "system";    function toggle(): void { ShellState.toggleSystem() } }
    IpcHandler { target: "clipboard"; function toggle(): void { ShellState.toggleClipboard() } }
    IpcHandler { target: "dashboard"; function toggle(): void { ShellState.toggleDashboard() } }
    IpcHandler {
        target: "railpopout"
        function open(name: string, y: real): void { ShellState.openRailPopout(name, y) }
        function close(): void { ShellState.closeRailPopout() }
    }
    // Screenshot (Super+Print / Print via Sway binds, or the context menu). region() drops slurp then
    // grim; screen() captures the whole output. Both save + copy + notify via the Screenshot service.
    IpcHandler {
        target: "screenshot"
        function region(): void { Screenshot.region() }
        function screen(): void { Screenshot.screen() }
    }
    // `menu open` drops the root context menu just under the bar — for the Super+M keybind and for
    // scripting/preview (right-click builds it with a cursor position instead).
    IpcHandler { target: "menu";     function open(): void { ShellState.openMenu(Tokens.railWidth + 2 * Tokens.spaceSm, 16, Menus.root()) } }

    // Load marker the desktop smoke test greps for.
    Component.onCompleted: console.log("SHREK-DESKTOP shell surfaces instantiated")
}
