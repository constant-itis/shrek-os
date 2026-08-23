# DankMaterialShell Donor Notice

Shrek shell-v2 uses DankMaterialShell as an approved donor for generic desktop UI
presentation patterns and QML component behavior.

Pinned donor:

- Repository: https://github.com/AvengeMedia/DankMaterialShell
- Commit: `eadb8cf9c710b8fa5138a71a028c7f9968ef7111`
- License: MIT

Imported/adapted scope:

- ControlCenter presentation patterns:
  - `quickshell/Modules/ControlCenter/Widgets/ToggleButton.qml`
  - `quickshell/Modules/ControlCenter/Widgets/SmallToggleButton.qml`
  - `quickshell/Modules/ControlCenter/Widgets/AudioSliderRow.qml`
  - `quickshell/Modules/ControlCenter/Components/HeaderPane.qml`
  - `quickshell/Modules/ControlCenter/utils/layout.js`
- Settings/control density and interaction behavior:
  - 60 px control tiles
  - 48 px compact icon tiles
  - 36 px action buttons
  - 40 px slider rows
  - short hover/press transitions
  - active tile fill + ring treatment

Compatibility/audit notes:

- Shrek does not import DMS backend services, settings model, plugin model, theme,
  ripple implementation, icon component, popout service, blur service, or edit-mode
  architecture.
- Adapted QML is rewritten against Quickshell v0.3.1-compatible QtQuick primitives
  already used by shell-v2.
- All color roles map through `ui-v2/theme/Tokens.qml`.
- All desktop state mutation remains bound to Shrek-owned services under
  `ui-v2/services`.
- Work/authority state remains read-only and is not connected to these ordinary
  desktop controls.

