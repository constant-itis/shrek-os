pragma Singleton
import QtQuick

// UI — transient shell UI state (which drawers are open). Read-only surfaces bind to these flags; the
// IPC seam in Shell.qml flips them (Sway keybinds / preview drive the same path).
QtObject {
    property bool panelOpen: false
    property string systemSection: "overview"

    function closeMajor(): void {
        panelOpen = false
        workOpen = false
    }

    function openSystem(section) {
        workOpen = false
        systemSection = section || "overview"
        panelOpen = true
    }

    function togglePanel(): void {
        if (panelOpen) {
            panelOpen = false
        } else {
            workOpen = false
            panelOpen = true
        }
    }
    function toggleSystem(section): void {
        if (panelOpen && (!section || section === systemSection)) {
            panelOpen = false
        } else {
            openSystem(section || systemSection || "overview")
        }
    }

    // Work drawer (the hero surface) — effective-authority view of live agent sessions.
    property bool workOpen: false
    function toggleWork(): void {
        if (workOpen) {
            workOpen = false
        } else {
            panelOpen = false
            workOpen = true
        }
    }
}
