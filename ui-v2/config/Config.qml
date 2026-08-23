pragma Singleton
import QtQuick

// Config — geometry + motion tokens (px / ms). The real semantic Tokens/Theme system gets ported in a
// later slice; these keep FIRST ACCEPTANCE self-contained with zero external dependencies.
QtObject {
    readonly property int railWidth: 56
    readonly property int frameMargin: 10
    readonly property int frameRadius: 16
    readonly property int frameBorder: 2
    readonly property int panelWidth: 320
    readonly property int gap: 10
    readonly property int animMs: 180
}
