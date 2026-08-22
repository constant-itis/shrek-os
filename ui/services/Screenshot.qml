pragma Singleton
import QtQuick
import Quickshell

// Screenshot — region / full-screen capture via grim (+ slurp for region select), behind ONE seam. Saves
// under ~/Pictures/Screenshots, also copies the PNG to the clipboard, and confirms through our own
// notification server (notify-send). grim uses the compositor's wlr-screencopy directly, independent of
// Quickshell's own SCREENCOPY build flag. Ordinary user action — no authority here.
QtObject {
    // Shell prelude: resolve the output dir + a timestamped filename in-shell (avoids QML/date/quoting
    // pitfalls). `$f` is the target path for the grim invocations that follow.
    readonly property string _prep: 'd="$HOME/Pictures/Screenshots"; mkdir -p "$d"; f="$d/shot-$(date +%Y%m%d-%H%M%S).png"; '
    readonly property string _finish: ' && wl-copy < "$f" && notify-send "Screenshot saved" "$f"'

    // Region select (slurp). If the user cancels slurp (Esc), $g is empty and we exit cleanly — no file.
    function region() {
        Quickshell.execDetached(["sh", "-c",
            _prep + 'g="$(slurp)"; [ -n "$g" ] || exit 0; grim -g "$g" "$f"' + _finish])
    }
    // Whole output.
    function screen() {
        Quickshell.execDetached(["sh", "-c", _prep + 'grim "$f"' + _finish])
    }
}
