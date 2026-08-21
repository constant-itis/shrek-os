import QtQuick

// MockSessionProvider — Bootstrap-0 stand-in provider.
//
// Returns NO running sessions, so the Work drawer renders its empty state
// ("Nothing running"). No fake data, no demo session pretending the backend
// contract exists. Swapping this for a real provider is a one-line change at the
// WorkDrawer binding site in Shell.qml.
SessionProvider {
    sessions: []
    available: true
}
