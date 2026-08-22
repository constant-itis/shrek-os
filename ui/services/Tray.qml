pragma Singleton
import QtQuick
import Quickshell.Services.SystemTray

// Tray — the system-tray (StatusNotifierItem) read model, behind ONE seam. Surfaces import this, never
// Quickshell.Services.SystemTray directly. Backed by Quickshell's StatusNotifier host + watcher (the
// freedesktop/KDE tray protocol over DBus). Presentation + ordinary item dispatch (activate / secondary /
// scroll / the item's own platform menu) only — no authority here.
QtObject {
    // The ObjectModel itself (usable directly as a Repeater model) + its JS-array view for counts.
    readonly property var model: SystemTray.items
    readonly property var items: SystemTray.items ? SystemTray.items.values : []
    readonly property int count: items.length
}
