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

    property NotificationServer server: NotificationServer {
        keepOnReload: false
        bodySupported: true
        actionsSupported: false     // Slice 1: display + dismiss; action buttons later
        imageSupported: true
        onNotification: function (n) { n.tracked = true }
    }
}
