pragma Singleton
import QtQuick
import Quickshell.Services.UPower

// Power — battery/charge, behind one seam. Native UPower over D-Bus. The display device is an aggregate
// that always exists; `present` is true only for a real laptop battery, so a desktop/VM (no battery)
// simply shows nothing. Read-only.
QtObject {
    id: root

    readonly property var device: UPower.displayDevice
    readonly property bool present: root.device ? (root.device.isLaptopBattery && root.device.isPresent) : false
    readonly property real percentage: root.device ? root.device.percentage : 0
    readonly property bool onBattery: UPower.onBattery
    readonly property bool charging: root.device
        ? (root.device.state === UPowerDeviceState.Charging || root.device.state === UPowerDeviceState.FullyCharged)
        : false
}
