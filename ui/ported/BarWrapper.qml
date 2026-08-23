// SPDX-License-Identifier: GPL-3.0-only
//
// Shrek Shell derivative port of Caelestia Shell's modules/bar/BarWrapper.qml.
// Preserves the wrapper/width/hover contract while adapting content to Shrek's
// Rail and ShellState.

pragma ComponentBehavior: Bound

import QtQuick
import "../themes"
import "../state"
import "../surfaces/bar"

Item {
    id: root

    property var session
    property var win

    readonly property int padding: Tokens.spaceSm
    readonly property int contentWidth: Tokens.railWidth + padding * 2
    readonly property int clampedWidth: Math.max(Tokens.spaceSm, implicitWidth)
    readonly property bool shouldBeVisible: true
    property bool isHovered

    function closeTray(): void {
        ShellState.closeRailPopout()
    }

    function checkPopout(y: real): void {
        const localY = Math.max(Tokens.spaceSm, Math.min(y, height - 80))
        ShellState.openRailPopout("bar", localY)
    }

    function handleWheel(y: real, angleDelta: point): void {
    }

    clip: true
    implicitWidth: contentWidth

    Rail {
        anchors {
            top: parent.top
            bottom: parent.bottom
            left: parent.left
            topMargin: Tokens.spaceSm
            bottomMargin: Tokens.spaceSm
            leftMargin: Tokens.spaceSm
        }
        width: Tokens.railWidth
        session: root.session
        window: root.win
    }
}
