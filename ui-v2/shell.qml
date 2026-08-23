// shell.qml — Quickshell config-root entry for shell-v2.
//
// Quickshell treats the entry file's directory (ui-v2/) as the "config folder" and forbids QML imports
// that escape it, so the entry stays at the root and every module lives under it. The real composition
// root is shell/Shell.qml.
import "shell"

Shell {}
