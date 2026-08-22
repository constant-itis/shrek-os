pragma Singleton
import QtQuick
import Quickshell.Services.Notifications

// Notifications — Shrek IS the notification server (Quickshell implements org.freedesktop.Notifications
// directly; one audited surface, no separate mako/dunst daemon). Incoming notifications are tracked so
// surfaces can render them; each Notification carries summary/body/appName/urgency and dismiss(). Read +
// ordinary dismiss only; no authority.
QtObject {
    id: root

    readonly property var list: server.trackedNotifications

    // Do Not Disturb: when on, incoming notifications are NOT tracked (no toast) — the sender still gets a
    // valid reply, the notification just doesn't interrupt. A quiet, honest mute of the attention surface.
    property bool dnd: false
    function toggleDnd() { root.dnd = !root.dnd }

    property NotificationServer server: NotificationServer {
        keepOnReload: false
        bodySupported: true
        actionsSupported: true      // render + invoke the sender's action buttons (display + dismiss + act)
        actionIconsSupported: false
        imageSupported: true
        onNotification: function (n) { if (!root.dnd) n.tracked = true }
    }
}
