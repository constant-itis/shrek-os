pragma Singleton
import QtQuick
import Quickshell.Io
import Quickshell.Services.UPower

// Power — UPower laptop status. This is status-only unless a mature power-profile backend is present.
QtObject {
    id: root

    readonly property var device: UPower.displayDevice
    readonly property bool present: root.device ? (root.device.isLaptopBattery && root.device.isPresent) : false
    readonly property real percentage: root.device ? root.device.percentage : 0
    readonly property bool onBattery: UPower.onBattery
    readonly property int stateCode: root.device ? root.device.state : 0
    readonly property string state: _stateName(stateCode)
    readonly property bool charging: state === "charging" || state === "full"
    readonly property int secondsToEmpty: root.device ? root.device.timeToEmpty : 0
    readonly property int secondsToFull: root.device ? root.device.timeToFull : 0
    readonly property string estimate: _estimate()

    property bool profilesAvailable: false
    property string profile: ""
    property var profiles: []

    function setProfile(p): void {
        if (p) profileProc.command = ["powerprofilesctl", "set", p]
        if (p) profileProc.running = true
        Qt.callLater(reloadProfiles)
    }
    function reloadProfiles(): void { profilesProc.running = true }

    property Process profilesProc: Process {
        command: ["sh", "-c",
            "command -v powerprofilesctl >/dev/null 2>&1 || exit 0; " +
            "cur=$(powerprofilesctl get 2>/dev/null || true); " +
            "printf 'current|%s\\n' \"$cur\"; " +
            "powerprofilesctl list 2>/dev/null | sed -n 's/^[ *]*//; s/:$//; /^[a-z].*/p'"]
        stdout: StdioCollector { id: ppCollector; onStreamFinished: root._ingestProfiles(ppCollector.text) }
    }
    property Process profileProc: Process {}
    property Timer timer: Timer {
        interval: 8000; running: true; repeat: true
        onTriggered: root.reloadProfiles()
        Component.onCompleted: root.reloadProfiles()
    }

    function _ingestProfiles(text) {
        var list = []
        var cur = ""
        var lines = ("" + text).split("\n")
        for (var i = 0; i < lines.length; i++) {
            var l = lines[i].trim()
            if (!l) continue
            if (l.indexOf("current|") === 0) cur = l.slice(8)
            else list.push(l)
        }
        profilesAvailable = list.length > 0
        profiles = list
        profile = cur
    }

    function _stateName(v) {
        if (v === UPowerDeviceState.Charging) return "charging"
        if (v === UPowerDeviceState.Discharging) return "discharging"
        if (v === UPowerDeviceState.FullyCharged) return "full"
        if (v === UPowerDeviceState.PendingCharge) return "pending charge"
        if (v === UPowerDeviceState.PendingDischarge) return "pending discharge"
        if (v === UPowerDeviceState.Empty) return "empty"
        return UPower.onBattery ? "on battery" : "AC power"
    }
    function _fmtSeconds(s) {
        if (!s || s <= 0) return ""
        var h = Math.floor(s / 3600)
        var m = Math.round((s % 3600) / 60)
        return h > 0 ? (h + "h " + m + "m") : (m + "m")
    }
    function _estimate() {
        if (!present) return "AC power"
        if (secondsToEmpty > 0) return _fmtSeconds(secondsToEmpty) + " remaining"
        if (secondsToFull > 0) return _fmtSeconds(secondsToFull) + " until full"
        return state
    }
}
