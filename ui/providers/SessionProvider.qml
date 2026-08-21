import QtQuick
import Quickshell.Io

// SessionProvider — the REAL, read-only Work-drawer data source (Phase-8 Slice-1,
// docs/phase8-slice1-agent-session.md). It reads the gatekeeperd-authored effective-
// authority view records `$SHREK_SESSION_DIR/*.json` (default /run/shrek/session,
// schema `shrek-session/1`) and projects each into an opaque row the WorkDrawer
// renders generically.
//
// This is the SECOND consumer of the exact record `shrek session status` reads — one
// schema, writer (gatekeeperd/session_view.rs) and reader identical. STRICTLY read-only:
// it enumerates + cats records, mints no authority, and offers no mutation surface
// (no grant/stop/promote). Absence of a record == session ended; a malformed record is
// skipped, never rendered (fail-closed, matching `shrek session status`).
QtObject {
    id: root

    // Each entry is an opaque row { session, title, subtitle, tier, trust, state }.
    // The WorkDrawer only relies on `title`; the rest is non-authoritative display data.
    property var sessions: []
    property bool available: true

    function reload() { proc.running = true }

    // Concatenate every record into one form-feed (0x0C) delimited stream. FF can never
    // appear raw in a record (gatekeeperd escapes control bytes as \uXXXX), so it is a safe
    // separator. Pure read: `cat` only. SHREK_SESSION_DIR default matches gatekeeperd + shrek.
    property Process proc: Process {
        command: ["sh", "-c",
            "d=\"${SHREK_SESSION_DIR:-/run/shrek/session}\"; for f in \"$d\"/*.json; do [ -e \"$f\" ] || continue; cat \"$f\"; printf '\\f'; done"]
        stdout: StdioCollector { id: collector; onStreamFinished: root._ingest(collector.text) }
    }

    // Poll the record dir for a live read model. Not cosmetic — it keeps the read-only view
    // in step with construct/teardown of sessions. First read fires immediately on load.
    property Timer timer: Timer {
        interval: 2000; running: true; repeat: true
        onTriggered: root.reload()
        Component.onCompleted: root.reload()
    }

    function _ingest(text) {
        var rows = []
        var chunks = ("" + text).split(String.fromCharCode(12))
        for (var i = 0; i < chunks.length; i++) {
            var s = chunks[i].trim()
            if (s.length === 0)
                continue
            var o
            try {
                o = JSON.parse(s)
            } catch (e) {
                // Per-record fail-closed: a malformed record is skipped, never rendered.
                continue
            }
            if (!o || o.schema !== "shrek-session/1")
                continue
            var eff = o.effective || {}
            rows.push({
                session: o.session,
                title: (o.session || "session") + "  ·  " + (eff.tier || "?") + " / " + (eff.trust || "?"),
                subtitle: (o.subject || ""),
                tier: eff.tier || "",
                trust: eff.trust || "",
                state: o.state || ""
            })
            // Load-bearing marker: the desktop-session proof greps this to confirm a real
            // gatekeeperd record flowed daemon -> provider -> drawer.
            console.log("SHREK-DESKTOP work session " + o.session
                + " tier=" + (eff.tier || "?") + " trust=" + (eff.trust || "?"))
        }
        root.sessions = rows
    }
}
