pragma Singleton
import QtQuick

// Config — LAYOUT geometry + motion tokens (px / ms) for shell-v2 composition. Colour + typography live
// in the semantic theme system (theme/Tokens.qml -> Theme.c); this file owns only the shell's spatial
// contract (rail/panel/work widths, frame insets, animation duration).
QtObject {
    readonly property int railWidth: 56
    readonly property int frameMargin: 10
    readonly property int frameRadius: 16
    readonly property int frameBorder: 2
    readonly property int panelWidth: 320
    readonly property int systemWidth: 520
    readonly property int workWidth: 360
    readonly property int gap: 10
    readonly property int animMs: 180
}
