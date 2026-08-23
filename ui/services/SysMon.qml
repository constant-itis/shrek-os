pragma Singleton
import QtQuick
import Quickshell.Io

// SysMon — honest READ-ONLY system telemetry for the dashboard's Performance tab. Same read-only Process
// idiom as Network.qml: a short sh probe polled on a timer, no daemon, no mutable-CLI table parsing.
// Reads /proc/stat (CPU busy% across a 0.25s window), /proc/meminfo (used/total), the first thermal zone
// (CPU temp), and a best-effort GPU hwmon. Missing sensors degrade honestly to -1 (e.g. GPU temp does not
// exist under the software-rendered dogfood VM) — the gauge shows "n/a" rather than a fake number.
QtObject {
    id: root

    property int    cpuPct:     0
    property real   cpuTemp:    -1     // °C, or -1 when no thermal zone is readable
    property real   gpuTemp:    -1     // °C, or -1 when no GPU hwmon is present
    property real   memUsedGiB: 0
    property real   memTotalGiB: 0
    readonly property real memPct: memTotalGiB > 0 ? (memUsedGiB / memTotalGiB) : 0

    function reload() { proc.running = true }

    property Process proc: Process {
        command: ["sh", "-c",
            "read _ a b c d e f g h _ < /proc/stat; t1=$((a+b+c+d+e+f+g+h)); i1=$d; " +
            "sleep 0.25; " +
            "read _ a b c d e f g h _ < /proc/stat; t2=$((a+b+c+d+e+f+g+h)); i2=$d; " +
            "dt=$((t2-t1)); di=$((i2-i1)); cpu=0; [ \"$dt\" -gt 0 ] && cpu=$(( (100*(dt-di))/dt )); " +
            "temp=-1; for z in /sys/class/thermal/thermal_zone*/temp; do [ -r \"$z\" ] && { temp=$(cat \"$z\"); break; }; done; " +
            "mt=$(awk '/^MemTotal/{print $2}' /proc/meminfo); ma=$(awk '/^MemAvailable/{print $2}' /proc/meminfo); mu=$((mt-ma)); " +
            "g=-1; for h in /sys/class/hwmon/hwmon*; do n=$(cat \"$h/name\" 2>/dev/null); " +
            "  case \"$n\" in amdgpu|nvidia|radeon) v=$(cat \"$h/temp1_input\" 2>/dev/null); [ -n \"$v\" ] && g=$v; break;; esac; done; " +
            "printf '%s|%s|%s|%s|%s' \"$cpu\" \"$temp\" \"$mu\" \"$mt\" \"$g\""]
        stdout: StdioCollector { id: collector; onStreamFinished: root._ingest(collector.text) }
    }

    property Timer timer: Timer {
        interval: 2000; running: true; repeat: true
        onTriggered: root.reload()
        Component.onCompleted: root.reload()
    }

    function _ingest(t) {
        var p = ("" + t).trim().split("|")
        root.cpuPct      = parseInt(p[0]) || 0
        var ct = parseInt(p[1]); root.cpuTemp = (isNaN(ct) || ct < 0) ? -1 : ct / 1000
        var mu = parseInt(p[2]) || 0
        var mt = parseInt(p[3]) || 0
        root.memUsedGiB  = mu / 1048576
        root.memTotalGiB = mt / 1048576
        var gt = parseInt(p[4]); root.gpuTemp = (isNaN(gt) || gt < 0) ? -1 : gt / 1000
    }
}
