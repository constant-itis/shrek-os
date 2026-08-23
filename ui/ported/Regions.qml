// SPDX-License-Identifier: GPL-3.0-only
//
// Shrek Shell derivative port of Caelestia Shell's modules/drawers/Regions.qml.
// Copyright notices and the upstream GPLv3 license are preserved under
// third_party/caelestia-shell. This file adapts the region composition model to
// Shrek's Sway/Quickshell shell state and surfaces.

pragma ComponentBehavior: Bound

import QtQuick
import Quickshell
import "../themes"

Region {
    id: root

    required property Item bar
    required property Item panels
    required property var win

    readonly property real borderThickness: win.borderLayoutThickness
    readonly property real dragMaskPadding: win.dragMaskPadding

    x: bar.clampedWidth + dragMaskPadding
    y: borderThickness + dragMaskPadding
    width: win.width - bar.clampedWidth - borderThickness - dragMaskPadding * 2
    height: win.height - borderThickness * 2 - dragMaskPadding * 2
    intersection: Intersection.Xor

    R {
        panel: root.panels.dashboard
        y: 0
        height: panel.height * (1 - panel.offsetScale) + root.borderThickness
    }

    R {
        panel: root.panels.launcher
        y: root.win.height - height
        height: panel.height * (1 - panel.offsetScale) + root.borderThickness
    }

    R {
        panel: root.panels.sidePanel
        x: root.win.width - width
        width: panel.width * (1 - panel.offsetScale) + root.borderThickness
    }

    R {
        panel: root.panels.popoutsWrapper
        width: panel.width * (1 - panel.offsetScale)
    }

    R {
        panel: root.panels.edgeDock
        x: root.win.width - width
        width: panel.width * (1 - panel.offsetScale) + root.borderThickness
    }

    component R: Region {
        required property Item panel

        x: panel.x + root.bar.implicitWidth
        y: panel.y + root.borderThickness
        width: panel.width
        height: panel.height
        intersection: Intersection.Subtract
    }
}
