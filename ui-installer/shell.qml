// shell.qml — Quickshell config-root entry for the Shrek OS installer (ui-installer/).
//
// Quickshell treats the entry file's directory (ui-installer/) as the config folder and forbids QML
// imports that escape it, so the entry stays at the root and the composition lives under ui/. This is a
// SEPARATE config root from the desktop shell (ui-v2/): the installer is its own application, launched in
// the live environment, retired after first boot. STATIC scaffold — no backend wiring (M1 sign-off pass).
import "ui"

Installer {}
