pragma Singleton
import QtQuick

// UI — transient shell UI state (which drawers are open). Read-only surfaces bind to these flags; the
// IPC seam in Shell.qml flips them (Sway keybinds / preview drive the same path).
QtObject {
    property bool panelOpen: false
    function togglePanel(): void { panelOpen = !panelOpen }

    // Work drawer (the hero surface) — effective-authority view of live agent sessions.
    property bool workOpen: false
    function toggleWork(): void { workOpen = !workOpen }
}
