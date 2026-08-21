import QtQuick

// SessionProvider — the swappable data SEAM for the future Work drawer.
//
// Bootstrap-0 defines the INTERFACE ONLY. Do NOT bake the final session schema here:
// `sessions` is an opaque list of rows the WorkDrawer renders generically, with no
// tier / authority / grant fields (all out of scope for Bootstrap 0). The real
// provider — and the versioned read model that mirrors the Phase-8 Slice-1
// session-view record — replaces MockSessionProvider at the WorkDrawer binding site.
QtObject {
    // Each entry is an opaque { title, subtitle } row. Non-authoritative by design.
    property var sessions: []
    property bool available: true
}
