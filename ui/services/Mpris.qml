pragma Singleton
import QtQuick
import Quickshell.Services.Mpris

// Mpris — the media-player read model + ordinary transport dispatch, behind ONE seam. Surfaces import
// this, never Quickshell.Services.Mpris directly. Backed by Quickshell's MPRIS backend (org.mpris
// MediaPlayer2 over DBus, event-driven). `active` is the player we surface: prefer one that is playing,
// else the first controllable one, else the first present. Transport is an ordinary user action — no
// authority is read or minted here.
QtObject {
    // Live array of MprisPlayer (Mpris.players is an ObjectModel; .values is its JS array, reactive via
    // valuesChanged).
    readonly property var players: Mpris.players ? Mpris.players.values : []

    readonly property var active: {
        var ps = players
        if (!ps || ps.length === 0)
            return null
        for (var i = 0; i < ps.length; i++)
            if (ps[i].isPlaying)
                return ps[i]
        for (var j = 0; j < ps.length; j++)
            if (ps[j].canControl)
                return ps[j]
        return ps[0]
    }

    readonly property bool hasPlayer: active !== null
    readonly property bool playing: active ? active.isPlaying : false
    readonly property string title: active ? ("" + active.trackTitle) : ""
    readonly property string artist: active ? ("" + active.trackArtist) : ""
    readonly property string artUrl: active ? ("" + active.trackArtUrl) : ""
    readonly property string identity: active ? ("" + active.identity) : ""
    readonly property bool canGoNext: active ? active.canGoNext : false
    readonly property bool canGoPrevious: active ? active.canGoPrevious : false
    readonly property bool canTogglePlaying: active ? active.canTogglePlaying : false

    function playPause() { if (active && active.canTogglePlaying) active.togglePlaying() }
    function next() { if (active && active.canGoNext) active.next() }
    function previous() { if (active && active.canGoPrevious) active.previous() }
    // Raise the player's own window (e.g. click the track title). Ordinary window action.
    function raise() { if (active && active.canRaise) active.raise() }
}
