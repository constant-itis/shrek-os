pragma Singleton
import QtQuick
import Quickshell
import Quickshell.Io

// Clipboard — a text clipboard history + copy-back, behind ONE seam. Backed by wl-clipboard: a long-lived
// `wl-paste --watch` captures every clipboard change; wl-copy writes an entry back. History is in-memory,
// newest-first, de-duplicated, capped. Ordinary user data handling — no authority is read or minted here.
QtObject {
    id: clip

    // Newest-first array of clipboard text entries.
    property var history: []
    readonly property int max: 50

    function ingest(text) {
        if (text === undefined || text === null) return
        var t = "" + text
        if (t.length === 0) return
        // De-dup: drop any earlier identical copy, then push to the front (so re-copying an old entry
        // just moves it to the top rather than growing the list).
        var out = []
        for (var i = 0; i < history.length; i++)
            if (history[i] !== t)
                out.push(history[i])
        out.unshift(t)
        if (out.length > max)
            out.length = max
        history = out
    }

    // Copy an entry back to the system clipboard. `--` guards entries that start with a dash; the text is
    // one argv element (no shell) so newlines/quotes survive intact. wl-paste --watch will see this as a
    // change and re-ingest it — harmless (it de-dups to the existing head).
    function copy(text) { Quickshell.execDetached(["wl-copy", "--", "" + text]) }
    function clearHistory() { history = [] }

    // wl-paste --watch runs the program on EVERY clipboard change, piping the new content to its stdin.
    // The program echoes that content then a form-feed; SplitParser emits each entry as its 0x0C arrives.
    property Process _watch: Process {
        command: ["wl-paste", "--type", "text", "--watch", "sh", "-c", "cat; printf '\\f'"]
        running: true
        stdout: SplitParser {
            splitMarker: "\f"
            onRead: (data) => clip.ingest(data)
        }
    }
}
