# Dogfood-0 (M0) — auto-start the Shrek desktop on the first VT under the autologin PAM/logind session.
#
# Sourced by /etc/profile for the login shell that agetty --autologin creates on tty1 — the non-root
# `dev` user as of Dogfood-0 M1 (was root in M0). Guarded so it fires ONLY on a real graphical VT that
# has a DRM device — so it NEVER runs on the headless CI serial gate (std-vga, no /dev/dri), leaving
# scripts/desktop-sealed-proof.sh and the other sealed proofs unaffected. dev's session owns seat0
# (pam_systemd), so sway's libseat→logind backend gets the DRM/input uaccess ACLs without group edits.
#
#   XDG_VTNR=1          set by pam_systemd for the tty1 session (present because libpam-systemd is baked)
#   WAYLAND_DISPLAY unset  don't recurse if a session is already up
#   /dev/dri/card0      a real KMS device exists (virtio-gpu in the dogfood VM; absent on the CI std-vga)
#   shrek-desktop       the shrek-desktop sysext merged onto /usr (oniond ran)
#
# Renderer selection belongs to wlroots/Qt in interactive sessions. Headless proofs pass pixman/software
# explicitly through their own harnesses.
if [ "${XDG_VTNR:-}" = 1 ] && [ -z "${WAYLAND_DISPLAY:-}" ] \
   && [ -e /dev/dri/card0 ] && command -v shrek-desktop >/dev/null 2>&1; then
    exec shrek-desktop
fi
