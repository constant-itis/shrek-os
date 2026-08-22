pragma Singleton
import QtQuick
import Quickshell.I3
import Quickshell.Wayland

// Sway — the compositor read model, behind ONE seam. Surfaces import this, never Quickshell.I3 /
// Quickshell.Wayland directly, so the Sway/Quickshell dependency stays isolated (and a later compositor
// swap touches one file). Backed by Quickshell's native i3/Sway IPC backend (event-driven, no swaymsg
// text parsing) + wlr-foreign-toplevel for the focused window. Read + ordinary dispatch only; no
// authority here.
QtObject {
    // Live ObjectModel of I3Workspace { name, number, active, focused, urgent, activate() }.
    readonly property var workspaces: I3.workspaces
    readonly property var focusedWorkspace: I3.focusedWorkspace

    // Outputs (multi-monitor). I3Monitor { name, focused, rect, screen, activeWorkspace } where `screen`
    // is the matching Quickshell ShellScreen. `focusedScreen` is the ShellScreen the active output maps to
    // (null until the compositor IPC connects) — surfaces that should follow focus target it.
    readonly property var monitors: I3.monitors
    readonly property var focusedMonitor: I3.focusedMonitor
    readonly property var focusedScreen: I3.focusedMonitor ? I3.focusedMonitor.screen : null

    // Focused window (wlr-foreign-toplevel): Toplevel { title, appId, activated }, or null.
    readonly property var activeToplevel: ToplevelManager.activeToplevel

    // Live ObjectModel of ALL open windows (wlr-foreign-toplevel): Toplevel { title, appId, activated,
    // activate(), close() }. Drives the bar's window list / taskbar.
    readonly property var toplevels: ToplevelManager.toplevels

    // Ordinary compositor command (e.g. switch workspace). Not an authority operation.
    function dispatch(cmd) { I3.dispatch(cmd) }
}
