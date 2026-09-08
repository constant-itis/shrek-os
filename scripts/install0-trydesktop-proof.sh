#!/usr/bin/env bash
# INSTALL-0 "Try Shrek Desktop (live)" mechanism proof (#3110).
#
# Proves the reload-swap plumbing DETERMINISTICALLY, without DMS: sway-live.config globs an empty
# include dir, so the live session is just the chooser; the REAL shrek-live-welcome "trydesktop" action
# drops sway-try-desktop.config into that dir and runs `swaymsg reload`, after which the full desktop
# config (keybinds + surfaces + DMS launcher) is live. (Whether DMS itself renders on real hardware is
# metal-gated, like the Quickshell installer — see #3101; this proof covers the config MECHANISM the
# owner flagged as risky: does a reload actually swap the minimal live session into the full desktop.)
#
# Observable: sway's `get_config` IPC returns ONLY the top-level file (not includes), so we don't grep
# it. Instead we stub `dms` to write a marker when run — the fragment (re)launches DMS via exec_always,
# which fires on the reload that pulls the fragment in. Marker absent before / present after == the whole
# chain worked: $XDG_RUNTIME_DIR include expanded (wordexp) -> fragment loaded -> exec_always fired.
# Runs headless sway (pixman, seatless) in debian:trixie — no KVM, no image, ~1min.
set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"; mkdir -p out

OVL=layers/shrek-installer/overlay/usr/share/shrek/installer
DESK=layers/shrek-desktop/overlay/usr/share/shrek/desktop
WELCOME=layers/shrek-installer/overlay/usr/bin/shrek-live-welcome
for f in "$OVL/sway-live.config" "$OVL/sway-try-desktop.config" "$DESK/sway.config" "$WELCOME"; do
  [ -r "$f" ] || { echo "missing $f" >&2; exit 1; }
done

echo "=== INSTALL-0 try-desktop mechanism proof (headless sway, no DMS) ==="
docker run --rm -v "${REPO_ROOT}:/work:ro" -w /work \
  -e OVL="$OVL" -e DESK="$DESK" -e WELCOME="$WELCOME" \
  debian:trixie \
  bash -uo pipefail -c '
    export DEBIAN_FRONTEND=noninteractive LC_ALL=C.UTF-8 LANG=C.UTF-8
    apt-get update -qq >/dev/null 2>&1
    apt-get install -y --no-install-recommends -qq sway locales-all >/dev/null 2>&1

    # Stage the overlay tree at the SAME absolute paths the fragment/config reference, so
    # `include /usr/share/shrek/desktop/sway.config` resolves exactly as on the live medium.
    mkdir -p /usr/share/shrek/installer /usr/share/shrek/desktop /usr/local/bin
    cp "/work/$OVL/sway-live.config"        /usr/share/shrek/installer/sway-live.config
    cp "/work/$OVL/sway-try-desktop.config" /usr/share/shrek/installer/sway-try-desktop.config
    cp "/work/$DESK/sway.config"            /usr/share/shrek/desktop/sway.config
    cp "/work/$WELCOME"                     /usr/local/bin/shrek-live-welcome
    chmod +x /usr/local/bin/shrek-live-welcome
    # Dummy wallpapers so `output * bg` succeeds (keeps the compositor log clean; content irrelevant).
    : > /usr/share/shrek/installer/live-wallpaper.jpg
    : > /usr/share/shrek/desktop/wallpaper.jpg
    # Stub the exec targets. `dms` writes a marker when run: that marker is our proof the fragment loaded
    # and its exec_always fired. The rest are inert no-ops so binds/execs resolve to something.
    printf "#!/bin/sh\ntouch /run/shrek-dms-ran\n" > /usr/local/bin/dms; chmod +x /usr/local/bin/dms
    # `qs` is launched ONLY by the desktop config (exec_always shrek-menu / shrek-connectivity), which is
    # pulled in via the fragment`s `include`. A qs marker therefore proves the INCLUDED desktop config
    # (keybinds + surfaces) actually loaded, not merely the fragment`s own lines.
    printf "#!/bin/sh\ntouch /run/shrek-qs-ran\n" > /usr/local/bin/qs; chmod +x /usr/local/bin/qs
    for b in foot shrek-install-ui shrek-agent brightnessctl shrek-boot-toast shrek-try-desktop-hello; do
      printf "#!/bin/sh\ntrue\n" > "/usr/local/bin/$b"; chmod +x "/usr/local/bin/$b"; done

    export XDG_RUNTIME_DIR=/run/xdg; mkdir -p "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
    export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 LIBGL_ALWAYS_SOFTWARE=1
    export WAYLAND_DISPLAY=wayland-1

    sway -c /usr/share/shrek/installer/sway-live.config >/run/sway.log 2>&1 &
    swpid=$!
    for i in $(seq 1 30); do
      SWAYSOCK=$(ls "$XDG_RUNTIME_DIR"/sway-ipc.*.sock 2>/dev/null | head -1)
      [ -n "$SWAYSOCK" ] && break || sleep 1
    done
    [ -n "${SWAYSOCK:-}" ] || { echo "SWAY-NOSOCK"; grep -a . /run/sway.log | tail; kill $swpid 2>/dev/null; exit 1; }
    export SWAYSOCK
    sleep 2   # let boot-time exec/exec_always settle

    echo "--- baseline (chooser session, empty include glob) ---"
    # The DMS launcher + desktop surfaces live ONLY in the fragment path, so pre-swap neither ran.
    { [ -e /run/shrek-dms-ran ] || [ -e /run/shrek-qs-ran ]; } && echo "BASE-DMS-FAIL(ran before swap)" || echo "BASE-CLEAN-OK"

    echo "--- fire the REAL shrek-live-welcome trydesktop action (autorun hook) ---"
    SHREK_LIVE_AUTORUN=trydesktop /usr/local/bin/shrek-live-welcome || true

    [ -L "$XDG_RUNTIME_DIR/shrek-live.d/10-try-desktop.config" ] && echo "SYMLINK=YES" || echo "SYMLINK=NO"

    sleep 3   # let the reload the action triggered pull in + run the fragment
    [ -e /run/shrek-dms-ran ] && echo "AFTER-DMS-OK" || echo "AFTER-DMS-FAIL(fragment did not load/fire)"
    # qs ran => the desktop config the fragment `include`s actually loaded (its exec_always surfaces fired).
    [ -e /run/shrek-qs-ran ] && echo "AFTER-DESKCONFIG-LOADED-OK" || echo "AFTER-DESKCONFIG-LOADED-FAIL"
    # And no expansion/parse failure on the fragment path.
    if grep -aiE "shrek-live\.d.*(no such|error|cannot|unable)" /run/sway.log; then
      echo "FRAGMENT-LOAD-ERROR"; else echo "FRAGMENT-NOERROR-OK"; fi

    swaymsg exit >/dev/null 2>&1 || kill $swpid 2>/dev/null || true
  ' 2>&1 | tee out/install0-trydesktop-proof.log

echo ""
echo "=== tally ==="
LOG=out/install0-trydesktop-proof.log
pass=0; fail=0
ok(){ echo "  PASS $*"; pass=$((pass+1)); }
bad(){ echo "  FAIL $*"; fail=$((fail+1)); }
grep -q "BASE-CLEAN-OK"              "$LOG" && ok "empty glob = chooser only (DMS launcher not fired at boot)" || bad "fragment/DMS active before the swap"
grep -q "SYMLINK=YES"               "$LOG" && ok "trydesktop action dropped the fragment symlink"             || bad "fragment symlink not created by the action"
grep -q "AFTER-DMS-OK"              "$LOG" && ok "reload-swap loaded the fragment + fired DMS (exec_always)"  || bad "fragment did not take effect after swap+reload"
grep -q "AFTER-DESKCONFIG-LOADED-OK" "$LOG" && ok "reload pulled in the full desktop config (keybinds/surfaces)" || bad "desktop config not loaded on reload"
grep -q "FRAGMENT-NOERROR-OK"       "$LOG" && ok "no include-expansion / parse error on the fragment"         || bad "fragment failed to expand/parse"

echo "--- try-desktop mechanism tally: PASS=$pass FAIL=$fail ---"
[ "$fail" -eq 0 ] && echo "=== try-desktop mechanism proof GREEN ===" || { echo "=== try-desktop mechanism proof NOT GREEN — inspect $LOG ==="; exit 1; }
