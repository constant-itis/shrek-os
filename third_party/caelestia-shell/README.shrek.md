# Caelestia Shell Port Source

This directory contains source copied from `caelestia-dots/shell` for the Shrek Shell
GPL-compatible port.

Upstream:

- Repository: https://github.com/caelestia-dots/shell
- Imported commit: `0a87e5d`
- License: GNU GPL v3, preserved in `LICENSE`

Current import scope:

- `plugin/src/Caelestia/Blobs/**`

The Blobs module is the native shape/rendering machinery responsible for Caelestia's
merged shell geometry: continuous frame, rounded inverted desktop hole, smooth panel
joins, concave/convex transitions, and deformation-aware SDF rendering.

Shrek build wrappers live outside the copied upstream subtree where practical so future
updates can be compared against upstream cleanly.
