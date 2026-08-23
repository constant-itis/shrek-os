pragma Singleton
import QtQuick
import Quickshell.Io

// Network — dormant, read-only NetworkManager adapter for host/human connectivity state.
//
// NetworkManager owns laptop/desktop connectivity (Wi-Fi, roaming, docks, VPN, metered state).
// Workload egress authority stays separate: gatekeeperd+nftables decide what sandboxes may reach.
// No surface consumes this yet, and this adapter has no connect/disconnect/mutation methods.
QtObject {
    id: root

    property bool available: false
    property bool online: false
    property string state: "unknown"
    property string connectivity: "unknown"
    property string primaryConnection: ""

    function reload() { proc.running = true }

    property Process proc: Process {
        command: ["sh", "-c",
            "if ! command -v busctl >/dev/null 2>&1; then printf 'missing|unknown|unknown|'; exit 0; fi; " +
            "svc=org.freedesktop.NetworkManager; path=/org/freedesktop/NetworkManager; iface=org.freedesktop.NetworkManager; " +
            "set -- $(busctl --system get-property \"$svc\" \"$path\" \"$iface\" State 2>/dev/null); st=${2:-}; " +
            "set -- $(busctl --system get-property \"$svc\" \"$path\" \"$iface\" Connectivity 2>/dev/null); co=${2:-}; " +
            "set -- $(busctl --system get-property \"$svc\" \"$path\" \"$iface\" PrimaryConnection 2>/dev/null); pc=${2:-}; " +
            "[ -n \"$st\" ] || { printf 'absent|unknown|unknown|'; exit 0; }; " +
            "printf 'present|%s|%s|%s' \"$st\" \"${co:-0}\" \"${pc:-/}\""]
        stdout: StdioCollector { id: collector; onStreamFinished: root._ingest(collector.text) }
    }

    property Timer timer: Timer {
        interval: 4000; running: true; repeat: true
        onTriggered: root.reload()
        Component.onCompleted: root.reload()
    }

    function _ingest(text) {
        var p = ("" + text).trim().split("|")
        root.available = p[0] === "present"
        root.state = root._stateName(parseInt(p[1] || "0"))
        root.connectivity = root._connectivityName(parseInt(p[2] || "0"))
        root.primaryConnection = (p[3] || "").replace(/^"|"$/g, "")
        root.online = root.available && (root.connectivity === "full" || root.connectivity === "limited")
    }

    function _stateName(v) {
        if (v === 70) return "connected-global"
        if (v === 60) return "connected-site"
        if (v === 50) return "connected-local"
        if (v === 40) return "connecting"
        if (v === 30) return "disconnected"
        if (v === 20) return "unavailable"
        if (v === 10) return "asleep"
        return "unknown"
    }

    function _connectivityName(v) {
        if (v === 4) return "full"
        if (v === 3) return "limited"
        if (v === 2) return "portal"
        if (v === 1) return "none"
        return "unknown"
    }
}
