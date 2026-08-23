// SPDX-License-Identifier: GPL-3.0-only
//
// Shrek Shell derivative port of Caelestia Shell's modules/drawers/Interactions.qml.
// Adapts the edge/bar hover routing to Shrek's ShellState and Sway-safe
// presentation model.

pragma ComponentBehavior: Bound

import QtQuick
import Quickshell
import "../themes"
import "../state"

MouseArea {
    id: root

    required property var screen
    required property Item popouts
    required property Item panels
    required property Item bar
    required property real borderThickness
    required property bool fullscreen

    property point dragStart
    property bool dashboardShortcutActive
    property bool launcherShortcutActive
    property bool rightShortcutActive

    function withinPanelHeight(panel: Item, x: real, y: real): bool {
        const panelY = borderThickness + panel.y
        return y >= panelY - Tokens.radiusLg && y <= panelY + panel.height + Tokens.radiusLg
    }

    function withinPanelWidth(panel: Item, x: real, y: real): bool {
        const panelX = bar.implicitWidth + panel.x
        return x >= panelX - Tokens.radiusLg && x <= panelX + panel.width + Tokens.radiusLg
    }

    function inLeftPanel(panel: Item, x: real, y: real): bool {
        return x < bar.implicitWidth + panel.x + panel.width && withinPanelHeight(panel, x, y)
    }

    function inRightPanel(panel: Item, x: real, y: real): bool {
        return x > Math.min(width - Tokens.spaceSm, bar.implicitWidth + panel.x) && withinPanelHeight(panel, x, y)
    }

    function inTopPanel(panel: Item, x: real, y: real): bool {
        const panelHeight = panel.height * (1 - panel.offsetScale)
        return y < Math.max(Tokens.spaceSm, borderThickness + panelHeight) && withinPanelWidth(panel, x, y)
    }

    function inBottomPanel(panel: Item, x: real, y: real): bool {
        const panelHeight = panel.height * (1 - panel.offsetScale)
        return y > height - Math.max(Tokens.spaceSm, borderThickness + panelHeight) && withinPanelWidth(panel, x, y)
    }

    anchors.fill: parent
    acceptedButtons: fullscreen ? Qt.NoButton : Qt.AllButtons
    hoverEnabled: true

    onPressed: event => dragStart = Qt.point(event.x, event.y)

    onClicked: event => {
        if (event.button === Qt.RightButton) {
            ShellState.openMenu(event.x, event.y, [])
        }
    }

    onContainsMouseChanged: {
        if (!containsMouse) {
            if (!dashboardShortcutActive)
                ShellState.dashboardOpen = false
            if (!launcherShortcutActive)
                ShellState.launcherOpen = false
            if (!rightShortcutActive)
                ShellState.rightEdgeHot = false
            ShellState.closeRailPopout()
            bar.closeTray()
        }
    }

    onPositionChanged: event => {
        if (fullscreen)
            return

        const x = event.x
        const y = event.y
        const dragX = x - dragStart.x
        const dragY = y - dragStart.y

        if (x < bar.implicitWidth) {
            bar.isHovered = true
            bar.checkPopout(y)
        } else if (!inLeftPanel(popouts, x, y)) {
            ShellState.closeRailPopout()
            bar.closeTray()
        }

        if (!rightShortcutActive && !inRightPanel(panels.edgeDock, x, y))
            ShellState.rightEdgeHot = false

        if (!dashboardShortcutActive)
            ShellState.dashboardOpen = inTopPanel(panels.dashboard, x, y)

        if (!launcherShortcutActive)
            ShellState.launcherOpen = inBottomPanel(panels.launcher, x, y)

        if (pressed && dragStart.x > width - Tokens.spaceLg && dragX < -Tokens.spaceLg) {
            ShellState.openRightEdge()
            rightShortcutActive = true
        }

        if (pressed && inTopPanel(panels.dashboard, dragStart.x, dragStart.y)) {
            if (dragY > Tokens.spaceLg) {
                ShellState.dashboardOpen = true
                dashboardShortcutActive = true
            } else if (dragY < -Tokens.spaceLg) {
                ShellState.dashboardOpen = false
                dashboardShortcutActive = false
            }
        }

        if (pressed && inBottomPanel(panels.launcher, dragStart.x, dragStart.y)) {
            if (dragY < -Tokens.spaceLg) {
                ShellState.launcherOpen = true
                launcherShortcutActive = true
            } else if (dragY > Tokens.spaceLg) {
                ShellState.launcherOpen = false
                launcherShortcutActive = false
            }
        }
    }

    Keys.onEscapePressed: {
        dashboardShortcutActive = false
        launcherShortcutActive = false
        rightShortcutActive = false
        ShellState.closeAll()
    }
}
