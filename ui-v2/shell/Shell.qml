import Quickshell
import Quickshell.Io
import QtQuick
import "../config"
import "../state"
import "../surfaces"

// Shell.qml — composition root for shell-v2 (FIRST ACCEPTANCE: geometry only, no backend services).
//
// This slice proves the load-bearing Quickshell/Sway primitives in isolation, on the software backend
// (no GPU), against verified v0.3.1 APIs:
//   • per-screen layer surfaces        — Variants over Quickshell.screens
//   • a left rail that reserves space   — 3-anchor ExclusionMode.Auto
//   • a central desktop frame           — Background layer, fully click-through (empty Region mask)
//   • one edge-attached panel           — animated open/close, mask tracks the card (input pass-through)
//
// No services, no Sway IPC, no authority. Those mount only AFTER this renders clean in preview AND in
// the sealed VM — the blank-VM staging failure must not survive into v2.
ShellRoot {
    id: root

    // Frame first so it stacks under the interactive surfaces (and, on the Background layer, under windows).
    Variants {
        model: Quickshell.screens
        Frame { required property var modelData; screen: modelData }
    }
    Variants {
        model: Quickshell.screens
        Rail { required property var modelData; screen: modelData }
    }
    Variants {
        model: Quickshell.screens
        Panel { required property var modelData; screen: modelData }
    }

    // Preview/keybind seam: toggle the panel over Quickshell's IPC socket
    // (`quickshell ipc call ui togglePanel true`). Functions must be fully typed to register.
    IpcHandler {
        target: "ui"
        function togglePanel(open: bool): void { UI.panelOpen = open }
        function toggle(): void { UI.togglePanel() }
        function panelState(): bool { return UI.panelOpen }
    }
}
