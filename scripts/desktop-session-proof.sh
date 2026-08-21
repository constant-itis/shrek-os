#!/usr/bin/env bash
# Desktop Slice-1 session-view proof — prove the REAL SessionProvider reads a gatekeeperd-authored
# shrek-session/1 record into the Quickshell Work drawer (Phase-8 Slice-1, "go C"). The WRITER is the
# real `gatekeeperd session-view` CLI (host); the READER is ui/providers/SessionProvider.qml running
# under the actual Sway + source-built Quickshell stack (container, headless/software, same idiom as
# scripts/desktop-smoke.sh). One schema, writer and reader identical — that identity IS the acceptance.
#
# Gates (each independent so we learn WHERE it breaks):
#   DS-surfaces  shell surfaces instantiate (no regression) with a record present
#   DS-read      the real provider parses the s0 record -> "work session s0 tier=T2 trust=T-untrust"
#   DS-empty     an EMPTY session dir -> surfaces still instantiate AND no work-session marker
#                (fail-closed: no record => no row, drawer stays "Nothing running", no fake data)
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"; cd "$REPO_ROOT"

# --- host: build gatekeeperd and author a REAL shrek-session/1 record (the writer side) ---
echo "=== building gatekeeperd (release) + authoring shrek-session/1 record ==="
CARGO_NET_OFFLINE=true cargo build --release -p gatekeeperd >/dev/null 2>&1 || \
  cargo build --release -p gatekeeperd
GK=target/release/gatekeeperd
BASE="out/desktop-session-proof"
SDIR="$BASE/session"; EDIR="$BASE/empty"; PROJ="$BASE/proj"
rm -rf "$BASE"; mkdir -p "$SDIR" "$EDIR" "$PROJ"
"$GK" session-view --dir "$SDIR" --session s0 --subject dev1 \
  --tier T2 --trust T-untrust --caps cnet --profile cnet --grant "$(readlink -f "$PROJ")" \
  --egress-profile model-anthropic --egress-dst shrek-model-proxy:8200 \
  --workload-arg coder --workload-arg --provider --workload-arg anthropic \
  --provider anthropic --mode deterministic --semantic-available --semantic-tier fts+semantic
echo "authored: $(ls -l "$SDIR/s0.json")"

# robust pin reader: strip inline comments, then take the quoted value (mirrors desktop-smoke.sh)
pin() { grep -E "^[[:space:]]*$1[[:space:]]*=" image/supply/desktop.pins | head -1 | sed 's/#.*//' | cut -d'"' -f2; }
QS_REPO="$(pin repo)"; QS_TAG="$(pin quickshell_tag)"

echo "=== Desktop session-view proof (repo=${QS_REPO} tag=${QS_TAG}) in debian:trixie ==="
docker run --rm --privileged \
  -v "${REPO_ROOT}:/work" -w /work \
  -e QS_REPO="${QS_REPO}" -e QS_TAG="${QS_TAG}" \
  debian:trixie \
  bash -euo pipefail -c '
    export DEBIAN_FRONTEND=noninteractive
    PASS=0; FAIL=0
    gate() { if [ "$1" = ok ]; then echo "SHREK_GATE: PASS $2"; PASS=$((PASS+1)); else echo "SHREK_GATE: FAIL $2"; FAIL=$((FAIL+1)); fi; }

    echo "--- apt: runtime + build deps ---"
    apt-get update -qq >/dev/null
    apt-get install -y --no-install-recommends -qq \
      ca-certificates openssl \
      sway foot qt6-wayland qml6-module-qtquick \
      git cmake ninja-build build-essential pkg-config \
      qt6-base-dev qt6-base-private-dev qt6-declarative-dev qt6-declarative-private-dev \
      qt6-wayland-dev qt6-wayland-private-dev qt6-shadertools-dev libwayland-dev libwayland-bin \
      wayland-protocols libcli11-dev libdrm-dev
    for p in qml6-module-qtquick-window qml6-module-qtquick-layouts qml6-module-qtquick-shapes \
             qml6-module-qtquick-controls qt6-declarative-dev-tools fonts-dejavu-core libgl1-mesa-dri; do
      apt-get install -y --no-install-recommends -qq "$p" >/dev/null 2>&1 || echo "WARN optional pkg missing: $p"
    done

    export XDG_RUNTIME_DIR=/run/xdgr; mkdir -p "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"
    export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1
    export QT_LOGGING_RULES="qt.qml*=false"

    # newer wayland-protocols over /usr (quickshell v0.3.1 refs ext-background-effect unconditionally)
    git clone --quiet https://gitlab.freedesktop.org/wayland/wayland-protocols /tmp/wp
    WP_TAG="$(cd /tmp/wp && git tag --sort=-v:refname | head -1)"
    ( cd /tmp/wp && git checkout --quiet "$WP_TAG" )
    WP_DATADIR="$(pkg-config --variable=pkgdatadir wayland-protocols 2>/dev/null || echo /usr/share/wayland-protocols)"
    cp -r /tmp/wp/staging /tmp/wp/stable /tmp/wp/unstable "$WP_DATADIR/" 2>/dev/null || echo "WARN wp overlay copy partial"

    # build Quickshell from the pinned tag (minimal feature set; keep WAYLAND + WLR layer-shell)
    git clone --quiet "$QS_REPO" /tmp/qs; cd /tmp/qs
    if [ "$QS_TAG" = "AUTO-FIRST-BUILD" ]; then QS_TAG="$(git tag --sort=-v:refname | head -1)"; fi
    git checkout --quiet "$QS_TAG" || echo "WARN could not checkout $QS_TAG; building default HEAD"
    cmake -S . -B build -GNinja -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr \
      -DHYPRLAND=OFF -DX11=OFF -DI3=OFF -DSCREENCOPY=OFF -DBLUETOOTH=OFF -DNETWORK=OFF \
      -DWAYLAND_SESSION_LOCK=OFF -DWAYLAND_TOPLEVEL_MANAGEMENT=OFF \
      -DSERVICE_STATUS_NOTIFIER=OFF -DSERVICE_PIPEWIRE=OFF -DSERVICE_MPRIS=OFF -DSERVICE_PAM=OFF \
      -DSERVICE_POLKIT=OFF -DSERVICE_GREETD=OFF -DSERVICE_UPOWER=OFF -DSERVICE_NOTIFICATIONS=OFF \
      -DCRASH_HANDLER=OFF -DUSE_JEMALLOC=OFF >/tmp/qs-cmake.log 2>&1 \
      || { echo "WARN cmake configure nonzero — tail:"; tail -40 /tmp/qs-cmake.log; }
    if [ -f build/build.ninja ] && ninja -C build >/tmp/qs-build.log 2>&1 && ninja -C build install >>/tmp/qs-build.log 2>&1; then
      QS="$(command -v quickshell || echo /usr/local/bin/quickshell)"
    else
      QS=""; echo "WARN quickshell build failed"; tail -20 /tmp/qs-build.log
    fi

    # start Sway headless
    export SWAYSOCK=/run/xdgr/sway.sock
    sway -c /work/layers/shrek-desktop/overlay/usr/share/shrek/desktop/sway.config >/tmp/sway.log 2>&1 &
    SWAY_PID=$!
    for i in $(seq 1 30); do swaymsg -t get_version >/dev/null 2>&1 && break; sleep 1; done
    swaymsg -t get_version >/dev/null 2>&1 || { echo "NOTE sway failed to start"; sed -n 1,40p /tmp/sway.log; }
    WD="$(ls "$XDG_RUNTIME_DIR"/wayland-* 2>/dev/null | grep -v "\.lock$" | head -1)"; WD="$(basename "${WD:-wayland-1}")"

    # run quickshell (wayland client of headless sway, software render) against a given session dir
    run_qs() { # $1 = SHREK_SESSION_DIR   $2 = logfile
      [ -n "$QS" ] || { echo "no quickshell binary" > "$2"; return; }
      WAYLAND_DISPLAY="$WD" QT_QPA_PLATFORM=wayland QT_QUICK_BACKEND=software SHREK_SESSION_DIR="$1" \
        timeout 20 "$QS" -p /work/ui/shell.qml >"$2" 2>&1 || true
    }

    # --- DS-surfaces + DS-read: a REAL gatekeeperd record renders in the Work drawer ---
    run_qs /work/out/desktop-session-proof/session /tmp/qs-seed.log
    grep -q "SHREK-DESKTOP shell surfaces instantiated" /tmp/qs-seed.log \
      && gate ok DS-surfaces || { gate no DS-surfaces; sed -n 1,40p /tmp/qs-seed.log; }
    grep -q "SHREK-DESKTOP work session s0 tier=T2 trust=T-untrust" /tmp/qs-seed.log \
      && gate ok DS-read || { gate no DS-read; grep -iE "work session|error|is not a type|cannot|SHREK-DESKTOP" /tmp/qs-seed.log | head; }

    # --- DS-empty: no record => surfaces still instantiate, and NO work-session row (fail-closed) ---
    run_qs /work/out/desktop-session-proof/empty /tmp/qs-empty.log
    if grep -q "SHREK-DESKTOP shell surfaces instantiated" /tmp/qs-empty.log \
       && ! grep -q "SHREK-DESKTOP work session" /tmp/qs-empty.log; then gate ok DS-empty
    else gate no DS-empty; grep -iE "work session|error|SHREK-DESKTOP" /tmp/qs-empty.log | head; fi

    swaymsg exit >/dev/null 2>&1 || true
    for i in $(seq 1 20); do kill -0 "$SWAY_PID" 2>/dev/null || break; sleep 0.5; done
    kill -9 "$SWAY_PID" 2>/dev/null || true

    echo "=================== DESKTOP SESSION-VIEW RESULT ==================="
    echo "PASS=$PASS FAIL=$FAIL"
    [ "$FAIL" = 0 ]
  '
