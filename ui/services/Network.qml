pragma Singleton
import QtQuick
import Quickshell.Io

// Network — honest READ-ONLY link/connectivity state for Desktop Slice 1. The image runs
// systemd-networkd (not NetworkManager), and the VM has no Wi-Fi, so this reports link state only; a
// Wi-Fi picker + NetworkManager are deferred to real-hardware dogfooding (docs/desktop-slice1-plan.md).
// Reads /proc/net/route (default route) + /sys/class/net (operstate) + optional `ip` for the address —
// no daemon control, no parsing of a mutable CLI's formatted tables. Same read-only Process idiom as
// SessionProvider.
QtObject {
    id: root

    property bool online: false
    property string iface: ""
    property string address: ""

    function reload() { proc.running = true }

    property Process proc: Process {
        command: ["sh", "-c",
            "dev=; while read -r i d g rest; do [ \"$d\" = 00000000 ] && dev=$i; done < /proc/net/route; " +
            "st=; ip4=; if [ -n \"$dev\" ]; then st=$(cat /sys/class/net/$dev/operstate 2>/dev/null); " +
            "command -v ip >/dev/null 2>&1 && ip4=$(ip -o -4 addr show dev $dev 2>/dev/null | awk '{print $4}' | head -1); fi; " +
            "printf '%s|%s|%s' \"$dev\" \"$st\" \"$ip4\""]
        stdout: StdioCollector { id: collector; onStreamFinished: root._ingest(collector.text) }
    }

    property Timer timer: Timer {
        interval: 4000; running: true; repeat: true
        onTriggered: root.reload()
        Component.onCompleted: root.reload()
    }

    function _ingest(t) {
        var p = ("" + t).trim().split("|")
        root.iface = p[0] || ""
        var st = p[1] || ""
        root.address = p[2] || ""
        root.online = root.iface.length > 0 && (st === "up" || st === "unknown" || st === "routable" || root.address.length > 0)
    }
}
