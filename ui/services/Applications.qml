pragma Singleton
import QtQuick
import Quickshell

// Applications — the launcher's app model. Wraps Quickshell.DesktopEntries (core; enumerates installed
// .desktop apps, already excluding NoDisplay/Hidden) and exposes a query-filtered, ranked result list.
// Reactive: DesktopEntries.applications is an ObjectModel whose `values` notify on change, and `results`
// rebinds when `query` or the app list changes. Launch is an ordinary user action (DesktopEntry.execute
// -> Quickshell.execDetached); no authority here.
QtObject {
    id: root

    property string query: ""

    // Live app list (DesktopEntry objects). ObjectModel.values is reactive.
    readonly property var all: DesktopEntries.applications ? DesktopEntries.applications.values : []

    // Filtered + ranked results (array of DesktopEntry). Rebinds on query/all change.
    readonly property var results: root._filter(root.query, root.all)

    function launch(entry) { if (entry) entry.execute() }

    // ── ranking (lower = better; -1 = no match) ──────────────────────────────────────────────────
    function _filter(q, list) {
        var needle = ("" + q).trim().toLowerCase()
        var scored = []
        for (var i = 0; i < list.length; i++) {
            var e = list[i]
            if (!e || e.noDisplay) continue
            var s = root._score(needle, e)
            if (s >= 0) scored.push({ e: e, s: s })
        }
        scored.sort(function (a, b) {
            if (a.s !== b.s) return a.s - b.s
            return ("" + (a.e.name || "")).localeCompare("" + (b.e.name || ""))
        })
        var out = []
        for (var j = 0; j < scored.length; j++) out.push(scored[j].e)
        return out
    }

    function _score(needle, e) {
        var name = ("" + (e.name || "")).toLowerCase()
        var generic = ("" + (e.genericName || "")).toLowerCase()
        if (needle.length === 0) return 0                 // empty query -> all, name-sorted
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
