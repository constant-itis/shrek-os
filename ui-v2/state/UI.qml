pragma Singleton
import QtQuick

// UI — transient shell UI state. FIRST ACCEPTANCE only needs the one panel toggle.
QtObject {
    property bool panelOpen: false
    function togglePanel(): void { panelOpen = !panelOpen }
}
