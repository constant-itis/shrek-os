pragma Singleton
import QtQuick
import "../theme"

// Appearance — ordinary user theme preference seam. Surfaces use Tokens for color and this service for
// the selected mode/mutation path; they do not import Theme directly.
QtObject {
    readonly property string mode: Theme.mode
    readonly property var modes: [
        { mode: "shrek-dark", label: "Shrek Dark", detail: "Default dark desktop" },
        { mode: "shrek-light", label: "Shrek Light", detail: "Bright desktop mode" },
        { mode: "high-contrast", label: "High Contrast", detail: "Stronger boundaries and text" },
        { mode: "dynamic", label: "Dynamic", detail: "Uses provider data when present" },
        { mode: "custom", label: "Custom", detail: "Uses ~/.config/shrek/custom.json" }
    ]
    function setMode(m): void { Theme.setMode(m) }
}
