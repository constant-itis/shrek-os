pragma Singleton
import QtQuick
import Quickshell
import Quickshell.Io

// Network — NetworkManager host/human connectivity control.
//
// This controls the laptop's own connectivity through NetworkManager. It is intentionally unrelated to
// gatekeeperd/nftables workload egress authority.
QtObject {
    id: root

    property bool available: false
    property bool online: false
    property bool wifiEnabled: false
    property bool wifiHardware: false
    property string state: "unknown"
    property string connectivity: "unknown"
    property string activeConnection: ""
    property string activeDevice: ""
    property string activeType: ""
    property string lastError: ""
    property var networks: []
    property var savedConnections: []
    property string pendingSsid: ""
    property string pendingSecurity: ""

    function reload(): void { scanProc.running = true }
    function setWifiEnabled(on): void {
        Quickshell.execDetached(["nmcli", "radio", "wifi", on ? "on" : "off"])
        Qt.callLater(root.reload)
    }
    function toggleWifi(): void { setWifiEnabled(!wifiEnabled) }
    function disconnect(): void {
        if (activeDevice.length > 0)
            Quickshell.execDetached(["nmcli", "device", "disconnect", activeDevice])
        Qt.callLater(root.reload)
    }
    function connectSaved(name): void {
        if (!name) return
        opProc.command = ["nmcli", "connection", "up", "id", "" + name]
        opProc.running = true
    }
    function requestConnect(row): void {
        if (!row) return
        pendingSsid = row.ssid || ""
        pendingSecurity = row.security || ""
        if (row.saved) connectSaved(row.ssid)
        else if (!row.secured) connectWifi(row.ssid, "")
    }
    function connectWifi(ssid, password): void {
        if (!ssid) return
        var args = ["nmcli", "device", "wifi", "connect", "" + ssid]
        if (password && ("" + password).length > 0) args.push("password", "" + password)
        opProc.command = args
        opProc.running = true
    }
    function clearPending(): void {
        pendingSsid = ""
        pendingSecurity = ""
    }

    property Process opProc: Process {
        stdout: StdioCollector { id: opOut }
        stderr: StdioCollector { id: opErr }
        onExited: function(exitCode, exitStatus) {
            root.lastError = exitCode === 0 ? "" : (opErr.text || opOut.text || "NetworkManager command failed")
            if (exitCode === 0) root.clearPending()
            root.reload()
        }
    }

    property Process scanProc: Process {
        command: ["sh", "-c",
            "if ! command -v nmcli >/dev/null 2>&1; then printf 'available|0\\n'; exit 0; fi; " +
            "printf 'available|1\\n'; " +
            "nmcli -t -e no --separator '|' -f STATE,CONNECTIVITY general status 2>/dev/null | " +
            "  awk -F'|' 'NR==1{printf \"general|%s|%s\\n\",$1,$2}'; " +
            "nmcli -t -e no --separator '|' -f WIFI-HW,WIFI radio 2>/dev/null | " +
            "  awk -F'|' 'NR==1{printf \"radio|%s|%s\\n\",$1,$2}'; " +
            "nmcli -t -e no --separator '|' -f NAME,TYPE,DEVICE connection show --active 2>/dev/null | " +
            "  awk -F'|' 'NR==1{printf \"active|%s|%s|%s\\n\",$1,$2,$3}'; " +
            "nmcli -t -e no --separator '|' -f NAME,TYPE connection show 2>/dev/null | " +
            "  awk -F'|' '$2 ~ /802-11-wireless|wifi/{printf \"saved|%s\\n\",$1}'; " +
            "nmcli -t -e no --separator '|' -f IN-USE,SSID,SIGNAL,SECURITY device wifi list --rescan yes 2>/dev/null | " +
            "  awk -F'|' 'length($2)>0{printf \"wifi|%s|%s|%s|%s\\n\",$1,$2,$3,$4}'"]
        stdout: StdioCollector { id: collector; onStreamFinished: root._ingest(collector.text) }
    }

    property Timer timer: Timer {
        interval: 5000; running: true; repeat: true
        onTriggered: root.reload()
        Component.onCompleted: root.reload()
    }

    function _ingest(text) {
        var lines = ("" + text).split("\n")
        var saved = []
        var rawWifi = []
        available = false
        activeConnection = ""
        activeDevice = ""
        activeType = ""
        for (var i = 0; i < lines.length; i++) {
            var l = lines[i]
            if (!l) continue
            var p = l.split("|")
            if (p[0] === "available") available = p[1] === "1"
            else if (p[0] === "general") {
                state = p[1] || "unknown"
                connectivity = p[2] || "unknown"
            } else if (p[0] === "radio") {
                wifiHardware = p[1] === "enabled"
                wifiEnabled = p[2] === "enabled"
            } else if (p[0] === "active") {
                activeConnection = p[1] || ""
                activeType = p[2] || ""
                activeDevice = p[3] || ""
            } else if (p[0] === "saved") {
                saved.push(p[1] || "")
            } else if (p[0] === "wifi") {
                rawWifi.push({ active: p[1] === "*", ssid: p[2] || "", signal: parseInt(p[3] || "0"), security: p[4] || "" })
            }
        }
        var map = {}
        for (var s = 0; s < saved.length; s++) map[saved[s]] = true
        var dedup = {}
        var list = []
        for (var w = 0; w < rawWifi.length; w++) {
            var r = rawWifi[w]
            if (!r.ssid || dedup[r.ssid]) continue
            dedup[r.ssid] = true
            r.saved = !!map[r.ssid]
            r.secured = r.security.length > 0 && r.security !== "--"
            list.push(r)
        }
        list.sort(function(a, b) {
            if (a.active !== b.active) return a.active ? -1 : 1
            if (a.saved !== b.saved) return a.saved ? -1 : 1
            return b.signal - a.signal
        })
        networks = list
        savedConnections = saved
        online = available && (state.indexOf("connected") === 0 || connectivity === "full" || connectivity === "limited")
    }
}
