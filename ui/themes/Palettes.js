.pragma library

// Palette contract + resolution helpers for the Shrek theme system. Pure data/logic (no QML types) so it
// is shared by Theme.qml. The SEMANTIC KEYS below ARE the stable colour contract every mode must satisfy;
// Tokens.qml exposes exactly these role names to the shell. Curated palettes live as JSON in palettes/*.json
// (the single source of truth, also readable outside QML); BOOTSTRAP mirrors shrek-dark.json purely as the
// guaranteed floor so the resolved palette is NEVER incomplete — even before any file has loaded or if every
// config/palette file is missing (the case the headless dev harness exercises).

var SEMANTIC_KEYS = [
    "bg", "surface", "surfaceAlt", "overlay", "border", "borderStrong",
    "barBg", "panelBg", "rowHi",
    "text", "textDim", "textFaint",
    "accent", "accentDim", "accentText",
    "notice", "danger", "ok",
    "dangerHover", "scrim"
];

// Compiled-in safety floor — MUST stay in sync with palettes/shrek-dark.json (kept intentionally redundant
// so the contract holds with zero I/O dependency).
var BOOTSTRAP = {
    "bg":           "#101014",
    "surface":      "#17181c",
    "surfaceAlt":   "#1f2127",
    "overlay":      "#22242b",
    "border":       "#2f333b",
    "borderStrong": "#3d424c",
    "barBg":        "#ec1a1b20",
    "panelBg":      "#f21d1f26",
    "rowHi":        "#26311c",
    "text":         "#e8e8e6",
    "textDim":      "#9aa0a8",
    "textFaint":    "#6b7079",
    "accent":       "#5aa02c",
    "accentDim":    "#47801f",
    "accentText":   "#0c1206",
    "notice":       "#d8a657",
    "danger":       "#e06c75",
    "ok":           "#5aa02c",
    "dangerHover":  "#3a1e22",
    "scrim":        "#000000"
};

function isColour(v) { return typeof v === "string" && v.length > 0 && v.charAt(0) === "#"; }

// Shallow overlay of several partial palettes; later arguments win. Non-colour values are ignored so a
// stray metadata field (name/appearance) or a malformed override can never poison a role.
function merge() {
    var out = {};
    for (var a = 0; a < arguments.length; a++) {
        var o = arguments[a];
        if (!o) continue;
        for (var k in o) { if (isColour(o[k])) out[k] = o[k]; }
    }
    return out;
}

// Resolve a (possibly partial) palette to the FULL contract: take each semantic key from `partial`, else
// from `base`, else from BOOTSTRAP. Guarantees all SEMANTIC_KEYS are present and colour-valid.
function complete(partial, base) {
    var out = {};
    base = base || BOOTSTRAP;
    for (var i = 0; i < SEMANTIC_KEYS.length; i++) {
        var k = SEMANTIC_KEYS[i];
        if (partial && isColour(partial[k])) out[k] = partial[k];
        else if (isColour(base[k])) out[k] = base[k];
        else out[k] = BOOTSTRAP[k];
    }
    return out;
}
