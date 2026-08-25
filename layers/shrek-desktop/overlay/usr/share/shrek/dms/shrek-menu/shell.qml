// shrek-menu — shrek-owned standalone command surface (docs/menu-engine.md; mycelium menu-engine GO decision).
//
// A SEPARATE `qs` process, NOT a DMS plugin and NOT spliced into DMS's tree: DMS's plugin schema has no
// surface type and plugins can't make a top-level PanelWindow, and quickshell IPC target names are scoped
// to one instance (`dms ipc` is hard-pinned to DMS's own PID/config — verified from quickshell
// command.cpp::selectInstance + DankMaterialShell shell.go). So:
//
//   launch:  exec_always qs -p /usr/share/shrek/dms/shrek-menu/shell.qml   (from sway.config)
//   toggle:  qs -p /usr/share/shrek/dms/shrek-menu/shell.qml ipc call shrek-menu toggle   ($mod+slash)
//
// `dms ipc call shrek-menu` will NOT reach this process — the toggle MUST go through `qs -p <this file>`.
//
// The engine model (MenuModel.js, ported ~verbatim from Omarchy) does all the pure work: parse+merge the
// menu.jsonc tree, generate the one-shot guard batch, flatten/score search, resolve routes. This surface
// owns the runtime: load the files, run the guard batch, render a themed keyboard-driven list, launch
// actions and apps. Theme parity comes from reading DMS's dms-colors.json (a second qs process is
// blackholed from importing DMS's Theme singleton), so the menu follows wallpaper/theme changes live.
import QtQuick
import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import "MenuModel.js" as MenuModel

ShellRoot {
    id: root

    Component.onCompleted: console.info("SHREK-MENU surface loaded (hidden until toggle)")

    // ── Live theme parity from DMS matugen output ────────────────────────────────────────────────
    // DMS rewrites ~/.cache/DankMaterialShell/dms-colors.json on every wallpaper/theme change. We can't
    // `import qs.Common` DMS's Theme singleton — a second qs process is blackholed by quickshell's
    // per-instance qs:// URL interceptor — so this surface reads the same file itself. The write is atomic
    // (matugen stages a .tmp then renames), so a watched read never sees a partial file. Schema verified
    // live against dms=1.5.3db1 (mycelium #2819): { colors: { dark|light: { <M3 key>: "#rrggbb" } }, dank16 }.
    readonly property string colorMode: "dark"   // shipped image is dark; DMS defaults to dark too.

    property var dmsColors: ({})
    function reloadColors() {
        try { root.dmsColors = JSON.parse(colorsFile.text() || "{}") || ({}); }
        catch (e) { root.dmsColors = ({}); }
    }
    function themed(key, fallback) {
        var mode = root.dmsColors && root.dmsColors.colors ? root.dmsColors.colors[root.colorMode] : null;
        var v = mode ? mode[key] : undefined;
        return (typeof v === "string" && v.length > 0) ? v : fallback;
    }

    // onLoaded (NOT onLoadedChanged): `loaded` transitions false->true only on the first load, so
    // onLoadedChanged would never re-fire on subsequent reloads and the surface would freeze at the launch
    // palette. onLoaded fires on every completed (re)load — the surface recolors each time matugen rewrites.
    FileView {
        id: colorsFile
        path: (Quickshell.env("HOME") || "") + "/.cache/DankMaterialShell/dms-colors.json"
        blockLoading: false
        watchChanges: true
        printErrors: false
        onFileChanged: reload()
        onLoaded: root.reloadColors()
    }

    // Surface tokens: live DMS value first, baked swamp-green (DMS default palette) as the fallback.
    readonly property color cSurface:     themed("surface_container_high", "#1b2a1a")  // elevated card
    readonly property color cSurfaceText: themed("on_surface",             "#e6efe0")
    readonly property color cSurfaceDim:  themed("on_surface_variant",     "#9db097")  // secondary text
    readonly property color cPrimary:     themed("primary",                "#7cae5a")
    readonly property color cOnPrimary:   themed("on_primary",             "#0c1a06")
    readonly property color cOutline:     themed("outline_variant",        "#3a4a34")
    readonly property color cSelected:    themed("primary_container",      "#2c3f22")  // highlighted row

    // Material Symbols Rounded — the icon font DMS bakes at this path. System fontconfig doesn't register
    // it, so (exactly like DMS's own DankIcon.qml) we FontLoader the .ttf directly and use its .name as the
    // family; menu.jsonc `icon` values are Material Symbol ligature names ("wifi", "lock", ...). The
    // bracketed filename is a valid file URL — DMS resolves the same path this way.
    FontLoader {
        id: symbolsFont
        source: Qt.resolvedUrl("file:///usr/share/quickshell/dms/assets/fonts/material-design-icons/variablefont/MaterialSymbolsRounded[FILL,GRAD,opsz,wght].ttf")
    }

    // ── Menu model state ─────────────────────────────────────────────────────────────────────────
    // The tree is default (baked, RO /usr) merged with an optional user file from writable /home; the
    // user file can override any key but a provider is only ever a fixed baked string (enforced below),
    // so /home can re-point but never inject a command. FileView watchChanges gives free live-reload.
    readonly property string defaultMenuPath: "/usr/share/shrek/dms/shrek-menu/menu.jsonc"
    readonly property string userMenuPath: (Quickshell.env("HOME") || "") + "/.config/shrek/menu.jsonc"

    property var defaultMenuItems: []
    property var userMenuItems: []
    property var items: ({})
    property var itemOrder: []

    // Installed apps (the `apps` provider). Quickshell core DesktopEntries is a per-instance C++ singleton
    // (NOT DMS's app service — a second qs process is blackholed from importing that), so it is reachable
    // directly with no cross-instance coupling. appList is a bound property: when the scan updates,
    // onAppListChanged re-merges — no need to know the ObjectModel's exact change-signal name. appEntries
    // maps a row's appId back to its DesktopEntry so activate() can launch it with entry.execute().
    readonly property var appList: DesktopEntries.applications ? DesktopEntries.applications.values : []
    property var appEntries: ({})
    onAppListChanged: root.rebuildFromSources()

    // Guard results: id -> bool. Absent id = "no guard" (visible / unchecked / enabled).
    property var whenResults: ({})
    property var checkedResults: ({})
    property var disabledResults: ({})

    // Navigation.
    property string currentMenuId: "root"
    property string filterText: ""
    property int selectedIndex: 0

    function rebuildFromSources() {
        var merged = MenuModel.mergeMenuSources(root.defaultMenuItems, root.userMenuItems);
        // Merge installed-app rows under the "apps" section (sorted by name) and record a launch map.
        var rows = [];
        var map = ({});
        var apps = (root.appList || []).slice();
        apps.sort(function (a, b) { return ("" + (a && a.name || "")).localeCompare("" + (b && b.name || "")); });
        for (var i = 0; i < apps.length; i++) {
            var e = apps[i];
            if (!e || e.noDisplay) continue;
            var appId = "" + (e.id || e.name || "");
            if (!appId) continue;
            map[appId] = e;
            rows.push(root.makeAppRow(appId, e));
        }
        var withApps = MenuModel.mergeAppRows(merged.items, merged.itemOrder, rows);
        root.items = withApps.items;
        root.itemOrder = withApps.itemOrder;
        root.appEntries = map;
        root.evaluateGuards();
    }

    // A display-ready app row parented under the "apps" section. genericName + keywords ride in aliases so
    // fuzzy search finds an app by what it does, not just its name (MenuModel scores kind:"app" specially).
    function makeAppRow(appId, e) {
        var aliases = [];
        if (e.genericName) aliases.push("" + e.genericName);
        var kw = e.keywords || [];
        for (var k = 0; k < kw.length; k++) aliases.push("" + kw[k]);
        return {
            id: "apps." + appId, parent: "apps", kind: "app",
            icon: "", iconFont: "", appIcon: "" + (e.icon || ""), appId: appId,
            label: "" + (e.name || appId), title: "", target: "", description: "" + (e.genericName || ""),
            action: "", provider: "", aliases: aliases, when: "", checked: "", disabled: ""
        };
    }

    // One bash subprocess per (re)load evaluates every when/checked/disabled. The menu renders on the last
    // batch's cached answers and never blocks; a fresh batch runs on open and on any file change.
    function evaluateGuards() {
        var script = MenuModel.guardScript(root.items);
        if (!script || !script.trim()) {
            root.whenResults = ({}); root.checkedResults = ({}); root.disabledResults = ({});
            root.refreshRows();
            return;
        }
        guardProc.command = ["bash", "-c", script];
        guardProc.running = true;
    }

    Process {
        id: guardProc
        stdout: StdioCollector {
            id: guardOut
            onStreamFinished: {
                var nextWhen = ({}), nextChecked = ({}), nextDisabled = ({});
                var lines = ("" + guardOut.text).split("\n");
                for (var i = 0; i < lines.length; i++) {
                    var line = lines[i];
                    if (!line) continue;
                    // "<id>:<tag>:<0|1>" — ids carry dots but never colons; parse from the right.
                    var lastColon = line.lastIndexOf(":");
                    if (lastColon < 0) continue;
                    var val = line.slice(lastColon + 1) === "1";
                    var rest = line.slice(0, lastColon);
                    var tagColon = rest.lastIndexOf(":");
                    if (tagColon < 0) continue;
                    var tag = rest.slice(tagColon + 1);
                    var id = rest.slice(0, tagColon);
                    if (tag === "w") nextWhen[id] = val;
                    else if (tag === "c") nextChecked[id] = val;
                    else if (tag === "d") nextDisabled[id] = val;
                }
                root.whenResults = nextWhen;
                root.checkedResults = nextChecked;
                root.disabledResults = nextDisabled;
                root.refreshRows();
            }
        }
    }

    // ── Display rows ─────────────────────────────────────────────────────────────────────────────
    // No filter: the visible children of the current menu, in declared order. With a filter: every leaf
    // in the whole tree that matches, ranked by MenuModel.searchScore (lower = better). displayRow() shapes
    // each into { itemId,kind,label,icon,target,childCount,disabled,action,provider,path,... } for the UI.
    property var rows: []
    function refreshRows() {
        var out = [];
        var order = root.itemOrder;
        var q = root.filterText.trim();
        if (q.length === 0) {
            for (var i = 0; i < order.length; i++) {
                var e = root.items[order[i]];
                if (!e || e.parent !== root.currentMenuId) continue;
                if (!MenuModel.isVisible(root.items, order, root.whenResults, e, 0)) continue;
                out.push(MenuModel.displayRow(root.items, order, root.checkedResults, root.disabledResults, e, "", 0, ""));
            }
        } else {
            var scored = [];
            for (var j = 0; j < order.length; j++) {
                var entry = root.items[order[j]];
                if (!entry) continue;
                var vis = MenuModel.isVisible(root.items, order, root.whenResults, entry, 0);
                if (!MenuModel.matchesQuery(entry, q, vis)) continue;
                scored.push({ entry: entry, score: MenuModel.searchScore(root.items, entry, q) });
            }
            scored.sort(function (a, b) { return a.score - b.score; });
            for (var k = 0; k < scored.length; k++) {
                var se = scored[k].entry;
                out.push(MenuModel.displayRow(root.items, order, root.checkedResults, root.disabledResults,
                                              se, "", scored[k].score, ""));
            }
        }
        root.rows = out;
        if (root.selectedIndex >= out.length) root.selectedIndex = Math.max(0, out.length - 1);
    }

    // ── Navigation actions ───────────────────────────────────────────────────────────────────────
    function openRoot() {
        root.currentMenuId = "root";
        root.filterText = "";
        root.selectedIndex = 0;
        root.evaluateGuards();   // fresh state every open; refreshRows() runs when the batch returns
    }
    function enterMenu(id) { root.currentMenuId = id; root.filterText = ""; root.selectedIndex = 0; root.refreshRows(); }
    function goBack() {
        if (root.filterText.length > 0) { root.filterText = ""; root.selectedIndex = 0; root.refreshRows(); return; }
        if (root.currentMenuId === "root") return;
        var cur = root.items[root.currentMenuId];
        root.currentMenuId = (cur && cur.parent) ? cur.parent : "root";
        root.selectedIndex = 0;
        root.refreshRows();
    }
    function activate(row) {
        if (!row || row.disabled) return;
        if (row.kind === "app") {
            var e = root.appEntries[row.appId];
            if (e) e.execute();          // DesktopEntry.execute() handles field codes / terminal / workdir
            win.visible = false;
            return;
        }
        if (row.action && row.action.length > 0) {
            Quickshell.execDetached(["bash", "-c", row.action]);
            win.visible = false;
            return;
        }
        // menu or link: descend. link resolves through its target/alias; a menu descends into its own id.
        var target = (row.kind === "link") ? MenuModel.resolveRoute(root.items, root.itemOrder, row.target) : row.itemId;
        root.enterMenu(target);
    }

    function moveSelection(delta) {
        var n = root.rows.length;
        if (n === 0) return;
        root.selectedIndex = Math.max(0, Math.min(n - 1, root.selectedIndex + delta));
    }

    // Menu-file loaders. onLoaded parses to the *Items array; a failed user file is simply empty.
    FileView {
        id: defaultMenuFile
        path: root.defaultMenuPath
        blockLoading: false
        watchChanges: true
        printErrors: false
        onFileChanged: reload()
        onLoaded: { root.defaultMenuItems = MenuModel.parseMenuJsonc(defaultMenuFile.text() || ""); root.rebuildFromSources(); }
    }
    FileView {
        id: userMenuFile
        path: root.userMenuPath
        blockLoading: false
        watchChanges: true
        printErrors: false
        onFileChanged: reload()
        onLoaded: { root.userMenuItems = MenuModel.parseMenuJsonc(userMenuFile.text() || ""); root.rebuildFromSources(); }
        onLoadFailed: { root.userMenuItems = []; root.rebuildFromSources(); }
    }

    // Breadcrumb for the header: the path to the current menu, or "Search" while filtering.
    function currentPath() {
        if (root.filterText.length > 0) return "Search";
        if (root.currentMenuId === "root") return "Shrek";
        return MenuModel.pathFor(root.items, root.currentMenuId) || "Shrek";
    }

    // The only inter-process seam. Named "shrek-menu"; addressed via `qs -p <this file> ipc call shrek-menu ...`.
    IpcHandler {
        target: "shrek-menu"
        function toggle(): void { if (win.visible) win.visible = false; else root.showMenu(); }
        function show():   void { root.showMenu(); }
        function hide():   void { win.visible = false }
    }
    function showMenu() { root.openRoot(); win.visible = true; }

    PanelWindow {
        id: win
        visible: false                 // hidden until the IPC toggle — the desktop shows nothing at boot
        color: "transparent"           // no opaque flash before the card paints

        // No edge anchors -> wlr-layer-shell centers the surface; sized to its content (capped).
        readonly property int rowHeight: 34
        readonly property int headerHeight: 44
        readonly property int maxRows: 12
        implicitWidth: 520
        implicitHeight: headerHeight + Math.max(1, Math.min(win.maxRows, root.rows.length)) * rowHeight + 24

        WlrLayershell.layer: WlrLayer.Overlay                    // above fullscreen windows
        WlrLayershell.keyboardFocus: WlrKeyboardFocus.Exclusive  // modal keyboard grab
        WlrLayershell.namespace: "shrek-menu"                    // surface id for sway/wlroots

        onVisibleChanged: if (visible) card.forceActiveFocus()

        Rectangle {
            id: card
            anchors.fill: parent
            focus: true
            color: root.cSurface
            radius: 16
            border.width: 1
            border.color: root.cOutline

            // Type-to-search + navigation. A focused Item (not a TextInput) so arrow keys are ours, not
            // eaten by a text cursor; printable keys append to filterText, Backspace edits or steps back.
            Keys.onPressed: function (event) {
                if (event.key === Qt.Key_Escape) {
                    if (root.filterText.length > 0) { root.filterText = ""; root.selectedIndex = 0; root.refreshRows(); }
                    else win.visible = false;
                    event.accepted = true;
                } else if (event.key === Qt.Key_Down) {
                    root.moveSelection(1); event.accepted = true;
                } else if (event.key === Qt.Key_Up) {
                    root.moveSelection(-1); event.accepted = true;
                } else if (event.key === Qt.Key_PageDown) {
                    root.moveSelection(win.maxRows); event.accepted = true;
                } else if (event.key === Qt.Key_PageUp) {
                    root.moveSelection(-win.maxRows); event.accepted = true;
                } else if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                    if (root.rows.length > 0) root.activate(root.rows[root.selectedIndex]);
                    event.accepted = true;
                } else if (event.key === Qt.Key_Right) {
                    var r = root.rows[root.selectedIndex];
                    if (r && !r.action && r.childCount > 0) root.activate(r);
                    event.accepted = true;
                } else if (event.key === Qt.Key_Left) {
                    root.goBack(); event.accepted = true;
                } else if (event.key === Qt.Key_Backspace) {
                    if (root.filterText.length > 0) { root.filterText = root.filterText.slice(0, -1); root.selectedIndex = 0; root.refreshRows(); }
                    else root.goBack();
                    event.accepted = true;
                } else if (event.text && event.text.length === 1 && event.text.charCodeAt(0) >= 0x20) {
                    root.filterText += event.text; root.selectedIndex = 0; root.refreshRows();
                    event.accepted = true;
                }
            }

            Column {
                anchors.fill: parent
                anchors.margins: 12
                spacing: 6

                // Header: breadcrumb + live filter text.
                Item {
                    width: parent.width
                    height: 32
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        text: root.currentPath()
                        color: root.cPrimary
                        font.pixelSize: 15
                        font.bold: true
                    }
                    Text {
                        anchors.verticalCenter: parent.verticalCenter
                        anchors.right: parent.right
                        text: root.filterText.length > 0 ? ("› " + root.filterText) : "type to search"
                        color: root.filterText.length > 0 ? root.cSurfaceText : root.cSurfaceDim
                        font.pixelSize: 13
                    }
                }

                Rectangle { width: parent.width; height: 1; color: root.cOutline }

                ListView {
                    id: resultList
                    width: parent.width
                    height: parent.height - 32 - 1 - parent.spacing * 2
                    clip: true
                    model: root.rows
                    currentIndex: root.selectedIndex
                    boundsBehavior: Flickable.StopAtBounds
                    // Keep the highlighted row on screen as the cursor moves through a long list (apps).
                    onCurrentIndexChanged: positionViewAtIndex(currentIndex, ListView.Contain)

                    delegate: Rectangle {
                        width: resultList.width
                        height: win.rowHeight
                        radius: 8
                        color: index === root.selectedIndex ? root.cSelected : "transparent"
                        opacity: modelData.disabled ? 0.4 : 1.0

                        MouseArea {
                            anchors.fill: parent
                            hoverEnabled: true
                            onEntered: root.selectedIndex = index
                            onClicked: root.activate(modelData)
                        }

                        Row {
                            anchors.fill: parent
                            anchors.leftMargin: 12
                            anchors.rightMargin: 12
                            spacing: 8

                            // Leading icon: a Material Symbol glyph for command-tree rows, the real app icon
                            // (from the baked Papirus theme via Quickshell.iconPath) for app rows.
                            Item {
                                id: iconSlot
                                width: 22
                                height: parent.height
                                Text {
                                    anchors.centerIn: parent
                                    visible: modelData.kind !== "app" && modelData.icon && modelData.icon.length > 0
                                    text: modelData.icon
                                    font.family: symbolsFont.name
                                    font.pixelSize: 19
                                    color: index === root.selectedIndex ? root.cPrimary : root.cSurfaceDim
                                }
                                Image {
                                    anchors.centerIn: parent
                                    visible: modelData.kind === "app"
                                    width: 18; height: 18
                                    sourceSize.width: 18; sourceSize.height: 18
                                    fillMode: Image.PreserveAspectFit
                                    asynchronous: true
                                    source: modelData.kind === "app"
                                            ? Quickshell.iconPath(modelData.appIcon, "application-x-executable") : ""
                                }
                            }

                            Text {
                                anchors.verticalCenter: parent.verticalCenter
                                width: parent.width - iconSlot.width - childrenExpander.width - parent.spacing * 2
                                text: modelData.label
                                elide: Text.ElideRight
                                color: root.cSurfaceText
                                font.pixelSize: 14
                            }
                            Text {
                                id: childrenExpander
                                anchors.verticalCenter: parent.verticalCenter
                                // A submenu (children, no action) shows a chevron; a search match shows its path.
                                text: (!modelData.action && modelData.childCount > 0) ? "›"
                                      : (root.filterText.length > 0 && modelData.path ? modelData.path : "")
                                color: root.cSurfaceDim
                                font.pixelSize: (!modelData.action && modelData.childCount > 0) ? 20 : 11
                            }
                        }
                    }

                    // Empty state.
                    Text {
                        anchors.centerIn: parent
                        visible: root.rows.length === 0
                        text: root.filterText.length > 0 ? "no matches" : "empty"
                        color: root.cSurfaceDim
                        font.pixelSize: 13
                    }
                }
            }
        }
    }
}
