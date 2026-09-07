#!/usr/bin/env bash
# Verify the ADR-009 v2 OQ-4 DMS UI suppressions (S5b), host-side/static:
#   (A) System Updater settings tab removed, (B) Plugins (marketplace) settings tab removed,
#   (C) the "What's New" changelog popup pre-suppressed via its version marker.
# Also enforces the DRIFT guard: both edits are pinned to the dms= package version in mkosi.conf,
# so a package bump that forgets to re-diff these files fails here (loud) instead of silently
# reverting DMS's newer tab structure / re-popping the changelog.
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MKOSI=$ROOT/layers/shrek-desktop/mkosi.conf
TABS=$ROOT/layers/shrek-desktop/overlay/usr/share/quickshell/dms/Common/SettingsTabs.qml
LAUNCHER=$ROOT/layers/shrek-desktop/overlay/usr/bin/shrek-desktop
SWAY=$ROOT/layers/shrek-desktop/overlay/usr/share/shrek/desktop/sway.config

pass=0; fail=0
chk() { if [ "$2" = "$3" ]; then echo "  PASS $1 ($2)"; pass=$((pass+1)); else echo "  FAIL $1 exp=[$3] got=[$2]"; fail=$((fail+1)); fi; }
has() { if grep -qF "$2" "$1"; then echo "  PASS $3"; pass=$((pass+1)); else echo "  FAIL $3 (missing: $2)"; fail=$((fail+1)); fi; }
hasnot() { if grep -qF "$2" "$1"; then echo "  FAIL $3 (present: $2)"; fail=$((fail+1)); else echo "  PASS $3"; pass=$((pass+1)); fi; }

echo "== pin: read dms= from mkosi.conf =="
DMS_PIN=$(sed -n 's/^[[:space:]]*dms=\([0-9A-Za-z.]*\).*/\1/p' "$MKOSI" | head -1)
# major.minor drives DMS's ChangelogService currentVersion (e.g. 1.6.0db1 -> "1.6")
DMS_MM=$(printf '%s\n' "$DMS_PIN" | sed -n 's/^\([0-9]*\.[0-9]*\).*/\1/p')
chk pin-nonempty "$([ -n "$DMS_PIN" ] && echo yes || echo no)" "yes"
chk pin-majorminor "$([ -n "$DMS_MM" ] && echo yes || echo no)" "yes"
echo "  (dms pin=$DMS_PIN  major.minor=$DMS_MM)"

echo "== (A/B) SettingsTabs.qml overlay: updater + plugins tabs removed =="
chk tabs-exists "$([ -f "$TABS" ] && echo yes || echo no)" "yes"
chk tabs-updater-count "$(grep -c '"id": "updater"' "$TABS")" "0"
chk tabs-plugins-count "$(grep -c '"id": "plugins"' "$TABS")" "0"
# ...and the surrounding structure is otherwise intact (didn't nuke the array)
for id in personalization system users multiplexers power_sleep network separator about; do
  chk "tabs-keep-$id" "$(grep -c "\"id\": \"$id\"" "$TABS")" "1"
done
# structural sanity: the QML still balances (deleting whole array elements must not orphan braces)
ob=$(tr -cd '{' < "$TABS" | wc -c); cb=$(tr -cd '}' < "$TABS" | wc -c)
osq=$(tr -cd '[' < "$TABS" | wc -c); csq=$(tr -cd ']' < "$TABS" | wc -c)
chk tabs-braces-balanced "$ob" "$cb"
chk tabs-brackets-balanced "$osq" "$csq"

echo "== (A/B) WIRING: dms is launched against the patched on-disk tree (else the overlay is inert) =="
# the packaged dms runs its EMBEDDED QML; only -c/DMS_SHELL_DIR makes it load /usr/share/quickshell/dms
has "$SWAY" "dms run -c /usr/share/quickshell/dms" "sway-dms-points-at-patched-tree"
# and the patched file must live exactly where -c points
chk tabs-at-c-path "$([ "$TABS" = "$ROOT/layers/shrek-desktop/overlay/usr/share/quickshell/dms/Common/SettingsTabs.qml" ] && echo yes || echo no)" "yes"

echo "== (A/B) drift guard: overlay header is pinned to the mkosi dms version =="
has "$TABS" "dms=$DMS_PIN" "tabs-header-names-pin"
has "$TABS" "SHREK-OS OVERLAY" "tabs-header-provenance"

echo "== (C) changelog popup: launcher pre-seeds the version marker =="
has "$LAUNCHER" ".changelog-$DMS_MM" "launcher-seeds-marker"
# existence-gated + created empty (idempotent; a user's own marker left alone)
has "$LAUNCHER" 'if [ ! -e "$_shrek_changelog_marker" ]' "launcher-marker-gated"
has "$LAUNCHER" ': > "$_shrek_changelog_marker"' "launcher-marker-empty-touch"
# the marker path lives under DankMaterialShell config (where DMS's ChangelogService looks)
has "$LAUNCHER" 'DankMaterialShell/.changelog-' "launcher-marker-path"

echo
echo "DMS-SUPPRESS: PASS=$pass FAIL=$fail"
[ "$fail" -eq 0 ]
