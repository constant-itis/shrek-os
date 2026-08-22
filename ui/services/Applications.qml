pragma Singleton
import QtQuick
import Quickshell
import Quickshell.Io

// Applications — the launcher's app model. Wraps Quickshell.DesktopEntries (core; enumerates installed
// .desktop apps, already excluding NoDisplay/Hidden) and exposes a query-filtered, FRECENCY-ranked result
// list. Frecency (frequency + recency) is learned from launches and PERSISTED to a small JSON state file
// so the launcher opens on the apps you actually use — the empty-query view is your recents/most-used,
// and matches are tie-broken by how often you launch them. Launch + run are ordinary user actions
// (DesktopEntry.execute / execDetached); no authority here.
QtObject {
    id: root

    property string query: ""

    // Live app list (DesktopEntry objects). ObjectModel.values is reactive.
    readonly property var all: DesktopEntries.applications ? DesktopEntries.applications.values : []

    // Frecency store: app key -> { count, last }. `last` is a monotonic launch sequence (recency proxy);
    // reassigned wholesale on change so bindings notify. Persisted to _stateFile.
    property var frecency: ({})
    property int _seq: 0
    readonly property string _stateDir: (Quickshell.env("HOME") || "/tmp") + "/.local/state/shrek"
    readonly property string _stateFile: _stateDir + "/launcher-frecency.json"

    // Filtered + ranked results (array of DesktopEntry). Rebinds on query/all/frecency change.
    readonly property var results: root._filter(root.query, root.all, root.frecency)

    function _key(e) { return "" + (e && (e.id || e.name) || "") }

    // Launch an app entry (records frecency).
    function launch(entry) {
        if (!entry) return
        root._bump(root._key(entry))
        entry.execute()
    }
    // Run an arbitrary command line (the launcher's `>` mode). Detached; the shell owns no lifecycle.
    function run(cmd) {
        var c = ("" + cmd).trim()
        if (c.length === 0) return
        Quickshell.execDetached(["sh", "-c", c])
    }

    // ── frecency persistence ─────────────────────────────────────────────────────────────────────
    property Process _loader: Process {
        command: ["sh", "-c", "cat '" + root._stateFile + "' 2>/dev/null"]
        stdout: StdioCollector { id: _lc; onStreamFinished: root._ingestFrecency(_lc.text) }
        Component.onCompleted: running = true
    }
    function _ingestFrecency(t) {
        var o
        try { o = JSON.parse(("" + t).trim() || "{}") } catch (e) { o = {} }
        if (!o || typeof o !== "object") o = {}
        var m = 0
        for (var k in o) if (o[k] && (o[k].last | 0) > m) m = o[k].last | 0
        root._seq = m
        root.frecency = o
    }
    // one-shot writer; launches are user-paced so a single reused Process cannot race meaningfully.
    property Process _writer: Process {}
    function _bump(key) {
        if (!key || key.length === 0) return
        var f = root.frecency
        var e = f[key] || { count: 0, last: 0 }
        e.count = (e.count | 0) + 1
        root._seq = root._seq + 1
        e.last = root._seq
        f[key] = e
        root.frecency = f            // reassign to notify bindings
        root._persist()
    }
    function _persist() {
        var b64 = Qt.btoa(JSON.stringify(root.frecency))   // base64 so no shell-quoting hazard
        _writer.command = ["sh", "-c",
            "mkdir -p '" + root._stateDir + "'; printf %s '" + b64 + "' | base64 -d > '" + root._stateFile + "'"]
        _writer.running = true
    }
    function _fr(key) { var e = root.frecency[key]; return e ? (e.count | 0) : 0 }

    // ── ranking (lower = better; -1 = no match) ──────────────────────────────────────────────────
    function _filter(q, list, _fre) {
        var needle = ("" + q).trim().toLowerCase()
        var scored = []
        for (var i = 0; i < list.length; i++) {
            var e = list[i]
            if (!e || e.noDisplay) continue
            var s = root._score(needle, e)
            if (s >= 0) scored.push({ e: e, s: s, f: root._fr(root._key(e)) })
        }
        scored.sort(function (a, b) {
            if (needle.length === 0) {
                // empty query: most-frecent first, then name
                if (a.f !== b.f) return b.f - a.f
                return ("" + (a.e.name || "")).localeCompare("" + (b.e.name || ""))
            }
            // searching: match quality first, frecency as the tie-breaker, then name
            if (a.s !== b.s) return a.s - b.s
            if (a.f !== b.f) return b.f - a.f
            return ("" + (a.e.name || "")).localeCompare("" + (b.e.name || ""))
        })
        var out = []
        for (var j = 0; j < scored.length; j++) out.push(scored[j].e)
        return out
    }

    function _score(needle, e) {
        var name = ("" + (e.name || "")).toLowerCase()
        var generic = ("" + (e.genericName || "")).toLowerCase()
        if (needle.length === 0) return 0                 // empty query -> all (frecency-sorted above)
        var idx = name.indexOf(needle)
        if (idx === 0) return 0                            // prefix on name = best
        if (idx > 0) return 10 + idx                       // substring in name
        var gidx = (name + " " + generic).indexOf(needle)
        if (gidx >= 0) return 100 + gidx                   // substring in name/generic
        if (root._subseq(needle, name)) return 1000        // fuzzy subsequence on name
        var kw = e.keywords || []
        for (var k = 0; k < kw.length; k++)
            if (("" + kw[k]).toLowerCase().indexOf(needle) >= 0) return 2000
        return -1
    }

    function _subseq(needle, hay) {
        var n = 0
        for (var i = 0; i < hay.length && n < needle.length; i++)
            if (hay.charAt(i) === needle.charAt(n)) n++
        return n === needle.length
    }
}
