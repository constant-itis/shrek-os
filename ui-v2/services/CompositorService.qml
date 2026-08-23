pragma Singleton
import QtQuick
import Quickshell.I3
import Quickshell.Wayland

// CompositorService — the Sway read model, behind ONE seam. Surfaces import this, never Quickshell.I3 /
// Quickshell.Wayland directly, so the compositor dependency stays isolated in one file.
//
// Shrek assumes SWAY EXPLICITLY (sway.config drives the session). There is NO socket-owner / compositor
// auto-detection — that generality is deliberately out of scope. Workspace +
// dispatch come from Quickshell's native i3/Sway IPC backend (event-driven, Sway speaks the i3 protocol);
// generic window state comes from wlr-foreign-toplevel (ToplevelManager). Read + ordinary workspace
// dispatch only — NO authority here.
QtObject {
    // Live workspaces: ObjectModel of I3Workspace { name, number, active, focused, urgent, activate() }.
    readonly property var workspaces: I3.workspaces
    // The focused workspace (I3Workspace), or null before the IPC connects.
    readonly property var activeWorkspace: I3.focusedWorkspace

    // Generic window state via wlr-foreign-toplevel:
    //   windows       — ObjectModel of Toplevel { title, appId, activated, activate(), close() }
    //   activeWindow  — the focused Toplevel, or null
    readonly property var windows: ToplevelManager.toplevels
    readonly property var activeWindow: ToplevelManager.activeToplevel
    readonly property int windowCount: _modelCount(windows)

    // Workspace navigation (ordinary compositor commands — NOT authority operations).
    function focusWorkspace(n) { I3.dispatch("workspace number " + n) }
    function nextWorkspace()   { I3.dispatch("workspace next_on_output") }
    function prevWorkspace()   { I3.dispatch("workspace prev_on_output") }

    function _modelCount(model) {
        if (!model)
            return 0
        if (model.count !== undefined)
            return model.count
        if (model.length !== undefined)
            return model.length
        return 0
    }
}
