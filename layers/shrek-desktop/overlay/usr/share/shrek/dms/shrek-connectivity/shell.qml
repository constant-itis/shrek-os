// shrek-connectivity — the "Network Access" panel (ADR-009 v2 §6, S4).
//
// A shrek-OWNED standalone Quickshell surface, the SAME pattern as shrek-menu: a SECOND `qs` process
// alongside DMS (NOT a DMS plugin, NOT spliced into DMS's tree), hidden until toggled, its IPC target
// scoped to THIS qs instance. So the toggle MUST go through `qs -p <this file>`, never `dms ipc`:
//
//   launch:  exec_always qs -p /usr/share/shrek/dms/shrek-connectivity/shell.qml -n   (sway.config)
//   toggle:  qs -p /usr/share/shrek/dms/shrek-connectivity/shell.qml ipc call shrek-connectivity toggle
//            (Super+Shift+N)
//
// This is the steady-state face of the desktop-egress plane (ADR-007 + ADR-009): the desktop starts
// sealed and the human blesses what it may reach. It is SELF-CONTAINED — a second qs process is
// blackholed from importing DMS/ui-v2 singletons, so it embeds its own read model (mirroring
// ui-v2/services/Egress.qml, extended for the ADR-009 catalog card text + the pending-needs inbox) and
// renders four sections in plain QtQuick themed from DMS's matugen output. It reads the root-written,
// world-readable /run projection (the 0700 store is unreadable to uid 1000 by design); the projection is
// the SOLE display truth — controls never optimistically flip on an action's exit code.
//
// Tiers (ADR-009 §6): baseline (time/updates) is on + explained, no control (revoke is console-only);
// `weather` is one-click over the egressd socket; `web-browsing` + raw destinations are the
// console-ceremony tier (routed through `shrek connectivity` -> gatekeeperd SAK/VT). Owner-installed
// capabilities render as legible cards but are display-only in S4 (their one-click bless + the intent
// watcher are a later slice; ADR banner: "toggle != live human intent").
import QtQuick
import Quickshell
import Quickshell.Io
import Quickshell.Wayland

ShellRoot {
    id: root

    Component.onCompleted: console.info("SHREK-CONNECTIVITY surface loaded (hidden until toggle)")

    // ── Live theme parity from DMS matugen output (identical to shrek-menu's reader) ─────────────────
    // A second qs process can't import DMS's Theme singleton (quickshell's per-instance qs:// interceptor
    // blackholes it), so read the same atomically-rewritten file DMS's matugen produces and follow it live.
    readonly property string colorMode: "dark"
    property var dmsColors: ({})
    function reloadColors() {
        try { root.dmsColors = JSON.parse(colorsFile.text() || "{}") || ({}); }
        catch (e) { root.dmsColors = ({}); }
    }
    function themed(key, fallback) {
        var mode = root.dmsColors && root.dmsColors.colors ? root.dmsColors.colors[root.colorMode] : null;
        var v = mode ? mode[key] : undefined;
        return (typeof v === "string" && v.length > 0) ? v : fallback;
    }
    FileView {
        id: colorsFile
        path: (Quickshell.env("HOME") || "") + "/.cache/DankMaterialShell/dms-colors.json"
        blockLoading: false
        watchChanges: true
        printErrors: false
        onFileChanged: reload()
        onLoaded: root.reloadColors()
    }

    // Surface tokens: live DMS value first, baked swamp-green (DMS default palette) as the fallback.
    readonly property color cSurface:     themed("surface_container_high", "#1b2a1a")
    readonly property color cCard:        themed("surface_container_low",  "#13200e")
    readonly property color cSurfaceText: themed("on_surface",             "#e6efe0")
    readonly property color cSurfaceDim:  themed("on_surface_variant",     "#9db097")
    readonly property color cPrimary:     themed("primary",                "#7cae5a")
    readonly property color cOnPrimary:   themed("on_primary",             "#0c1a06")
    readonly property color cOutline:     themed("outline_variant",        "#3a4a34")
    readonly property color cSelected:    themed("primary_container",      "#2c3f22")
    // Status tints (derived, not matugen keys): on = primary, waiting = amber, attention = red, off = dim.
    readonly property color cOk:      cPrimary
    readonly property color cWaiting: themed("tertiary", "#d8b45a")
    readonly property color cDanger:  themed("error",    "#e06c6c")

    // ── Egress read model (mirrors ui-v2/services/Egress.qml; extended for ADR-009) ──────────────────
    readonly property string egressBin: Quickshell.env("SHREK_EGRESS_BIN") || "/usr/libexec/shrek/egressd"
    readonly property string shrekBin:  Quickshell.env("SHREK_CLI_BIN")    || "shrek"
    readonly property string runDir:    Quickshell.env("SHREK_EGRESS_RUN") || "/run/shrek/egress"

    property var profiles: []          // [{name,tier,blessed,pins,refreshed,fault,source,feature,hasPins,faulted,live,pending}]
    property var rawEntries: []        // [{host,proto,port,blessed,pins,refreshed,hasPins,pending,wire}]
    property var wants: []             // [{token,ts}] — the pending-needs inbox
    property var cardText: ({})        // name -> {title,purpose,capfault}
    property bool available: false
    property var lastEvent: null
    property string _lastEventRaw: ""

    // One action in flight at a time (an appliance blesses one thing at a time); disables that control so
    // a deny/resolve-fail/rate-limit is never masked by an optimistic flip.
    property string busyProfile: ""
    property int _busyWasBlessed: -1
    function busy(name) { return busyProfile === name }

    // Console-ceremony hint: shown from launch until the SAK window should have resolved (~120s). Not
    // authority — the projection stays the display truth for what actually got blessed.
    property bool ceremonyActive: false
    property string ceremonyLabel: ""

    function profileByName(n) {
        for (var i = 0; i < profiles.length; i++) if (profiles[i].name === n) return profiles[i]
        return null
    }
    readonly property var baselineProfiles: profiles.filter(function (p) { return p.tier === "baseline" })
    readonly property var featureProfiles:  profiles.filter(function (p) { return p.tier !== "baseline" })

    // --- actions --------------------------------------------------------------------------------------
    function bless(name)   { _ask("bless", name) }
    function unbless(name) { _ask("unbless", name) }
    function repin(name)   { _ask("repin", name) }
    function _ask(verb, name) {
        if (!name || busyProfile.length > 0) return
        var p = profileByName(name)
        busyProfile = name
        _busyWasBlessed = p ? (p.blessed ? 1 : 0) : 0
        Quickshell.execDetached([root.egressBin, "ask", verb, name])   // fixed argv, no shell
        busyGuard.restart(); kick.restart()
    }
    function blessCeremony(name)   { _ceremony("bless", name, "Allow " + name) }
    function unblessCeremony(name) { _ceremony("unbless", name, "Turn off " + name) }
    function addRaw(triple)        { _ceremony("add-raw", triple, "Add " + triple) }
    function removeRaw(triple)     { _ceremony("remove-raw", triple, "Remove " + triple) }
    function _ceremony(verb, arg, label) {
        if (!arg) return
        root.ceremonyActive = true; root.ceremonyLabel = label
        Quickshell.execDetached([root.shrekBin, "connectivity", verb, arg])   // fixed argv, no shell
        ceremonyGuard.restart(); kick.restart()
    }
    property Timer ceremonyGuard: Timer { interval: 120000; repeat: false; onTriggered: { root.ceremonyActive = false; root.ceremonyLabel = "" } }

    function reload() { proc.running = true }

    // One read of state + events + wants, form-feed (0x0C) separated (FF cannot appear in any: state +
    // wants are closed-token grammars, events escape control bytes). Pure cat/tail, no mutation.
    property Process proc: Process {
        command: ["sh", "-c",
            "d=\"${SHREK_EGRESS_RUN:-/run/shrek/egress}\"; " +
            "cat \"$d/state\" 2>/dev/null; printf '\\f'; " +
            "tail -n 20 \"$d/events\" 2>/dev/null; printf '\\f'; " +
            "cat \"$d/wants\" 2>/dev/null"]
        stdout: StdioCollector { id: collector; onStreamFinished: root._ingest(collector.text) }
    }
    property Timer timer: Timer { interval: 2000; running: true; repeat: true; onTriggered: root.reload(); Component.onCompleted: root.reload() }
    property Timer kick: Timer { interval: 700; repeat: false; onTriggered: root.reload() }
    property Timer busyGuard: Timer { interval: 4500; repeat: false; onTriggered: root.busyProfile = "" }

    function _ingest(text) {
        var parts = ("" + text).split(String.fromCharCode(12))
        // wants first so the state-load marker below reflects the current inbox count, not last poll's.
        _ingestWants(parts.length > 2 ? parts[2] : "")
        _ingestState(parts.length > 0 ? parts[0] : "")
        _ingestEvents(parts.length > 1 ? parts[1] : "")
    }

    function _ingestState(text) {
        var lines = ("" + text).split("\n")
        var header = "", start = 0
        for (var h = 0; h < lines.length; h++) {
            if (lines[h].trim().length > 0) { header = lines[h].trim(); start = h + 1; break }
        }
        if (header !== "schema shrek-egress-state/1") { root.available = false; root.profiles = []; root.rawEntries = []; root.cardText = ({}); return }
        var rows = [], raws = [], cards = ({})
        for (var i = start; i < lines.length; i++) {
            var l = lines[i].trim()
            if (l.length === 0) continue
            if (l.indexOf("profile ") === 0)      { var row = _parseProfileLine(l); if (row) rows.push(row) }
            else if (l.indexOf("raw ") === 0)      { var rr = _parseRawLine(l); if (rr) raws.push(rr) }
            else if (l.indexOf("title ") === 0)    _cardPut(cards, l.slice(6), "title")
            else if (l.indexOf("purpose ") === 0)  _cardPut(cards, l.slice(8), "purpose")
            else if (l.indexOf("capfault ") === 0) _cardPut(cards, l.slice(9), "capfault")
        }
        root.profiles = rows; root.rawEntries = raws; root.cardText = cards; root.available = true
        if (busyProfile.length > 0) {
            var bp = root.profileByName(busyProfile)
            if (bp && _busyWasBlessed >= 0 && (bp.blessed ? 1 : 0) !== _busyWasBlessed) root.busyProfile = ""
        }
        // Load-bearing marker: the headless render proof greps this to confirm file -> model -> panel.
        console.log("SHREK-CONNECTIVITY egress state profiles=" + rows.length
            + " raw=" + raws.length + " wants=" + root.wants.length + " available=1")
    }

    // `<name> <rest...>` for title/purpose; `<name> source=<s> <rest...>` for capfault. rest-of-line values.
    function _cardPut(cards, rest, key) {
        var sp = rest.indexOf(" ")
        if (sp < 0) return
        var name = rest.slice(0, sp), val = rest.slice(sp + 1)
        if (!cards[name]) cards[name] = { title: "", purpose: "", capfault: "" }
        if (key === "capfault") {
            // drop a leading source=<s> token if present; keep the human reason
            var v = val, m = v.match(/^source=\S+\s+(.*)$/)
            cards[name].capfault = m ? m[1] : v
        } else cards[name][key] = val
    }

    function _parseProfileLine(l) {
        var toks = l.split(/\s+/)
        if (toks[0] !== "profile" || toks.length < 2) return null
        var row = { name: toks[1], tier: "", blessed: false, pins: [], refreshed: 0, fault: "-",
                    source: "", feature: "", hasPins: false, faulted: false, live: false, pending: false }
        for (var i = 2; i < toks.length; i++) {
            var kv = toks[i].split("="); if (kv.length !== 2) continue
            var k = kv[0], v = kv[1]
            if (k === "tier") row.tier = v
            else if (k === "blessed") row.blessed = v === "1"
            else if (k === "pins") row.pins = (v === "-") ? [] : v.split(",")
            else if (k === "refreshed") row.refreshed = (v === "-") ? 0 : (parseInt(v) || 0)
            else if (k === "fault") row.fault = v
            else if (k === "source") row.source = v
            else if (k === "feature") row.feature = v
        }
        row.hasPins = row.pins.length > 0
        row.faulted = row.fault !== "-"
        row.live = row.blessed && row.hasPins && !row.faulted
        row.pending = row.blessed && !row.live
        return row
    }

    function _parseRawLine(l) {
        var toks = l.split(/\s+/); if (toks[0] !== "raw") return null
        var r = { host: "", proto: "", port: "", blessed: false, pins: [], refreshed: 0 }
        for (var i = 1; i < toks.length; i++) {
            var kv = toks[i].split("="); if (kv.length !== 2) continue
            var k = kv[0], v = kv[1]
            if (k === "host") r.host = v
            else if (k === "proto") r.proto = v
            else if (k === "port") r.port = v
            else if (k === "blessed") r.blessed = v === "1"
            else if (k === "pins") r.pins = (v === "-") ? [] : v.split(",")
            else if (k === "refreshed") r.refreshed = (v === "-") ? 0 : (parseInt(v) || 0)
        }
        if (!r.host || !r.proto || !r.port) return null
        r.hasPins = r.pins.length > 0; r.pending = !r.hasPins; r.wire = r.host + ":" + r.proto + ":" + r.port
        return r
    }

    // `want <token> <unix>` lines (store::record_want map; sorted, bounded). Closed token, never free text.
    function _ingestWants(text) {
        var lines = ("" + text).split("\n"); var out = []
        for (var i = 0; i < lines.length; i++) {
            var t = lines[i].trim().split(/\s+/)
            if (t.length >= 2 && t[0] === "want") out.push({ token: t[1], ts: parseInt(t[2]) || 0 })
        }
        root.wants = out
    }

    function _ingestEvents(text) {
        var lines = ("" + text).split("\n").filter(function (s) { return s.trim().length > 0 })
        if (lines.length === 0) return
        var raw = lines[lines.length - 1]
        if (raw === _lastEventRaw) return
        _lastEventRaw = raw
        var t = raw.split(/\s+/)
        root.lastEvent = { ts: parseInt(t[0]) || 0, verb: t[1] || "", profile: t[2] || "", result: t.slice(3).join(" ") }
    }

    // --- presentation helpers -------------------------------------------------------------------------
    function friendlyName(p) {
        if (p.name === "desktop-ntp") return "Time sync"
        if (p.name === "desktop-updates") return "System updates"
        var c = root.cardText[p.name]
        if (c && c.title.length > 0) return c.title
        if (p.name === "weather") return "Weather"
        if (p.name === "web-browsing") return "Web browsing"
        return p.name
    }
    function purposeText(p) {
        var c = root.cardText[p.name]
        if (c && c.purpose.length > 0) return c.purpose
        if (p.name === "desktop-ntp") return "Keeps the clock correct (secure NTP)."
        if (p.name === "desktop-updates") return "Fetches signed system updates."
        if (p.name === "weather") return "Lets the weather widget reach its forecast API."
        if (p.name === "web-browsing") return "Opens broad internet access for the browser."
        return ""
    }
    function statusText(p) {
        if (p.tier === "baseline") return "On"
        if (p.fault === "quarantined") return "Disabled — needs attention"
        if (!p.blessed) return p.tier === "ceremony" ? "Needs console approval" : "Off"
        if (p.live) return "Active"
        if (p.fault === "apply-fail") return "Needs attention — will retry"
        return "Waiting for network"
    }
    function statusTint(p) {
        if (p.tier === "baseline") return root.cOk
        if (p.fault === "quarantined" || p.fault === "apply-fail") return root.cDanger
        if (!p.blessed) return root.cSurfaceDim
        if (p.live) return root.cOk
        return root.cWaiting
    }

    // The only inter-process seam. Named "shrek-connectivity"; addressed via `qs -p <this file> ipc call`.
    IpcHandler {
        target: "shrek-connectivity"
        function toggle(): void { if (win.visible) win.visible = false; else root.showPanel(); }
        function show(): void { root.showPanel(); }
        function hide(): void { win.visible = false }
    }
    function showPanel() { root.reload(); win.visible = true; }

    PanelWindow {
        id: win
        visible: false
        color: "transparent"

        implicitWidth: 600
        implicitHeight: Math.min(760, contentCol.implicitHeight + 2 * 18 + header.height + 12)

        WlrLayershell.layer: WlrLayer.Overlay
        WlrLayershell.keyboardFocus: WlrKeyboardFocus.Exclusive
        WlrLayershell.namespace: "shrek-connectivity"

        onVisibleChanged: if (visible) card.forceActiveFocus()

        Rectangle {
            id: card
            anchors.fill: parent
            focus: true
            color: root.cSurface
            radius: 16
            border.width: 1
            border.color: root.cOutline

            Keys.onPressed: function (event) {
                if (event.key === Qt.Key_Escape) { win.visible = false; event.accepted = true }
            }

            Column {
                anchors.fill: parent
                anchors.margins: 18
                spacing: 12

                // Header.
                Item {
                    id: header
                    width: parent.width
                    height: 40
                    Column {
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 1
                        Text { text: "Network Access"; color: root.cPrimary; font.pixelSize: 18; font.bold: true }
                        Text { text: "Your desktop starts sealed. Choose what it may reach."; color: root.cSurfaceDim; font.pixelSize: 12 }
                    }
                    Text {
                        anchors.right: parent.right; anchors.verticalCenter: parent.verticalCenter
                        text: "Esc to close"; color: root.cSurfaceDim; font.pixelSize: 11
                    }
                }
                Rectangle { width: parent.width; height: 1; color: root.cOutline }

                // Fail-closed banner + ceremony hint.
                Text {
                    width: parent.width; visible: !root.available
                    text: "Connectivity status is unavailable right now."
                    color: root.cSurfaceDim; font.pixelSize: 12; wrapMode: Text.WordWrap
                }
                Rectangle {
                    width: parent.width; visible: root.ceremonyActive
                    height: visible ? ceremonyCol.implicitHeight + 20 : 0
                    radius: 10; color: root.cSelected; border.width: 1; border.color: root.cPrimary
                    Column {
                        id: ceremonyCol
                        anchors.fill: parent; anchors.margins: 10; spacing: 3
                        Text { text: "Approve at the console"; color: root.cPrimary; font.pixelSize: 13; font.bold: true }
                        Text {
                            width: parent.width
                            text: (root.ceremonyLabel.length > 0 ? (root.ceremonyLabel + ": ") : "")
                                  + "press the Secure Attention key (Ctrl-Alt-Break), then type the code shown on the secure screen. Anything else denies."
                            color: root.cSurfaceText; font.pixelSize: 11; wrapMode: Text.WordWrap
                        }
                    }
                }

                Flickable {
                    id: scroller
                    width: parent.width
                    height: parent.height - header.height - 12 - parent.spacing * 3
                          - (root.ceremonyActive ? (ceremonyCol.implicitHeight + 20 + parent.spacing) : 0)
                    contentWidth: width
                    contentHeight: contentCol.implicitHeight
                    clip: true
                    boundsBehavior: Flickable.StopAtBounds

                    Column {
                        id: contentCol
                        width: scroller.width
                        spacing: 14

                        // ── Section 1: System baseline (status-only; revoke is console-only, Q8) ──────────
                        SectionHeader { text: "System baseline"; sub: "Always on — turned off only at the console." }
                        Repeater {
                            model: root.available ? root.baselineProfiles : []
                            Row {
                                required property var modelData
                                width: contentCol.width
                                CardBox {
                                    width: contentCol.width
                                    Column {
                                        width: parent.width; spacing: 2
                                        Row {
                                            width: parent.width
                                            Text { text: root.friendlyName(modelData); color: root.cSurfaceText; font.pixelSize: 14; font.bold: true; width: parent.width - stat.width; elide: Text.ElideRight }
                                            Text { id: stat; text: root.statusText(modelData); color: root.statusTint(modelData); font.pixelSize: 12; font.bold: true }
                                        }
                                        Text { width: parent.width; text: root.purposeText(modelData); color: root.cSurfaceDim; font.pixelSize: 11; wrapMode: Text.WordWrap }
                                    }
                                }
                            }
                        }

                        // ── Section 2: Features (weather one-click; web-browsing ceremony; owner display-only) ──
                        SectionHeader { text: "Features"; sub: "Each choice is pinned to a specific destination — nothing else opens." }
                        Repeater {
                            model: root.available ? root.featureProfiles : []
                            CardBox {
                                required property var modelData
                                readonly property bool isWeather: modelData.tier === "one-click" && modelData.source !== "owner"
                                readonly property bool isCeremony: modelData.tier === "ceremony"
                                readonly property bool isOwner: modelData.source === "owner"
                                width: contentCol.width
                                Column {
                                    width: parent.width; spacing: 4
                                    Row {
                                        width: parent.width; spacing: 8
                                        Column {
                                            width: parent.width - ctrl.width - 8
                                            Row {
                                                spacing: 6
                                                Text { text: root.friendlyName(modelData); color: root.cSurfaceText; font.pixelSize: 14; font.bold: true }
                                                // source badge — owner-installed capabilities are visibly distinct.
                                                Rectangle {
                                                    visible: isOwner; radius: 4; color: root.cSelected
                                                    height: 16; width: ownerLbl.implicitWidth + 10; anchors.verticalCenter: parent.verticalCenter
                                                    Text { id: ownerLbl; anchors.centerIn: parent; text: "installed"; color: root.cPrimary; font.pixelSize: 9; font.bold: true }
                                                }
                                            }
                                            Text { width: parent.width; text: root.purposeText(modelData); color: root.cSurfaceDim; font.pixelSize: 11; wrapMode: Text.WordWrap }
                                            Text {
                                                width: parent.width
                                                visible: modelData.hasPins
                                                text: "Pinned: " + modelData.pins.join(", ")
                                                color: root.cSurfaceDim; font.pixelSize: 10
                                            }
                                        }
                                        // control column: toggle (weather), ceremony button (web-browsing),
                                        // or a status word (owner display-only in S4).
                                        Item {
                                            id: ctrl
                                            width: 132; height: Math.max(28, childrenRect.height)
                                            anchors.top: parent.top
                                            // weather one-click toggle
                                            Pill {
                                                visible: isWeather
                                                anchors.right: parent.right
                                                on: modelData.blessed
                                                busy: root.busy(modelData.name)
                                                onClicked: { if (modelData.blessed) root.unbless(modelData.name); else root.bless(modelData.name) }
                                            }
                                            // ceremony button
                                            Btn {
                                                visible: isCeremony
                                                anchors.right: parent.right
                                                text: modelData.blessed ? "Turn off (console)" : "Set up at console"
                                                enabled: !root.ceremonyActive
                                                onClicked: { if (modelData.blessed) root.unblessCeremony(modelData.name); else root.blessCeremony(modelData.name) }
                                            }
                                            // owner display-only status
                                            Text {
                                                visible: isOwner
                                                anchors.right: parent.right; anchors.verticalCenter: parent.verticalCenter
                                                text: root.statusText(modelData)
                                                color: root.statusTint(modelData); font.pixelSize: 12; font.bold: true
                                            }
                                        }
                                    }
                                    // weather retry when blessed-but-pending
                                    Btn {
                                        visible: isWeather && modelData.pending
                                        text: "Try now"; enabled: !root.busy(modelData.name)
                                        onClicked: root.repin(modelData.name)
                                    }
                                    // owner-cap quarantine reason (S3 §4.4) — legible, never silent.
                                    Text {
                                        width: parent.width
                                        visible: isOwner && modelData.fault === "quarantined" && root.cardText[modelData.name] && root.cardText[modelData.name].capfault.length > 0
                                        text: root.cardText[modelData.name] ? root.cardText[modelData.name].capfault : ""
                                        color: root.cDanger; font.pixelSize: 10; wrapMode: Text.WordWrap
                                    }
                                }
                            }
                        }

                        // ── Section 3: Pending needs (the closed-token request inbox) ────────────────────
                        SectionHeader {
                            text: "Pending needs"
                            sub: "Apps can ask for a capability by name. Enable it above, or set it up at the console."
                            visible: root.wants.length > 0
                        }
                        Repeater {
                            model: root.available ? root.wants : []
                            CardBox {
                                required property var modelData
                                width: contentCol.width
                                Row {
                                    width: parent.width
                                    Text { text: modelData.token; color: root.cSurfaceText; font.pixelSize: 13; width: parent.width - reqLbl.width; elide: Text.ElideRight }
                                    Text { id: reqLbl; text: "requested"; color: root.cWaiting; font.pixelSize: 11; font.bold: true }
                                }
                            }
                        }

                        // ── Section 4: Advanced destinations (raw editor + last activity) ────────────────
                        SectionHeader { text: "Advanced destinations"; sub: "Allow a specific host, protocol and port — approved at the console, pinned to the address it resolves to." }
                        Repeater {
                            model: root.available ? root.rawEntries : []
                            CardBox {
                                required property var modelData
                                width: contentCol.width
                                Row {
                                    width: parent.width; spacing: 8
                                    Column {
                                        width: parent.width - rmBtn.width - 8
                                        Text { text: modelData.wire; color: root.cSurfaceText; font.pixelSize: 13; elide: Text.ElideRight; width: parent.width }
                                        Text { text: modelData.hasPins ? ("Active · Pinned: " + modelData.pins.join(", ")) : "Waiting for network"; color: root.cSurfaceDim; font.pixelSize: 10 }
                                    }
                                    Btn { id: rmBtn; text: "Remove"; enabled: !root.ceremonyActive; onClicked: root.removeRaw(modelData.wire); anchors.verticalCenter: parent.verticalCenter }
                                }
                            }
                        }
                        // add row
                        CardBox {
                            width: contentCol.width
                            Row {
                                width: parent.width; spacing: 8
                                Rectangle {
                                    width: parent.width - addBtn.width - 8; height: 30; radius: 6
                                    color: root.cCard; border.width: 1
                                    border.color: rawInput.activeFocus ? root.cPrimary : root.cOutline
                                    anchors.verticalCenter: parent.verticalCenter
                                    TextInput {
                                        id: rawInput
                                        anchors.fill: parent; anchors.leftMargin: 8; anchors.rightMargin: 8
                                        verticalAlignment: TextInput.AlignVCenter; clip: true
                                        color: root.cSurfaceText; font.pixelSize: 12
                                        selectionColor: root.cSelected
                                        Text { anchors.verticalCenter: parent.verticalCenter; visible: rawInput.text.length === 0; text: "example.com:tcp:443"; color: root.cSurfaceDim; font: rawInput.font }
                                    }
                                }
                                Btn {
                                    id: addBtn; text: "Add at console"
                                    enabled: !root.ceremonyActive && rawInput.text.trim().length > 0
                                    onClicked: { root.addRaw(rawInput.text.trim()); rawInput.text = "" }
                                    anchors.verticalCenter: parent.verticalCenter
                                }
                            }
                        }

                        // Last activity from the downstream notification log.
                        Text {
                            width: contentCol.width
                            visible: root.lastEvent !== null
                            text: root.lastEvent
                                  ? ("Last change: " + root.lastEvent.profile + " — " + root.lastEvent.verb + " (" + root.lastEvent.result + ")")
                                  : ""
                            color: root.cSurfaceDim; font.pixelSize: 10; wrapMode: Text.WordWrap
                        }
                    }
                }
            }
        }
    }

    // ── small reusable UI atoms (kept inline; a second qs process can't import ui-v2 components) ──────
    component SectionHeader: Column {
        property string text: ""
        property string sub: ""
        width: parent ? parent.width : 0
        spacing: 1
        Text { text: parent.text; color: root.cPrimary; font.pixelSize: 13; font.bold: true }
        Text { width: parent.width; visible: parent.sub.length > 0; text: parent.sub; color: root.cSurfaceDim; font.pixelSize: 11; wrapMode: Text.WordWrap }
    }

    component CardBox: Rectangle {
        default property alias content: inner.data
        implicitHeight: inner.childrenRect.height + 20
        radius: 10
        color: root.cCard
        border.width: 1
        border.color: root.cOutline
        Item { id: inner; anchors.fill: parent; anchors.margins: 10 }
    }

    // A one-click on/off pill (no optimistic flip: reflects projected `on`, disabled while busy).
    component Pill: Rectangle {
        property bool on: false
        property bool busy: false
        signal clicked()
        width: 56; height: 26; radius: 13
        opacity: busy ? 0.5 : 1.0
        color: on ? root.cPrimary : root.cCard
        border.width: 1; border.color: on ? root.cPrimary : root.cOutline
        Rectangle {
            width: 20; height: 20; radius: 10
            y: 3; x: parent.on ? parent.width - width - 3 : 3
            color: parent.on ? root.cOnPrimary : root.cSurfaceDim
            Behavior on x { NumberAnimation { duration: 120 } }
        }
        MouseArea { anchors.fill: parent; enabled: !parent.busy; cursorShape: Qt.PointingHandCursor; onClicked: parent.clicked() }
    }

    component Btn: Rectangle {
        property string text: ""
        property bool enabled: true
        signal clicked()
        implicitWidth: label.implicitWidth + 20
        width: implicitWidth; height: 28; radius: 8
        opacity: enabled ? 1.0 : 0.4
        color: root.cSelected; border.width: 1; border.color: root.cOutline
        Text { id: label; anchors.centerIn: parent; text: parent.text; color: root.cPrimary; font.pixelSize: 12; font.bold: true }
        MouseArea { anchors.fill: parent; enabled: parent.enabled; cursorShape: Qt.PointingHandCursor; onClicked: parent.clicked() }
    }
}
