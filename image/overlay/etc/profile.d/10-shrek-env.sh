# Shrek OS — session environment for the dev user (Dogfood-0 M1). Sourced by login shells: the tty1
# autologin session (so Sway and everything it spawns inherit this) and each foot window (login-shell=yes).

# Admin tools — the systemctl symlinks shutdown/poweroff/reboot live in sbin. Put sbin on PATH so the box
# behaves like a normal machine for the dev user, not only for root.
case ":$PATH:" in
  *:/usr/sbin:*) ;;
  *) PATH="$PATH:/usr/sbin:/sbin" ;;
esac
export PATH

# UTF-8 so foot and TUIs stop falling back to the C locale (box-drawing/glyphs render).
export LANG="${LANG:-C.UTF-8}"
export EDITOR="${EDITOR:-nano}"
export PAGER="${PAGER:-less}"

# Interactive-only niceties.
case "$-" in
  *i*)
    # Calm swamp-green prompt: user@host green, cwd dim, prompt mark accent.
    PS1='\[\e[38;5;107m\]\u@\h\[\e[0m\] \[\e[38;5;245m\]\w\[\e[0m\] \[\e[38;5;107m\]\$\[\e[0m\] '
    alias ls='ls --color=auto'
    alias ll='ls -alF --color=auto'
    alias la='ls -A --color=auto'
    alias l='ls -CF --color=auto'
    alias grep='grep --color=auto'
    ;;
esac
