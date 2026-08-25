#!/usr/bin/env node
// Standalone validation of the ported menu-engine model — no QML runtime needed.
//
// MenuModel.js is pure ES5 loaded by Quickshell's JS engine at runtime; it also keeps a
// `module.exports` block, so the exact same file runs under plain `node` for testing. This
// exercises jsonc parse, default/user merge, alias/route resolve, fuzzy search, and — the part
// that actually changed in the Omarchy->Shrek port — the sealed-Debian guard batch: it must be
// valid bash, run with rc=0 on the build host, use dpkg (never pacman), and honor the
// `<id>:<w|c|d>:<0|1>` contract the surface parses.
//
// Run:  node tests/menu-model.test.js     (from repo root)

const path = require('path');
const fs = require('fs');
const cp = require('child_process');

const REPO = path.resolve(__dirname, '..');
const MODEL = path.join(REPO, 'layers/shrek-desktop/overlay/usr/share/shrek/dms/shrek-menu/MenuModel.js');
const M = require(MODEL);

let fail = 0;
function ok(cond, msg) { console.log((cond ? 'PASS' : 'FAIL') + ' — ' + msg); if (!cond) fail++; }

// Representative tree: the shrek starter shape + one guard per helper family so the batch
// exercises shrek-cmd-present (when), a live command (checked), and shrek-pkg-present (disabled).
const jsonc = `{
  // shrek starter menu
  "apps":    {"icon":"apps","label":"Apps","provider":"apps"},
  "system":  {"icon":"power_settings_new","label":"System","aliases":["power"]},
  "network": {"icon":"wifi","label":"Network"},
  "audio":   {"icon":"volume_up","label":"Audio"},

  "system.lock":    {"icon":"lock","label":"Lock","action":"loginctl lock-session"},
  "system.suspend": {"icon":"bedtime","label":"Suspend","action":"systemctl suspend"},

  "network.toggle-wifi": {"icon":"wifi_off","label":"Toggle Wi-Fi",
    "checked":"nmcli radio wifi | grep -q enabled",
    "action":"nmcli radio wifi off"},

  "audio.mute": {"icon":"volume_off","label":"Mute","when":"shrek-cmd-present wpctl",
    "checked":"wpctl get-volume @DEFAULT_AUDIO_SINK@ | grep -q MUTED",
    "action":"wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle"},

  "tools.editor": {"icon":"edit","label":"Editor","disabled":"shrek-pkg-present bash"},
}`;

// 1. Parse + merge (dotted ids derive parents; a root node is synthesized).
const items = M.parseMenuJsonc(jsonc);
ok(Array.isArray(items) && items.length === 9, `parseMenuJsonc -> ${items.length} items (expect 9)`);
const merged = M.mergeMenuSources(items, []);
ok(!!merged.items.root, 'merge synthesizes a root node');
ok(merged.items['system.lock'].parent === 'system', 'dotted id derives parent (system.lock -> system)');
ok(merged.items['tools.editor'].parent === 'tools', 'parent derived even when the "tools" menu is implicit');

// 2. Route + alias resolve.
ok(M.resolveRoute(merged.items, merged.itemOrder, 'power') === 'system', 'alias "power" -> system');
ok(M.resolveRoute(merged.items, merged.itemOrder, 'system.lock') === 'system.lock', 'exact id resolves');
ok(M.resolveRoute(merged.items, merged.itemOrder, '') === 'root', 'empty -> root');

// 3. Fuzzy search.
const lock = merged.items['system.lock'];
ok(M.matchesQuery(lock, 'lock', true) === true, 'matchesQuery matches on label');
ok(M.matchesQuery(lock, 'zzz', true) === false, 'matchesQuery rejects non-match');
ok(typeof M.searchScore(merged.items, lock, 'lock') === 'number', 'searchScore returns a number');

// 4. Guard batch — the Omarchy->Shrek port. Non-empty, dpkg-based, no pacman, valid + runs.
const script = M.guardScript(merged.items);
ok(script.length > 0, 'guardScript emits a batch');
ok(script.indexOf('dpkg-query') >= 0, 'batch uses dpkg-query');
ok(script.indexOf('pacman') < 0, 'batch has NO pacman references');
ok(script.indexOf('shrek-cmd-present') >= 0, 'shrek-cmd-present helper defined');
ok(script.indexOf('shrek-pkg-present') >= 0, 'shrek-pkg-present helper defined');
ok(script.indexOf('shrek-unit-active') >= 0, 'shrek-unit-active helper defined');

const tmp = path.join(require('os').tmpdir(), 'shrek-guard-batch.sh');
fs.writeFileSync(tmp, script);

const syn = cp.spawnSync('bash', ['-n', tmp]);
ok(syn.status === 0, 'guard batch passes `bash -n`' + (syn.status ? '\n' + syn.stderr : ''));

const run = cp.spawnSync('bash', [tmp], { encoding: 'utf8' });
ok(run.status === 0, 'guard batch executes with rc=0');
const lines = (run.stdout || '').trim().split('\n').filter(Boolean);
const good = lines.length > 0 && lines.every(l => /^[^:]+:[wcd]:[01]$/.test(l));
ok(good, `every line matches <id>:<w|c|d>:<0|1> (${lines.length} lines)`);

// bash is installed on any build host -> the dpkg-backed presence check must report 1.
const bashLine = lines.find(l => l.startsWith('tools.editor:d:'));
ok(bashLine === 'tools.editor:d:1', `shrek-pkg-present bash -> installed (got ${bashLine})`);
// wpctl may or may not exist on the build host, but the line must be well-formed either way.
const wpLine = lines.find(l => l.startsWith('audio.mute:w:'));
ok(/^audio\.mute:w:[01]$/.test(wpLine || ''), `shrek-cmd-present wpctl well-formed (got ${wpLine})`);

try { fs.unlinkSync(tmp); } catch (e) {}
console.log('\n' + (fail === 0 ? 'ALL PASS' : fail + ' FAILURE(S)'));
process.exit(fail === 0 ? 0 : 1);
