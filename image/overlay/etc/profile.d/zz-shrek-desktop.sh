# Dogfood-0 (M0) — auto-start the Shrek desktop on the first VT under the autologin PAM/logind session.
#
# Sourced by /etc/profile for the login shell that agetty --autologin creates on tty1 — the non-root
# `dev` user as of Dogfood-0 M1 (was root in M0). Guarded so it fires ONLY on a real graphical VT that
# has a DRM device — so it NEVER runs on the headless CI serial gate (std-vga, no /dev/dri), leaving
# scripts/desktop-sealed-proof.sh and the other sealed proofs unaffected. dev's session owns seat0
# (pam_systemd), so sway's libseat→logind backend gets the DRM/input uaccess ACLs without group edits.
#
#   XDG_VTNR=1 or tty=/dev/tty1
#                       the first VT login; some autologin paths do not export XDG_VTNR reliably
#   WAYLAND_DISPLAY unset  don't recurse if a session is already up
#   /dev/dri/card0      a real KMS device exists (virtio-gpu in the dogfood VM; absent on the CI std-vga)
#   shrek-desktop       the shrek-desktop sysext merged onto /usr (oniond ran)
#
# Renderer selection belongs to wlroots/Qt in interactive sessions. Headless proofs pass pixman/software
# explicitly through their own harnesses.
_shrek_tty="$(tty 2>/dev/null || true)"
if { [ "${XDG_VTNR:-}" = 1 ] || [ "$_shrek_tty" = /dev/tty1 ]; } \
   && [ -z "${WAYLAND_DISPLAY:-}" ] \
   && [ -e /dev/dri/card0 ] && command -v shrek-desktop >/dev/null 2>&1; then
    # §5b compositor keymap adapter: XKBLAYOUT (in /etc/vconsole.conf, bound from the provisioning store) is
    # the ONE canonical source; export it as XKB_DEFAULT_LAYOUT so sway/libxkbcommon picks up the
    # provisioned layout at context creation (the shipped sway.config sets no xkb_layout/input override, so
    # the env is authoritative). Parse it WITHOUT sourcing vconsole.conf (never eval config). No-op / left
    # to the libxkbcommon default when unset or `us`.
    if [ -r /etc/vconsole.conf ]; then
        _xkbl="$(grep -E '^XKBLAYOUT=' /etc/vconsole.conf 2>/dev/null | tail -n1)"
        _xkbl="${_xkbl#XKBLAYOUT=}"; _xkbl="${_xkbl%\"}"; _xkbl="${_xkbl#\"}"; _xkbl="${_xkbl%\'}"; _xkbl="${_xkbl#\'}"
        case "$_xkbl" in ''|*[!a-z0-9]*) : ;; *) export XKB_DEFAULT_LAYOUT="$_xkbl" ;; esac
        unset _xkbl
    fi
    exec shrek-desktop
fi
unset _shrek_tty
