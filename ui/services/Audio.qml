pragma Singleton
import QtQuick
import Quickshell.Services.Pipewire

// Audio — the default output sink, behind one seam. Native PipeWire (event-driven, no wpctl parsing).
// PipeWire node audio props only stay live while the node is tracked, so the default sink is bound via
// PwObjectTracker. Ordinary user control (volume/mute); no authority.
QtObject {
    id: root

    readonly property var sink: Pipewire.defaultAudioSink
    readonly property bool ready: Pipewire.ready && root.sink !== null
    readonly property real volume: (root.sink && root.sink.audio) ? root.sink.audio.volume : 0
    readonly property bool muted: (root.sink && root.sink.audio) ? root.sink.audio.muted : false
    readonly property string label: root.sink ? (root.sink.description || root.sink.nickname || root.sink.name || "Output") : ""

    // Keep the default sink bound so its audio properties are live.
    property PwObjectTracker _tracker: PwObjectTracker { objects: root.sink ? [root.sink] : [] }

    function setVolume(v) {
        if (root.sink && root.sink.audio)
            root.sink.audio.volume = Math.max(0, Math.min(1, v))
    }
    function toggleMute() {
        if (root.sink && root.sink.audio)
            root.sink.audio.muted = !root.sink.audio.muted
    }
}
