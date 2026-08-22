pragma Singleton
import QtQuick
import Quickshell
import "../services"
import "../state"

// Menus — builders for the shell's right-click context menus (Desktop Slice 1). Returns plain item
// arrays ({ label, danger?, separator?, action }) that surfaces/menu/ContextMenu.qml renders, so the
// desktop and the bar reach the SAME menu. Ordinary user actions only — launch, compositor dispatch
// (Sway service), session — no authority is read or minted here.
QtObject {
    function terminal() { Quickshell.execDetached(["foot", "--font=DejaVu Sans Mono:size=11"]) }

    // Root/desktop menu. Includes the focused window's actions when a window is focused, so window
    // management is reachable by mouse without knowing a single keybind.
    function root() {
        var items = [
            { label: "Applications…",   action: function () { ShellState.toggleLauncher() } },
            { label: "Terminal",        action: function () { terminal() } },
            { label: "System settings", action: function () { ShellState.toggleSystem() } }
        ]
        if (Sway.activeToplevel) {
            items.push({ separator: true })
            items.push({ label: "Close window",    action: function () { Sway.dispatch("kill") } })
            items.push({ label: "Toggle floating", action: function () { Sway.dispatch("floating toggle") } })
            items.push({ label: "Fullscreen",      action: function () { Sway.dispatch("fullscreen toggle") } })
        }
        items.push({ separator: true })
        items.push({ label: "Reload shell", action: function () { Sway.dispatch("reload") } })
        items.push({ separator: true })
        items.push({ label: "Log out",   action: function () { Sway.dispatch("exit") } })
        items.push({ label: "Reboot",    action: function () { Quickshell.execDetached(["sudo", "-n", "systemctl", "reboot"]) } })
        items.push({ label: "Power off", danger: true, action: function () { Quickshell.execDetached(["sudo", "-n", "systemctl", "poweroff"]) } })
        return items
    }
}
