import QtQuick
import "../../themes"
import "../../state"

// ThemeToggle — a rail button that cycles the appearance mode (dynamic -> dark -> light -> high-contrast)
// via the theme system. Rendered as a split accent/surface swatch: reads as a "switch theme" affordance
// AND live-previews the current palette (the two tones repaint with each mode). Routes through ShellState
// so the surface never references the Theme controller directly (check-tokens keeps the token chokepoint).
Rectangle {
    id: root
    width: 26; height: 26
    radius: Tokens.radius
    clip: true
    color: "transparent"
    border.color: hovered ? Tokens.borderStrong : Tokens.border
    border.width: 1

    property bool hovered: ma.containsMouse

    Row {
        anchors.fill: parent
        anchors.margins: 2
        Rectangle { width: parent.width / 2; height: parent.height; color: Tokens.accent }
        Rectangle { width: parent.width / 2; height: parent.height; color: Tokens.surface }
    }

    MouseArea {
        id: ma
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onEntered: ShellState.openRailPopout("theme", root.mapToItem(null, 0, root.height / 2).y)
        onExited: ShellState.closeRailPopout("theme")
        onClicked: ShellState.cycleTheme()
    }
}
