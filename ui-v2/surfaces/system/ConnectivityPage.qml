import QtQuick
import "../../components"
import "../../theme"
import "../../services"

// ConnectivityPage — the DMS Settings "Connectivity" panel (ADR-007 S3, §8).
//
// The steady-state face of the desktop-egress bless plane: the desktop starts sealed, the baseline
// (time + updates) is on and explained, and the human blesses the rest. Reads its status from the
// `Egress` service's file projection (never the 0700 store); the ONLY mutation offered here is the
// one-click weather bless (Tier-B). `web-browsing` is broad and routes through the console ceremony
// (S4) — shown, not toggled. Baseline is legible + inspectable but revoke-via-ceremony only (Q8): its
// "revoke" affordance is explanation-only and NEVER wired to `unbless` (defense in depth — the daemon
// refuses a baseline unbless regardless).
Flickable {
    id: page
    contentWidth: width
    contentHeight: body.implicitHeight
    clip: true
    boundsBehavior: Flickable.StopAtBounds

    // --- presentation helpers --------------------------------------------------------------------
    function friendlyName(n) {
        if (n === "desktop-ntp") return "Time sync"
        if (n === "desktop-updates") return "System updates"
        if (n === "weather") return "Weather"
        if (n === "web-browsing") return "Web browsing"
        return n
    }
    function purpose(n) {
        if (n === "desktop-ntp") return "Keeps the clock correct (secure NTP)."
        if (n === "desktop-updates") return "Fetches signed system updates."
        if (n === "weather") return "Lets the weather widget reach its forecast API."
        if (n === "web-browsing") return "Opens broad internet access for the browser."
        return ""
    }
    // A short status line + its tint, derived ONLY from the projected state (display truth).
    function statusText(p) {
        if (p.tier === "baseline") return p.hasPins ? "On" : "On"
        if (!p.blessed) return p.tier === "ceremony" ? "Needs console approval" : "Off"
        if (p.live) return "Active"
        if (p.fault === "apply-fail") return "Needs attention — will retry"
        return "Blessed — waiting for network"
    }
    function statusTint(p) {
        if (p.tier === "baseline") return Tokens.success
        if (!p.blessed) return Tokens.muted
        if (p.live) return Tokens.success
        if (p.fault === "apply-fail") return Tokens.danger
        return Tokens.warning
    }
    function pinsText(p) {
        return p.hasPins ? ("Pinned: " + p.pins.join(", ")) : ""
    }

    Column {
        id: body
        width: parent.width
        spacing: Tokens.spaceLg

        ShrekSection {
            title: "Connectivity"
            detail: "Your desktop starts sealed. Choose what it may reach — each choice is pinned to a specific destination, nothing else opens."

            // Fail-closed banner if the supervisor's projection isn't readable yet.
            ShrekCard {
                visible: !Egress.available
                height: visible ? implicitHeight : 0
                Text {
                    width: parent.width
                    text: "Connectivity status is unavailable right now."
                    color: Tokens.textSecondary
                    font.family: Tokens.fontFamily
                    font.pixelSize: Tokens.fontCaption
                    wrapMode: Text.WordWrap
                }
            }

            Repeater {
                model: Egress.available ? Egress.profiles : []

                ShrekSettingRow {
                    required property var modelData
                    readonly property bool isWeather: modelData.name === "weather"
                    readonly property bool isCeremony: modelData.tier === "ceremony"
                    readonly property bool isBaseline: modelData.tier === "baseline"

                    title: page.friendlyName(modelData.name)
                    detail: {
                        var s = page.statusText(modelData)
                        var pins = page.pinsText(modelData)
                        return pins.length > 0 ? (s + "  ·  " + pins) : s
                    }
                    enabledRow: false

                    // One-click weather bless (Tier-B). No optimistic flip: the toggle reflects the
                    // projected `blessed`, and is disabled while an action is in flight (debounce +
                    // "don't mask a denied/resolve-failed bless").
                    ShrekToggle {
                        visible: isWeather
                        checked: modelData.blessed
                        available: !Egress.busy(modelData.name)
                        onToggled: {
                            if (modelData.blessed) Egress.unbless(modelData.name)
                            else Egress.bless(modelData.name)
                        }
                    }

                    // A blessed-but-pending weather offers a single user-initiated retry (within the
                    // rate budget — never an auto-loop, which would starve real clicks).
                    ShrekButton {
                        visible: isWeather && modelData.pending
                        text: "Try now"
                        compact: true
                        enabled: !Egress.busy(modelData.name)
                        onActivated: Egress.repin(modelData.name)
                    }

                    // Baseline: a status word, no control. Revoke is console-ceremony only (Q8).
                    Text {
                        visible: isBaseline
                        text: page.statusText(modelData)
                        color: page.statusTint(modelData)
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontSmall
                        font.weight: Font.DemiBold
                    }

                    // Web browsing: broad grant, ceremony-only. Shown, not toggled here (S4).
                    Text {
                        visible: isCeremony
                        text: modelData.blessed ? "Enabled" : "Console approval"
                        color: modelData.blessed ? Tokens.success : Tokens.muted
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontSmall
                        font.weight: Font.DemiBold
                    }
                }
            }
        }

        // The high-consequence / always-on explanations, so the tiers read as deliberate, not arbitrary.
        ShrekSection {
            title: "About these choices"

            Text {
                width: parent.width
                text: "Time and updates are part of the system baseline — always on, and turned off only through a console confirmation (turning them off can break the clock and leave the system unpatched)."
                color: Tokens.textSecondary
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontCaption
                wrapMode: Text.WordWrap
            }
            Text {
                width: parent.width
                text: "Web browsing opens broad internet access, so it is approved at the console rather than with a single click here."
                color: Tokens.textSecondary
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontCaption
                wrapMode: Text.WordWrap
            }

            // Last activity from the downstream notification log (consumes /run/shrek/egress/events).
            Text {
                width: parent.width
                visible: Egress.lastEvent !== null
                text: Egress.lastEvent
                      ? ("Last change: " + page.friendlyName(Egress.lastEvent.profile) + " — "
                         + Egress.lastEvent.verb + " (" + Egress.lastEvent.result + ")")
                      : ""
                color: Tokens.muted
                font.family: Tokens.fontFamily
                font.pixelSize: Tokens.fontCaption
                wrapMode: Text.WordWrap
            }
        }
    }
}
