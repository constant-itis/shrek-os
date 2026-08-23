import Quickshell
import Quickshell.Io
import QtQuick
import "../config"
import "../state"
import "../theme"
import "../services"
import "../providers"
import "../surfaces"

// Shell.qml — composition root for shell-v2.
//
// This slice mounts the load-bearing surfaces on the software backend (no GPU), against verified v0.3.1
// APIs, and connects the first real plumbing:
//   • per-screen layer surfaces        — Variants over Quickshell.screens
//   • a left rail that reserves space   — 3-anchor ExclusionMode.Auto, now showing LIVE Sway workspaces
//   • a central desktop frame           — Background layer, fully click-through (empty Region mask)
//   • an edge panel                     — animated open/close, mask tracks the card
//   • the WORK hero surface             — read-only effective-authority view of live agent sessions,
//                                          driven by the verbatim SessionProvider (the security seam)
//
// Colour flows through the semantic theme system (theme/Tokens). Nothing here reads or mints authority —
// the shell is presentation/orchestration; gatekeeperd authors the records the Work surface displays.
ShellRoot {
    id: root

    // The single read-only session data seam: reads gatekeeperd-authored shrek-session/1 records. Ported
    // VERBATIM from ui/providers/SessionProvider.qml (the security seam — unchanged).
    SessionProvider { id: sessionProvider }

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
    Variants {
        model: Quickshell.screens
        Work { required property var modelData; screen: modelData; provider: sessionProvider }
    }

    // Preview/keybind seam: toggle drawers over Quickshell's IPC socket
    // (`quickshell ipc call ui togglePanel true` / `... ui toggleWork true`). Functions must be fully typed.
    IpcHandler {
        target: "ui"
        function togglePanel(open: bool): void { UI.panelOpen = open }
        function toggle(): void { UI.togglePanel() }
        function panelState(): bool { return UI.panelOpen }
        function toggleWork(open: bool): void { UI.workOpen = open }
        function work(): void { UI.toggleWork() }
        function workState(): bool { return UI.workOpen }
    }

    // Load marker the smoke/session proofs grep for (shared with the Slice-1 desktop-session proof).
    Component.onCompleted: console.log("SHREK-DESKTOP shell surfaces instantiated")
}
