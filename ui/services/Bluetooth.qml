pragma Singleton
import QtQuick
import Quickshell.Bluetooth

// Bluetooth — the default BlueZ adapter + devices, behind one seam. Native org.bluez over D-Bus. In a
// VM with no controller, defaultAdapter is null and `available` is false (honest empty state, no fake
// adapter). Ordinary user control (power toggle); no authority.
QtObject {
    id: root

    readonly property var adapter: Bluetooth.defaultAdapter
    readonly property bool available: root.adapter !== null
    readonly property bool enabled: root.adapter ? root.adapter.enabled : false
    readonly property var devices: Bluetooth.devices

    function setEnabled(on) { if (root.adapter) root.adapter.enabled = on }
    function toggle() { if (root.adapter) root.adapter.enabled = !root.adapter.enabled }
}
