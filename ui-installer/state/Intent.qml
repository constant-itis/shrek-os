pragma Singleton
import QtQuick

// Intent — the installer's collected install intent + linear flow state (ADR-005 §6, the "collect" step).
// A mutable singleton (mirrors the theme/Tokens singleton pattern) so every screen reads and writes the
// SAME state as the user moves through the flow. NON-SECRET ONLY: locale, keymap, owner display name. The
// passphrase is NEVER collected here — it is set target-side at first run (§3). At the point of no return
// (EraseConfirm) this state is handed to /usr/libexec/shrek/shrek-provision-collect, which writes the
// file-legible collect file that shrek-provision-stage validates + stages for the target transplant.
QtObject {
    id: intent

    // ── collected intent (non-secret) ───────────────────────────────────────────
    // Defaults are the §5a baked defaults; M1 ships these wired to the flow. Rich locale/keymap pickers are
    // a follow-up — the value+label pair is already the single source the collect bridge reads.
    property string locale:      "en_US.UTF-8"
    property string localeLabel: "English (United States)"
    property string keymap:      "us"
    property string keymapLabel: "English (US)"
    property string ownerName:   ""
    readonly property int schemaVersion: 1

    // ── linear flow ─────────────────────────────────────────────────────────────
    readonly property var order: ["welcome", "locale", "name", "disk", "erase", "progress", "done"]
    property int step: 0
    readonly property string screen: order[step]
    property bool committed: false          // set once the collect bridge has been invoked (EraseConfirm)

    function next() { if (step < order.length - 1) step += 1 }
    function back() { if (step > 0) step -= 1 }
    function jumpTo(name) { var i = order.indexOf(name); if (i >= 0) step = i }
}
