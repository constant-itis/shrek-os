pragma Singleton
import QtQuick
import Quickshell
import Quickshell.Io
import Quickshell.Services.Pipewire

// Audio — PipeWire default input/output control. Native PipeWire owns live volume/mute; `wpctl` is used
// only for default-device selection because Quickshell v0.3.1 exposes the nodes but not a stable setter.
QtObject {
    id: root

    readonly property var sink: Pipewire.defaultAudioSink
    readonly property var source: Pipewire.defaultAudioSource
    readonly property bool ready: Pipewire.ready && root.sink !== null
    readonly property bool inputReady: Pipewire.ready && root.source !== null
    readonly property real volume: (root.sink && root.sink.audio) ? root.sink.audio.volume : 0
    readonly property bool muted: (root.sink && root.sink.audio) ? root.sink.audio.muted : false
    readonly property real inputVolume: (root.source && root.source.audio) ? root.source.audio.volume : 0
    readonly property bool inputMuted: (root.source && root.source.audio) ? root.source.audio.muted : false
    readonly property string label: root._nodeLabel(root.sink, "Output")
    readonly property string inputLabel: root._nodeLabel(root.source, "Input")

    property var outputs: []
    property var inputs: []

    property PwObjectTracker _tracker: PwObjectTracker {
        objects: {
            var o = []
            if (root.sink) o.push(root.sink)
            if (root.source) o.push(root.source)
            return o
        }
    }

    function reloadDevices(): void { devicesProc.running = true }

    function setVolume(v): void {
        if (root.sink && root.sink.audio)
            root.sink.audio.volume = Math.max(0, Math.min(1.5, v))
    }
    function toggleMute(): void {
        if (root.sink && root.sink.audio)
            root.sink.audio.muted = !root.sink.audio.muted
    }
    function setInputVolume(v): void {
        if (root.source && root.source.audio)
            root.source.audio.volume = Math.max(0, Math.min(1.5, v))
    }
    function toggleInputMute(): void {
        if (root.source && root.source.audio)
            root.source.audio.muted = !root.source.audio.muted
    }
    function selectOutput(id): void {
        if (id) Quickshell.execDetached(["wpctl", "set-default", "" + id])
        Qt.callLater(root.reloadDevices)
    }
    function selectInput(id): void {
        if (id) Quickshell.execDetached(["wpctl", "set-default", "" + id])
        Qt.callLater(root.reloadDevices)
    }

    function _nodeLabel(node, fallback) {
        if (!node) return fallback
        return node.description || node.nickname || node.name || fallback
    }

    property Process devicesProc: Process {
        command: ["sh", "-c",
            "command -v wpctl >/dev/null 2>&1 || exit 0; " +
            "wpctl status 2>/dev/null | awk '" +
            "BEGIN{sec=\"\"} " +
            "/Sinks:/{sec=\"out\"; next} /Sources:/{sec=\"in\"; next} " +
            "/^[[:alnum:]_ -]+$/{sec=\"\"} " +
            "sec && match($0,/^[^0-9*]*(\\*)?[[:space:]]*([0-9]+)\\. (.*)$/,m){label=m[3]; sub(/[[:space:]]*\\[vol:.*$/, \"\", label); printf \"%s|%s|%s|%s\\n\", sec,m[2],label,(m[1]==\"*\"?\"1\":\"0\")}'"]
        stdout: StdioCollector { id: devCollector; onStreamFinished: root._ingestDevices(devCollector.text) }
    }

    property Timer timer: Timer {
        interval: 5000; running: true; repeat: true
        onTriggered: root.reloadDevices()
        Component.onCompleted: root.reloadDevices()
    }

    function _ingestDevices(text) {
        var outs = []
        var ins = []
        var lines = ("" + text).split("\n")
        for (var i = 0; i < lines.length; i++) {
            var p = lines[i].split("|")
            if (p.length < 4) continue
            var row = { id: p[1], label: p[2], active: p[3] === "1" }
            if (p[0] === "out") outs.push(row)
            if (p[0] === "in") ins.push(row)
        }
        outputs = outs
        inputs = ins
    }
}
