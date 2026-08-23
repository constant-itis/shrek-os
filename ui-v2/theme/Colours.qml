pragma Singleton
import QtQuick
import Quickshell
import Quickshell.Io

// Dynamic (wallpaper-derived) palette SOURCE. An external tool (matugen — Phase B) extracts a Material
// palette from the active wallpaper and writes it as a flat, semantic-keyed JSON to the path below; this
// singleton watches that file and republishes it as `scheme`. The shell NEVER reads this directly — it
// flows through Theme -> Tokens like every other mode. Absent/invalid file => empty scheme => Theme falls
// back (to the curated shrek-dark floor), so "dynamic" is always safe even before any wallpaper is processed.
QtObject {
    id: root

    readonly property string path: (Quickshell.env("HOME") || "") + "/.local/state/shrek/colours.json"
    // The live wallpaper-derived scheme (a partial or full semantic-keyed object), or {} when unavailable.
    property var scheme: ({})

    function _load() {
        try { root.scheme = JSON.parse(_fv.text() || "{}") || ({}); }
        catch (e) { root.scheme = ({}); }
    }

    // QtObject has no default child list, so the watcher is held via an explicit property.
    property FileView _fv: FileView {
        path: root.path
        watchChanges: true
        printErrors: false
        onFileChanged: reload()
        onLoadedChanged: if (loaded) root._load()
    }
}
