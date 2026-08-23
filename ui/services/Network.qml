pragma Singleton
import QtQuick
import Quickshell.Io

// Network — legacy read-only link/connectivity state from the pre-NetworkManager shell.
// ui-v2/services/Network.qml is the current dormant NetworkManager adapter.
// Reads /proc/net/route (default route) + /sys/class/net (operstate) + optional `ip` for the address —
// no daemon control, no parsing of a mutable CLI's formatted tables. Same read-only Process idiom as
// SessionProvider.
QtObject {
    id: root

    property bool online: false
    property bool hasRoute: false
    property string iface: ""
    property string address: ""

    function reload() { proc.running = true }

    // Honest link/connectivity probe, robust to the virtio-net quirks seen in the dogfood VM:
    //   - virtio operstate is often "unknown" (not "up") even on a live link, and
    //   - a DHCP-leased iface may momentarily lack a default route.
    // So we don't hinge "online" on a default route existing. We find the default-route iface (best
    // signal) but ALSO scan every non-loopback iface for one that's up/unknown with a GLOBAL IPv4, and
    // report connected when we have an address (LAN-reachable) or a default route (internet-reachable).
    property Process proc: Process {
        command: ["sh", "-c",
            "defdev=; while read -r i d g rest; do [ \"$d\" = 00000000 ] && defdev=$i; done < /proc/net/route; " +
            "best=; bst=; for n in /sys/class/net/*; do dev=${n##*/}; [ \"$dev\" = lo ] && continue; " +
            "  st=$(cat \"$n/operstate\" 2>/dev/null); case \"$st\" in up|unknown) ;; *) continue ;; esac; " +
            "  if [ -z \"$best\" ] || [ \"$dev\" = \"$defdev\" ]; then best=$dev; bst=$st; fi; done; " +
            "ip4=; if command -v ip >/dev/null 2>&1 && [ -n \"$best\" ]; then " +
            "  ip4=$(ip -o -4 addr show dev \"$best\" scope global 2>/dev/null | awk '{print $4}' | head -1); fi; " +
            "printf '%s|%s|%s|%s' \"$best\" \"$bst\" \"$ip4\" \"$defdev\""]
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
        root.address = p[2] || ""
        root.hasRoute = (p[3] || "").length > 0
        // Connected if we hold a global address (LAN) or a default route (internet) on an up/unknown link.
        root.online = root.iface.length > 0 && (root.address.length > 0 || root.hasRoute)
    }
}
