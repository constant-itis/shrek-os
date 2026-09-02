# Shrek OS — render the provisioned OWNER display name in the shell prompt (#2939, must-fix 7).
#
# Owner provisioning writes the display name to /home/.shrek-identity/owner and leaves the /etc/passwd
# GECOS sealed (Identity-Model-A keeps the `dev`/uid-1000 slot immutable). Sourced AFTER 10-shrek-env.sh
# (which sets the default `dev@shrek` PS1), so on a provisioned box this overrides the prompt to show the
# owner's chosen name instead of the internal `dev@` account — the account name is an implementation
# detail the owner never chose. No-op on an unprovisioned box (LIVE_INSTALLER / pre-wizard): the file is
# absent and the 10-shrek-env.sh prompt stands.
if [ -r /home/.shrek-identity/owner ]; then
    _shrek_owner=$(head -n1 /home/.shrek-identity/owner 2>/dev/null)
    if [ -n "$_shrek_owner" ]; then
        case "$-" in
        *i*)
            # Same calm swamp-green palette as 10-shrek-env.sh, owner name in place of dev@host.
            PS1='\[\e[38;5;107m\]'"$_shrek_owner"'\[\e[0m\] \[\e[38;5;245m\]\w\[\e[0m\] \[\e[38;5;107m\]\$\[\e[0m\] '
            ;;
        esac
    fi
    unset _shrek_owner
fi
