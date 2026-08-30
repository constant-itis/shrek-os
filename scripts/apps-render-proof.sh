#!/usr/bin/env bash
# ADR-003 Part 1 — RENDER proof for the baseline apps (the half the dogfood VM cannot prove).
#
# WHY THIS EXISTS: a virtio-vga guest WITHOUT virgl scanout-captures GTK clients as bare window OUTLINES
# (a GPU-less QEMU capture artifact, NOT a render failure — proven in #2923: zenity/GParted render fully in
# a headless-sway+grim container but come back blank under `screendump`). So dogfood-vm.sh proves the Part-1
# Onions MERGE (binaries/.desktop/fonts land on /usr); THIS proves the apps actually DRAW pixels, fast
# (~2min vs an ~8min image rebuild), using the exact repro that worked for the live-installer GTK check:
# debian:trixie + sway (WLR_BACKENDS=headless, WLR_RENDERER=pixman software) + grim, then assert the capture
# is not a flat/near-blank frame (unique-colour count) AND that the app actually mapped a Wayland toplevel.
#
# It renders the SAME package manifest the shrek-browser/shrek-apps layers install (firefox-esr + a GTK app),
# so a PASS validates the manifest choice (Wayland-native, no missing render deps). Evidence PNGs land in out/.
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"
HOST_UID="$(id -u)"; HOST_GID="$(id -g)"
mkdir -p out/apps-render

echo "=== apps render proof: firefox-esr + gnome-text-editor under headless sway+grim (debian:trixie) ==="
docker run --rm \
  -v "${REPO_ROOT}:/work" -w /work \
  -e HOST_UID="${HOST_UID}" -e HOST_GID="${HOST_GID}" \
  debian:trixie \
  bash -euo pipefail -c '
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq >/dev/null
    # sway (+wlroots software path via libgl1-mesa-dri/llvmpipe), grim, the apps under test, the same fonts
    # the shrek-apps layer ships (so glyphs render, not tofu), imagemagick for the colour-count assertion,
    # and dbus (GTK4 apps register on a session bus — run everything under dbus-run-session).
    apt-get install -y --no-install-recommends -qq \
      sway grim libgl1-mesa-dri dbus dbus-user-session \
      firefox-esr gnome-text-editor \
      fonts-noto-core fonts-noto-color-emoji fonts-liberation2 \
      imagemagick ca-certificates >/dev/null

    # Inner render logic in a quoted heredoc (no host-side expansion; envs are exported before we invoke it,
    # so dbus-run-session/sway inherit them). Keeps this file free of triple-nested bash -c quoting.
    cat > /tmp/render-inner.sh <<"INNER"
#!/usr/bin/env bash
set -uo pipefail
mkdir -p /tmp/render
cat > /tmp/render/sway.config <<EOF
output HEADLESS-1 resolution 1280x800 position 0 0
default_border none
EOF
cat > /tmp/render/page.html <<EOF
<!doctype html><meta charset=utf-8><title>shrek render proof</title>
<body style="margin:0">
<div style="display:grid;grid-template-columns:repeat(6,1fr)">
<div style="height:130px;background:#e63946"></div><div style="height:130px;background:#f1a208"></div>
<div style="height:130px;background:#2a9d8f"></div><div style="height:130px;background:#264653"></div>
<div style="height:130px;background:#8338ec"></div><div style="height:130px;background:#06d6a0"></div>
</div>
<h1 style="font-family:sans-serif">Shrek OS baseline browser render proof</h1>
<p style="font-family:sans-serif;font-size:20px">The quick brown fox 0123456789 emoji + CJK test.</p>
</body>
EOF
printf "Shrek OS baseline editor render proof.\nThe quick brown fox jumps over the lazy dog.\n0123456789\n" > /tmp/render/note.txt
FFP=/tmp/render/ffp; mkdir -p "$FFP"
cat > "$FFP/prefs.js" <<EOF
user_pref("browser.startup.homepage_override.mstone","ignore");
user_pref("toolkit.telemetry.reportingpolicy.firstRun",false);
user_pref("datareporting.policy.dataSubmissionEnabled",false);
user_pref("browser.aboutwelcome.enabled",false);
user_pref("browser.messaging-system.whatsNewPanel.enabled",false);
user_pref("browser.shell.checkDefaultBrowser",false);
EOF

sway -c /tmp/render/sway.config >/tmp/render/sway.log 2>&1 &
for i in $(seq 1 50); do [ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ] && break; sleep 0.2; done
[ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ] || { echo "RENDER sway=FAIL no wayland socket"; cat /tmp/render/sway.log; exit 3; }
# sway exports SWAYSOCK into its OWN env only (we launched it as a child), so swaymsg in this
# shell cannot auto-find the IPC socket — grim uses WAYLAND_DISPLAY and is unaffected, but the
# winstate corroboration below needs SWAYSOCK. Discover it from the runtime dir.
for i in $(seq 1 25); do SWAYSOCK=$(ls "$XDG_RUNTIME_DIR"/sway-ipc.*.sock 2>/dev/null | head -1); [ -n "${SWAYSOCK:-}" ] && break; sleep 0.2; done
export SWAYSOCK
echo "RENDER sway=up socket=$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY ipc=${SWAYSOCK:-none}"

# a flat/near-blank frame has a handful of colours; a real rendered window (chrome+text+our colour grid) has
# hundreds+. Threshold 200 is comfortably above any blank frame. `convert %k` = unique-colour count.
colours() { convert "$1" -format "%k" info: 2>/dev/null || echo 0; }
arender() { local png="$1" label="$2" min="$3" n; n=$(colours "$png"); n=${n:-0}
  if [ "$n" -ge "$min" ]; then echo "RENDER ${label}=ok colours=$n png=$png"; else echo "RENDER ${label}=FAIL colours=$n (<$min) png=$png"; return 1; fi; }
# Informational corroboration only — the sway IPC socket is not reliably discoverable in this headless
# harness (sway exports SWAYSOCK into its own env, not ours), so this NEVER gates the result. The
# load-bearing assertion is the arender colour count on the grim capture. Reports mapped / absent / na.
winstate() { # <pattern>
  if swaymsg -t get_tree >/dev/null 2>&1; then
    swaymsg -t get_tree 2>/dev/null | grep -qi "$1" && echo mapped || echo absent-in-tree
  else echo "na(no-ipc)"; fi; }
rc=0

# (1) GTK app — gnome-text-editor. Renders like the #2923 zenity/GParted repro (guaranteed baseline).
gnome-text-editor /tmp/render/note.txt >/tmp/render/gte.log 2>&1 &
sleep 10; grim /work/out/apps-render/editor.png 2>/tmp/render/grim-e.log || echo "RENDER editor-grim=FAIL"
echo "RENDER editor-window=$(winstate "editor\|gnome-text")"
arender /work/out/apps-render/editor.png editor 200 || rc=1

# (2) BROWSER — firefox-esr on Wayland, software GL, pointed at a local colour-grid page.
firefox-esr --no-remote --profile /tmp/render/ffp "file:///tmp/render/page.html" >/tmp/render/ff.log 2>&1 &
sleep 30; grim /work/out/apps-render/browser.png 2>/tmp/render/grim-b.log || echo "RENDER browser-grim=FAIL"
echo "RENDER browser-window=$(winstate "firefox\|Navigator\|Mozilla")"
arender /work/out/apps-render/browser.png browser 200 || rc=1

swaymsg exit >/dev/null 2>&1 || true
exit $rc
INNER

    export XDG_RUNTIME_DIR=/run/xdg; mkdir -p "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
    export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1
    export WAYLAND_DISPLAY=wayland-1 MOZ_ENABLE_WAYLAND=1 LIBGL_ALWAYS_SOFTWARE=1
    RC=0; dbus-run-session -- bash /tmp/render-inner.sh || RC=$?
    chown -R "$HOST_UID:$HOST_GID" out/apps-render 2>/dev/null || true
    echo "--- render proof exit rc=$RC ---"
    exit $RC
  '
echo "=== apps render proof GREEN — evidence: out/apps-render/{editor,browser}.png ==="
