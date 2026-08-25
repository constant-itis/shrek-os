#!/usr/bin/env node
// Validates the SHIPPED menu.jsonc content (as opposed to menu-model.test.js, which validates the
// engine). Loads the real baked file through MenuModel and asserts it parses, nests without orphans,
// keeps providers to fixed baked string keys (the security invariant — never a path/command), and
// that its when/checked/disabled guards compile to valid bash that runs with the right contract.
//
// Run:  node tests/menu-jsonc.test.js     (from repo root)

const path = require('path');
const fs = require('fs');
const os = require('os');
const cp = require('child_process');

const REPO = path.resolve(__dirname, '..');
const DIR = path.join(REPO, 'layers/shrek-desktop/overlay/usr/share/shrek/dms/shrek-menu');
const M = require(path.join(DIR, 'MenuModel.js'));
const raw = fs.readFileSync(path.join(DIR, 'menu.jsonc'), 'utf8');

let fail = 0;
function ok(cond, msg) { console.log((cond ? 'PASS' : 'FAIL') + ' — ' + msg); if (!cond) fail++; }

const items = M.parseMenuJsonc(raw);
ok(items.length > 0, `menu.jsonc parses (${items.length} items; comments/commas stripped)`);
const merged = M.mergeMenuSources(items, []);
const ids = merged.itemOrder;
ok(!!merged.items.root, 'root node synthesized');

// No dotted child points at a parent that does not exist.
const orphans = ids.filter(id => {
  const it = merged.items[id];
  return it.parent && it.parent !== 'root' && !merged.items[it.parent];
});
ok(orphans.length === 0, 'no orphan parents' + (orphans.length ? ': ' + orphans.join(', ') : ''));

// The intended top-level sections are all present.
['apps', 'system', 'network', 'style', 'capture', 'audio'].forEach(t =>
  ok(!!merged.items[t], `section "${t}" present`));

// SECURITY INVARIANT: every provider is a bare lowercase string key, never a path or command.
const provs = [...new Set(ids.map(i => merged.items[i].provider).filter(Boolean))];
ok(provs.every(p => /^[a-z][a-z0-9]*$/.test(p)), 'providers are fixed baked keys: ' + JSON.stringify(provs));

// Aliases route to their sections.
ok(M.resolveRoute(merged.items, ids, 'power') === 'system', 'alias "power" -> system');
ok(M.resolveRoute(merged.items, ids, 'screenshot') === 'capture', 'alias "screenshot" -> capture');

// Guard batch: dpkg-based, valid bash, runs, honors <id>:<w|c|d>:<0|1>.
const script = M.guardScript(merged.items);
ok(script.indexOf('pacman') < 0 && script.indexOf('dpkg-query') >= 0, 'guard batch is dpkg-based (no pacman)');
const tmp = path.join(os.tmpdir(), 'shrek-menu-jsonc-guard.sh');
fs.writeFileSync(tmp, script);
ok(cp.spawnSync('bash', ['-n', tmp]).status === 0, 'guard batch passes `bash -n`');
const run = cp.spawnSync('bash', [tmp], { encoding: 'utf8' });
ok(run.status === 0, 'guard batch runs rc=0');
const lines = (run.stdout || '').trim().split('\n').filter(Boolean);
ok(lines.length > 0 && lines.every(l => /^[^:]+:[wcd]:[01]$/.test(l)), `guard contract ok (${lines.length} lines)`);
try { fs.unlinkSync(tmp); } catch (e) {}

console.log('\n' + (fail === 0 ? 'ALL PASS' : fail + ' FAILURE(S)'));
process.exit(fail === 0 ? 0 : 1);
