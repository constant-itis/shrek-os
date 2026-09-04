pragma Singleton
import QtQuick
import Quickshell
import Quickshell.Io

// Egress — the desktop-egress bless read model + action seam (ADR-007 S3).
//
// Deny-by-default desktop egress (S1/S2): the session (uid 1000) starts sealed and the human BLESSES
// what it may reach. This service is the uid-1000 side of that plane. It is deliberately split the same
// way the backend is:
//
//   * READ is file-based, never the socket. It polls the root-written, world-readable projection
//     `/run/shrek/egress/state` (schema `shrek-egress-state/1`) + the `events` notification log — the
//     0700 bless store is unreadable to us by design ([R2-MF-A]). The state file is the SOLE display
//     truth; we never infer a profile's status from an action's exit code (which a fake daemon could
//     spoof, and which races the projection anyway).
//   * WRITE is the socket, via the unprivileged `egressd ask` client with a FIXED argv (no shell). The
//     daemon is the sole authority (uid gate, sealed Tier-B, verb allowlist, rate limit) — we only ask.
//     Only `weather` is one-click blessable here; baseline is always-on and `web-browsing` needs the
//     console ceremony (S4), both refused at the socket regardless of what this UI sends.
//
// After an action we do NOT optimistically flip the control: we mark it busy (disabled) and let the next
// projection poll reveal the real outcome, kicking a fast refresh so the lag is short. A resolve failure
// (first-run before the clock/network is up) surfaces legibly as "blessed, waiting" — not an error.
QtObject {
    id: root

    // The sealed egressd client + the /run projection dir. Overridable ONLY for the headless render
    // proof; the sealed session inherits the defaults (the client itself ignores env in the shipped
    // build, so this cannot redirect it at a fake daemon on a real box).
    readonly property string bin: Quickshell.env("SHREK_EGRESS_BIN") || "/usr/libexec/shrek/egressd"
    readonly property string runDir: Quickshell.env("SHREK_EGRESS_RUN") || "/run/shrek/egress"

    // Parsed per-profile rows: { name, tier, blessed, pins:[..], refreshed, fault, hasPins, faulted,
    // live, pending }. `available` is false until a well-formed state projection has been read
    // (fail-closed: an unknown schema or a missing file renders "unavailable", never a guess).
    property var profiles: []
    property bool available: false

    // The single in-flight action (an appliance blesses one thing at a time); while set, the panel
    // disables that profile's controls so a resolve/rate-limit/deny can't be masked by an optimistic
    // flip, and rapid re-clicks are debounced (six clicks would otherwise eat the whole rate window).
    property string busyProfile: ""
    property int _busyWasBlessed: -1

    // Latest event from the downstream notification log, for a lightweight "last activity" line.
    property var lastEvent: null
    property string _lastEventRaw: ""
    signal eventArrived(var ev)

    // --- lookups ---------------------------------------------------------------------------------
    function profile(name) {
        for (var i = 0; i < profiles.length; i++)
            if (profiles[i].name === name) return profiles[i]
        return null
    }
    function busy(name) { return busyProfile === name }
    readonly property var weather: profile("weather")
    // The DMS weather dashTab is enabled IFF weather is a LIVE bless (blessed + pinned + no fault).
    readonly property bool weatherLive: weather ? weather.live : false

    // --- actions (the socket write path) ---------------------------------------------------------
    function bless(name)   { _ask("bless", name) }
    function unbless(name) { _ask("unbless", name) }
    function repin(name)   { _ask("repin", name) }

    function _ask(verb, name) {
        if (!name || busyProfile.length > 0) return           // debounce: one action in flight
        var p = profile(name)
        busyProfile = name
        _busyWasBlessed = p ? (p.blessed ? 1 : 0) : 0
        Quickshell.execDetached([root.bin, "ask", verb, name]) // fixed argv, no shell
        busyGuard.restart()                                    // clear busy even if nothing changes
        kick.restart()                                         // catch the projection up fast
    }

    function reload() { proc.running = true }

    // --- the projection reader -------------------------------------------------------------------
    // One read of BOTH files, form-feed (0x0C) separated (FF cannot appear in either — state is a
    // closed token grammar, events escape control bytes). Pure `cat`/`tail`, no mutation.
    property Process proc: Process {
        command: ["sh", "-c",
            "d=\"${SHREK_EGRESS_RUN:-/run/shrek/egress}\"; " +
            "cat \"$d/state\" 2>/dev/null; printf '\\f'; tail -n 20 \"$d/events\" 2>/dev/null"]
        stdout: StdioCollector { id: collector; onStreamFinished: root._ingest(collector.text) }
    }

    property Timer timer: Timer {
        interval: 2000; running: true; repeat: true
        onTriggered: root.reload()
        Component.onCompleted: root.reload()
    }

    // A one-shot fast refresh fired right after an action, so a bless is reflected in ~1s not ~2s.
    property Timer kick: Timer { interval: 700; repeat: false; onTriggered: root.reload() }

    // Failsafe: clear the busy lock a few seconds after an action even if the profile's blessed state
    // never changed (e.g. a repin that keeps it blessed, or a denied request), so controls never wedge.
    property Timer busyGuard: Timer { interval: 4500; repeat: false; onTriggered: root.busyProfile = "" }

    function _ingest(text) {
        var parts = ("" + text).split(String.fromCharCode(12))
        _ingestState(parts.length > 0 ? parts[0] : "")
        _ingestEvents(parts.length > 1 ? parts[1] : "")
    }

    function _ingestState(text) {
        var lines = ("" + text).split("\n")
        // Fail-closed: the first non-empty line MUST be the known schema header, else "unavailable".
        var header = ""
        var start = 0
        for (var h = 0; h < lines.length; h++) {
            if (lines[h].trim().length > 0) { header = lines[h].trim(); start = h + 1; break }
        }
        if (header !== "schema shrek-egress-state/1") {
            root.available = false
            root.profiles = []
            return
        }
        var rows = []
        for (var i = start; i < lines.length; i++) {
            var l = lines[i].trim()
            if (l.length === 0) continue
            var row = _parseProfileLine(l)
            if (row) rows.push(row)
        }
        root.profiles = rows
        root.available = true
        // If the busy profile has settled to a different blessed state, release the lock immediately.
        if (busyProfile.length > 0) {
            var bp = root.profile(busyProfile)
            if (bp && _busyWasBlessed >= 0 && (bp.blessed ? 1 : 0) !== _busyWasBlessed)
                root.busyProfile = ""
        }
        // Load-bearing marker: the headless render proof greps this to confirm the projection flowed
        // file -> service -> panel.
        console.log("SHREK-DESKTOP connectivity egress state profiles=" + rows.length
            + " weatherLive=" + root.weatherLive)
    }

    // `profile <name> tier=<t> blessed=<0|1> pins=<ip,ip|-> refreshed=<unix|-> fault=<kind|->`
    function _parseProfileLine(l) {
        var toks = l.split(/\s+/)
        if (toks[0] !== "profile" || toks.length < 2) return null
        var row = {
            name: toks[1], tier: "", blessed: false, pins: [],
            refreshed: 0, fault: "-", hasPins: false, faulted: false, live: false, pending: false
        }
        for (var i = 2; i < toks.length; i++) {
            var kv = toks[i].split("=")
            if (kv.length !== 2) continue
            var k = kv[0], v = kv[1]
            if (k === "tier") row.tier = v
            else if (k === "blessed") row.blessed = v === "1"
            else if (k === "pins") row.pins = (v === "-") ? [] : v.split(",")
            else if (k === "refreshed") row.refreshed = (v === "-") ? 0 : (parseInt(v) || 0)
            else if (k === "fault") row.fault = v
        }
        row.hasPins = row.pins.length > 0
        row.faulted = row.fault !== "-"
        row.live = row.blessed && row.hasPins && !row.faulted
        row.pending = row.blessed && !row.live       // blessed but not yet reachable (waiting/fault)
        return row
    }

    function _ingestEvents(text) {
        var lines = ("" + text).split("\n").filter(function(s) { return s.trim().length > 0 })
        if (lines.length === 0) return
        var raw = lines[lines.length - 1]
        if (raw === _lastEventRaw) return             // dedup by content position, not by timestamp
        var firstLoad = _lastEventRaw.length === 0
        _lastEventRaw = raw
        // `<ts> <verb> <profile> <result...>`
        var t = raw.split(/\s+/)
        var ev = { ts: parseInt(t[0]) || 0, verb: t[1] || "", profile: t[2] || "",
                   result: t.slice(3).join(" ") }
        root.lastEvent = ev
        if (!firstLoad) root.eventArrived(ev)         // don't toast the backlog on first paint
    }
}
