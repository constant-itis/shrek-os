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

            // The SAK ceremony instruction — shown while a console ceremony is in flight. The approval
            // does NOT happen in this panel: the screen switches to a secure text console.
            ShrekCard {
                visible: Egress.ceremonyActive
                height: visible ? implicitHeight : 0
                Column {
                    width: parent.width
                    spacing: Tokens.spaceXs
                    Text {
                        width: parent.width
                        text: "Approve at the console"
                        color: Tokens.accent
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontSmall
                        font.weight: Font.DemiBold
                    }
                    Text {
                        width: parent.width
                        text: (Egress.ceremonyLabel.length > 0 ? (Egress.ceremonyLabel + ": ") : "")
                              + "press the Secure Attention key (Ctrl-Alt-Break), then type the code shown on the secure screen to approve. Anything else denies."
                        color: Tokens.textSecondary
                        font.family: Tokens.fontFamily
                        font.pixelSize: Tokens.fontCaption
                        wrapMode: Text.WordWrap
                    }
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

                    // Web browsing: broad grant, ceremony-only (S4). The button LAUNCHES the console
                    // ceremony (it does not flip anything here) — the human approves at the secure text
                    // console. The state projection remains the display truth for what actually blessed.
                    ShrekButton {
                        visible: isCeremony
                        text: modelData.blessed ? "Turn off (console)" : "Set up at console"
                        compact: true
                        enabled: !Egress.ceremonyActive
                        onActivated: {
                            if (modelData.blessed) Egress.unblessCeremony(modelData.name)
                            else Egress.blessCeremony(modelData.name)
                        }
                    }
                }
            }
        }

        // Advanced raw-destination editor (S4, §8): add a specific host:proto:port. Each add/remove is a
        // console ceremony (broad-consequence: an attacker-chosen host), resolve-and-pinned like a profile.
        ShrekSection {
            title: "Advanced destinations"
            detail: "Allow a specific host, protocol and port. Each is approved at the console and pinned to the address it resolves to — nothing else opens."

            // Existing raw destinations, each with a console-ceremony remove.
            Repeater {
                model: Egress.available ? Egress.rawEntries : []
                ShrekSettingRow {
                    required property var modelData
                    title: modelData.wire
                    detail: modelData.hasPins
                            ? ("Active  ·  Pinned: " + modelData.pins.join(", "))
                            : "Blessed — waiting for network"
                    enabledRow: false
                    ShrekButton {
                        text: "Remove"
                        compact: true
                        enabled: !Egress.ceremonyActive
                        onActivated: Egress.removeRaw(modelData.wire)
                    }
                }
            }

            // The add row: a host:proto:port field + a console-ceremony "Add" button.
            ShrekCard {
                Row {
                    width: parent.width
                    spacing: Tokens.spaceSm
                    Rectangle {
                        width: parent.width - addBtn.width - Tokens.spaceSm
                        height: 32
                        radius: Tokens.radiusSm
                        color: Tokens.surfaceAlt
                        border.color: rawInput.activeFocus ? Tokens.accent : Tokens.border
                        border.width: 1
                        TextInput {
                            id: rawInput
                            anchors.fill: parent
                            anchors.leftMargin: Tokens.spaceSm
                            anchors.rightMargin: Tokens.spaceSm
                            verticalAlignment: TextInput.AlignVCenter
                            clip: true
                            color: Tokens.text
                            font.family: Tokens.fontFamily
                            font.pixelSize: Tokens.fontSmall
                            selectionColor: Tokens.accentDim
                            // A host:proto:port hint when empty.
                            Text {
                                anchors.verticalCenter: parent.verticalCenter
                                visible: rawInput.text.length === 0
                                text: "example.com:tcp:443"
                                color: Tokens.muted
                                font: rawInput.font
                            }
                        }
                    }
                    ShrekButton {
                        id: addBtn
                        text: "Add at console"
                        compact: true
                        enabled: !Egress.ceremonyActive && rawInput.text.trim().length > 0
                        onActivated: {
                            Egress.addRaw(rawInput.text.trim())
                            rawInput.text = ""
                        }
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
