// shell.qml — Quickshell config-root entry.
//
// Kept at the ui/ ROOT on purpose: Quickshell uses the entry file's directory as the "config folder"
// and FORBIDS QML imports that escape it. Loading shell/Shell.qml directly would make ui/shell/ the
// config folder, so its `import "../providers"` / `import "../themes"` (siblings under ui/) would be
// rejected ("Module path ... is outside of the config folder"). With the entry here, the config folder
// is ui/, and every module (shell/, providers/, themes/) lives inside it. The real shell root is
// shell/Shell.qml.
import "shell"

Shell {}
