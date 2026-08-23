pragma Singleton
import QtQuick
import Quickshell
import Quickshell.Io
import "Palettes.js" as Palettes

// The theme CONTROLLER — the one place that selects a mode, resolves it to the full semantic contract, and
// exposes the result as `c` (consumed only by Tokens.qml). Design goals:
//   * ONE stable contract: every mode resolves to the same SEMANTIC_KEYS (see Palettes.js).
//   * Dynamic is the DEFAULT/recommended experience, not the only one (mode defaults to "dynamic").
//   * User override without bypass: `overrides` is a semantic-keyed partial that merges over ANY mode's base
//     — so a component can never reach around the token layer; it can only be handed a different value.
//   * Never incomplete: gaps fall back through base -> shrek-dark floor, so the shell always has a full palette.
//
// Config: ~/.config/shrek/theme.json = { "mode": "...", "overrides": { "<key>": "#hex", ... } }
//   mode ∈ dynamic | shrek-dark | shrek-light | high-contrast | custom
//   custom pulls its base from ~/.config/shrek/custom.json (a full or partial palette of the same shape).
// SHREK_THEME_CONFIG env overrides the config path (used by the preview harness to render each mode).
QtObject {
    id: root

    readonly property string _home: Quickshell.env("HOME") || ""
    readonly property string configPath: Quickshell.env("SHREK_THEME_CONFIG") || (_home + "/.config/shrek/theme.json")
    // Palette files live next to this singleton (palettes/*.json), so this resolves correctly whether the
    // shell runs from the repo (dev harness) or /usr/share/shrek/ui (sealed image).
    readonly property string themesDir: ("" + Qt.resolvedUrl("palettes/")).replace(/^file:\/\//, "")

    // ── config ────────────────────────────────────────────────────────────────────────────────────────
    property var _cfg: ({})
    readonly property string mode: (_cfg && typeof _cfg.mode === "string" && _cfg.mode.length) ? _cfg.mode : "dynamic"
    readonly property var overrides: (_cfg && _cfg.overrides) ? _cfg.overrides : ({})

    function _basePathFor(m) {
        if (m === "dynamic") return "";                       // dynamic reads the live Colours source, not a file
        if (m === "custom")  return root._home + "/.config/shrek/custom.json";
        return root.themesDir + m + ".json";                  // curated: shrek-dark / shrek-light / high-contrast
    }
    readonly property string basePath: _basePathFor(mode)
    property var _baseFile: ({})

    // The selected base palette (partial ok): dynamic => live wallpaper scheme; everything else => the file.
    readonly property var base: (mode === "dynamic") ? Colours.scheme : _baseFile

    // ── the resolved semantic palette (THE product) ─────────────────────────────────────────────────────
    // overrides win over base; base fills the rest; the shrek-dark floor fills anything still missing.
    readonly property var c: Palettes.complete(Palettes.merge(root.base, root.overrides), Palettes.BOOTSTRAP)

    // Appearance cycle order for the rail's theme toggle (dynamic is the default/recommended first stop).
    readonly property var _cycle: ["dynamic", "shrek-dark", "shrek-light", "high-contrast"]

    // Write the mode into the user config (preserving overrides) and update live. Persists via FileView so
    // the choice survives a restart; the running shell repaints through the normal watch path.
    function setMode(m) {
        var cfg = {};
        try { cfg = JSON.parse(_cfgFv.text() || "{}") || ({}); } catch (e) { cfg = ({}); }
        cfg.mode = m;
        _cfgFv.setText(JSON.stringify(cfg));
        root._cfg = cfg;
    }
    function cycleAppearance() {
        var i = _cycle.indexOf(root.mode);
        setMode(_cycle[(i + 1) % _cycle.length]);
    }

    function _loadCfg()  { try { root._cfg = JSON.parse(_cfgFv.text() || "{}") || ({}); }  catch (e) { root._cfg = ({}); } }
    function _loadBase() { try { root._baseFile = JSON.parse(_baseFv.text() || "{}") || ({}); } catch (e) { root._baseFile = ({}); } }

    property FileView _cfgFv: FileView {
        path: root.configPath
        watchChanges: true
        printErrors: false
        onFileChanged: reload()
        onLoadedChanged: if (loaded) root._loadCfg()
    }
    property FileView _baseFv: FileView {
        path: root.basePath
        watchChanges: true
        printErrors: false
        onFileChanged: reload()
        onLoadedChanged: if (loaded) root._loadBase()
    }

    // ── sway window chrome ──────────────────────────────────────────────────────────────────────────────
    // Window borders live in sway, outside QML. Rather than an include on the read-only sealed /usr, the
    // shell pushes the active palette to sway at runtime (it inherits SWAYSOCK from the session that spawned
    // it), so borders track every theme switch. sway.config keeps a static shrek-dark default for first paint.
    function _syncSway() {
        var p = root.c;
        if (!p || !p.bg) return;
        // args: border background text indicator child_border
        Quickshell.execDetached(["swaymsg", "client.focused",          p.accentDim, p.surface, p.text,    p.accent,  p.accentDim]);
        Quickshell.execDetached(["swaymsg", "client.unfocused",        p.border,    p.surface, p.textDim, p.border,  p.border]);
        Quickshell.execDetached(["swaymsg", "client.focused_inactive", p.border,    p.surface, p.textDim, p.border,  p.border]);
    }
    onCChanged: _syncSway()
    Component.onCompleted: _syncSway()
}
