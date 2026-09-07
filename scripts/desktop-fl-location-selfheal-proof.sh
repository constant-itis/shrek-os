#!/usr/bin/env bash
# Verify the FL location self-heal python (extracted verbatim from the shrek-desktop launcher).
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LAUNCHER=$ROOT/layers/shrek-desktop/overlay/usr/bin/shrek-desktop
SEED=$ROOT/layers/shrek-desktop/overlay/usr/share/shrek/dms/default-session.json
WP=/usr/share/shrek/desktop/wallpaper.jpg
T=$(mktemp -d)
# extract the python heredoc body verbatim
awk "/<<'PY'/{f=1;next} /^PY\$/{f=0} f" "$LAUNCHER" > "$T/heal.py"
echo "extracted $(wc -l < "$T/heal.py") lines of python"

run() { SHREK_WP=$WP SHREK_SEED=$SEED python3 "$T/heal.py" "$1" 2>&1; }
field() { python3 -c "import json,sys;print(json.load(open(sys.argv[1])).get(sys.argv[2]))" "$1" "$2"; }
pass=0; fail=0
chk() { if [ "$2" = "$3" ]; then echo "  PASS $1 ($2)"; pass=$((pass+1)); else echo "  FAIL $1 exp=[$3] got=[$2]"; fail=$((fail+1)); fi; }

echo "== case 1: untouched NY default (blank wp, nightIP true) -> heals to FL =="
cat > "$T/c1.json" <<'J'
{"wallpaperPath":"","weatherLocation":"New York, NY","weatherCoordinates":"40.7128,-74.006","latitude":40.7128,"longitude":-74.006,"nightModeUseIPLocation":true,"configVersion":7}
J
run "$T/c1.json"
chk c1-loc     "$(field "$T/c1.json" weatherLocation)"        "Lake Mary, FL"
chk c1-coords  "$(field "$T/c1.json" weatherCoordinates)"     "28.7589,-81.3178"
chk c1-lat     "$(field "$T/c1.json" latitude)"               "28.7589"
chk c1-nightIP "$(field "$T/c1.json" nightModeUseIPLocation)" "False"
chk c1-wp      "$(field "$T/c1.json" wallpaperPath)"          "$WP"
chk c1-cfgver  "$(field "$T/c1.json" configVersion)"          "7"

echo "== case 2: user-chosen location -> LEFT ALONE (only nightIP normalized) =="
cat > "$T/c2.json" <<'J'
{"wallpaperPath":"/home/dev/mypic.jpg","weatherLocation":"Miami, FL","weatherCoordinates":"25.7617,-80.1918","latitude":25.7617,"longitude":-80.1918,"nightModeUseIPLocation":true,"configVersion":7}
J
run "$T/c2.json"
chk c2-loc     "$(field "$T/c2.json" weatherLocation)"        "Miami, FL"
chk c2-coords  "$(field "$T/c2.json" weatherCoordinates)"     "25.7617,-80.1918"
chk c2-wp      "$(field "$T/c2.json" wallpaperPath)"          "/home/dev/mypic.jpg"
chk c2-nightIP "$(field "$T/c2.json" nightModeUseIPLocation)" "False"

echo "== case 3: location keys missing entirely -> heals to FL =="
cat > "$T/c3.json" <<'J'
{"wallpaperPath":"/x.jpg","configVersion":7}
J
run "$T/c3.json"
chk c3-loc    "$(field "$T/c3.json" weatherLocation)"    "Lake Mary, FL"
chk c3-coords "$(field "$T/c3.json" weatherCoordinates)" "28.7589,-81.3178"

echo "== case 4: already FL, nightIP already false -> idempotent (no rewrite needed) =="
cat > "$T/c4.json" <<'J'
{"wallpaperPath":"/x.jpg","wallpaperPathDark":"/x.jpg","wallpaperPathLight":"/x.jpg","weatherLocation":"Lake Mary, FL","weatherCoordinates":"28.7589,-81.3178","latitude":28.7589,"longitude":-81.3178,"nightModeUseIPLocation":false,"configVersion":7}
J
before=$(cat "$T/c4.json"); run "$T/c4.json"; after=$(python3 -c "import json;print(json.dumps(json.load(open('$T/c4.json')),sort_keys=True))")
exp=$(python3 -c "import json;print(json.dumps(json.loads('''$before'''),sort_keys=True))")
chk c4-idempotent "$after" "$exp"

echo "== case 5: malformed session.json -> no crash, no change =="
printf '{ not json' > "$T/c5.json"
run "$T/c5.json"; chk c5-nocrash "$(cat "$T/c5.json")" "{ not json"

echo
echo "FL-SELFHEAL: PASS=$pass FAIL=$fail"
rm -rf "$T"
[ "$fail" -eq 0 ]
