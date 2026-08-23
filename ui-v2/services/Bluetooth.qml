pragma Singleton
import QtQuick
import Quickshell
import Quickshell.Bluetooth

// Bluetooth — BlueZ adapter and known devices. Pairing UX is deliberately deferred; DAILY-DRIVER-0
// exposes power plus connect/disconnect for devices BlueZ already knows.
QtObject {
    id: root

    readonly property var adapter: Bluetooth.defaultAdapter
    readonly property bool available: root.adapter !== null
    readonly property bool enabled: root.adapter ? root.adapter.enabled : false
    readonly property var devices: Bluetooth.devices
    readonly property int connectedCount: _connectedCount()

    function setEnabled(on): void { if (root.adapter) root.adapter.enabled = on }
    function toggle(): void { if (root.adapter) root.adapter.enabled = !root.adapter.enabled }
    function connectDevice(device): void {
        if (device && device.address)
            Quickshell.execDetached(["bluetoothctl", "connect", device.address])
    }
    function disconnectDevice(device): void {
        if (device && device.address)
            Quickshell.execDetached(["bluetoothctl", "disconnect", device.address])
    }

    function label(device) {
        if (!device) return "Device"
        return device.name || device.alias || device.address || "Device"
    }
    function connected(device) { return device && device.connected }

    function _connectedCount() {
        var n = 0
        if (!devices) return 0
        for (var i = 0; i < devices.count; i++) {
            var d = devices.get ? devices.get(i) : devices[i]
            if (d && d.connected) n++
        }
        return n
    }
}
