# Shell architecture — Shrek Shell (the desktop/console layer)

> Design-of-record for the roadmap **Phase 10** surface. Thesis: Shrek does **not** build a desktop
> environment. It defines a set of **roles**, ships a **default implementation** per role, and manages
> them as **systemd user services** — so the environment *feels* like a finished DE but is internally a
> bundle of independently replaceable, WM-era components. The whole shell is an **optional Onion layer**
> (Phase 2) above the boring headless base; the base stays server-capable with no shell at all.
> "i3/Sway philosophy + DE completeness + appliance-like defaults + Unix-style modularity."

## The invariant this doc must not break

The shell is where a human sees and approves what agents ask for. The one role that carries the
project's core invariant (`semantic authority ≤ data authority`, README / security-model.md) into the
UI is **policy/agent-UI** — the grant prompt. Everything else on the role list is cosmetic and freely
swappable; that one role is **trusted-path** and is *not* freely swappable (§4). Get this asymmetry
wrong and the invariant dies at a mouse click.

## The role model

A **role** is a stable interface; the **component** behind it is an implementation detail. The role is
defined by a **contract** (a D-Bus interface, a Wayland protocol, a freedesktop spec) — not by the
program. Wherever a real cross-implementation contract already exists, we **adopt** it; we only
**invent** a contract where none exists. This is the same "borrow, not build" discipline as
architecture.md §3 (systemd-sysext does the layering; Shrek only orchestrates).

Consequence: roles are **transport-agnostic**. "Launcher" is a role whether it's Fuzzel (Wayland) or
`fzf` in a pane (TTY). The role stays; the implementation swaps per rung.

Roles: `compositor` · `bar` · `launcher` · `notifications` · `session/lock` · `portals` · `audio` ·
`network` · `bluetooth` · `file-handling` · **`policy/agent-UI`**.

## The three-rung ladder (same base, chosen by which layer you strap on)

```
Rung 0 — bare       no shell layer. getty + vanilla $SHELL. keyboard+screen, server/headless.
                    GUI = whatever a plain terminal does, nothing more.
Rung 1 — TUI shell  a terminal-native "desktop": mux + bar + launcher + notifications + lock,
                    NO Wayland/X11. one signed sysext layer. runs identically over SSH.
Rung 2 — Wayland    the graphical hybrid DE. a heavier signed layer set.
```

Same base, same Onion mechanism (Phase 2), three targets. Picking a rung = enabling a layer, not
installing a different OS.

## Role → contract table (adopt vs build, per rung)

`ADOPT` = ship an existing component configured to a standard contract. `BUILD` = we author it.
`DEFINE` = a real component exists to adopt, but **no cross-implementation contract does**, so
swappability is by *our own* convention (a systemd user unit + our config schema), not a standard.
Both rungs grounded by research 2026-08-18 (terminal + Wayland prior art).

| Role | Contract (the seam) | Rung 1 (TTY) | Rung 2 (Wayland) |
| --- | --- | --- | --- |
| compositor / session | UWSM + `graphical-session.target` (session lifecycle); the compositor is the transport | ADOPT **zellij** (Rust, MIT; WASM plugin API = the swap mechanism) | ADOPT wlroots compositor (Sway/river) run as a **UWSM** systemd user session |
| bar | **none — we DEFINE** | ADOPT zellij status (Rust/WASM plugin) | ADOPT Waybar-class as a user unit |
| launcher | **none — we DEFINE** | ADOPT `fzf`-logic as a zellij plugin | ADOPT Fuzzel-class as a user unit |
| notifications | `org.freedesktop.Notifications` (D-Bus, real spec) | **BUILD** — no TTY standard exists (the gap) | ADOPT mako/dunst/swaync (spec-compliant) |
| session / login | **greetd** JSON-IPC (clean daemon↔greeter split) | ADOPT **tuigreet** | ADOPT greetd + regreet/gtkgreet |
| lock + idle | **`ext-session-lock-v1`** (fail-secure Wayland proto) + **`ext-idle-notify-v1`** for idle | ADOPT physlock-class VT locker (external, GPL — don't vendor) | ADOPT swaylock/gtklock (ext-session-lock) + swayidle-class idle daemon |
| portals | `xdg-desktop-portal` spec + backends | n/a (no portal surface in a TTY) | ADOPT xdp frontend + **compose ≥2 backends** (gtk for FileChooser/Settings + wlr for ScreenCast/Screenshot) |
| audio | PipeWire / MPRIS | ADOPT (pipewire + a TUI mixer) | ADOPT (pipewire) |
| network | **NetworkManager** D-Bus (de facto; no unified standard) | ADOPT nmtui-class | ADOPT NM applet-class |
| bluetooth | BlueZ `org.bluez` (only real standard) | ADOPT bluetuith-class | ADOPT a BlueZ applet |
| file-handling | — | ADOPT **yazi** (Rust, MIT) | ADOPT a GUI FM |
| **policy/agent-UI** | **Shrek-native (invent)** — gatekeeperd protocol | **BUILD** (ratatui front-end) | **BUILD** (minimal Rust GUI front-end) |

**We author exactly two components:** `notifications` (Rung 1 only — Rung 2 gets it free from the
freedesktop spec) and `policy/agent-UI` (both rungs). Everything else is assembly.

**Grounding caveats from the Wayland research (carry into the build):**
- **UWSM is not in trixie *stable*** — only `trixie-backports` (0.26.4) / sid. Since we image via mkosi
  from Debian provenance we pull it into the shell layer explicitly; note the backport pin (it is not a
  stable-suite guarantee).
- **Portal fragmentation is real.** `xdg-desktop-portal-wlr` ships **no GlobalShortcuts**; only the
  Hyprland fork adds it (Hyprland-specific). Accept GlobalShortcuts as a known gap on a wlroots default,
  or pin the compositor to gain it. No single non-DE backend covers the full interface.
- **`org.freedesktop.ScreenSaver` is convention, not a ratified spec** — real idle/lock enforcement on
  Wayland routes through `ext-idle-notify-v1` + an idle daemon, not the D-Bus inhibit alone.
- **Network has no unified contract** — code the applet against NetworkManager's API for tray
  compatibility; iwd/connman are backend choices, not the contract.

## The trusted-path asymmetry (the load-bearing rule)

`policy/agent-UI` is not a swappable cosmetic role. It is the front-end of `gatekeeperd`
(roadmap Phase 4) and must be:

- **Authored in Rust, in the same cargo workspace as gatekeeperd/shrekctl/agentd.** No IPC/serialization
  seam between the daemon that *authenticates* a grant and the UI that *renders* it; shared types; one
  audit surface. This is why the answer to "Rust UI?" is *yes, for the built pieces* — structural, not
  aesthetic.
- **Un-shadowable by untrusted layers.** A sandboxed agent must not be able to draw a fake grant
  dialog. gatekeeperd owns the trusted path; the merge/image policy (Phase 2) keeps an untrusted layer
  from supplying this role.
- **One core, two front-ends.** The decision logic (gatekeeperd client + the `caps × trust ×
  semantic≤data` model) is computed **once** in audited Rust and merely *rendered* per rung: `ratatui`
  for the TTY, a minimal Rust GUI (slint/iced/gtk-rs `# VERIFY`) for Wayland.

**Security bonus:** a TTY console grant prompt is *harder* for a sandboxed agent to spoof than a Wayland
surface — the console is a more defensible trusted path. The headless rung may be where the
capability-approval UX is *most* trustworthy, not least.

## The immutable wrinkle (where config lives)

Sealed read-only `/usr` + volatile `/var` (Phase 1/2) forces a clean split:

```
role DEFAULTS            baked + signed in the layer   /usr/lib/systemd/user/*.service, /usr/share
user OVERRIDES / swaps   per-user writable             $HOME (XDG_CONFIG_HOME), or a per-user overlay
enable/disable state     per-user writable             systemctl --user state under $HOME/XDG
security-relevant config sealed / confext              never user-writable
```

"Appliance defaults" (baked) and "user swaps Rofi→Fuzzel" (writable `$HOME`) stop colliding. Anything
that could change a trust decision stays in the sealed/confext plane.

## How it rides the Onion

The shell is delivered exactly like any Phase-2 layer — nothing new to invent:

- Each rung is a **signed dm-verity sysext layer** (or set) on the untrusted layer-store, merged by
  `oniond` (Phase 4; `shrek-onion.service` today) under the baked image policy. Swapping a component =
  swapping a signed layer, **not** mutating sealed `/usr` (phase2-onion.md).
- Components run as **systemd user services** with proper `graphical-session.target` /
  session-target wiring — lifecycle, dependencies, restart, logging, enable/disable per role. This is
  what turns "some guy's dotfiles" into an actual architecture. **UWSM** (confirmed) provides exactly
  this wiring for the *compositor/session* slice: generated `wayland-session@.target` /
  `wayland-wm@.service` units + `app-graphical.slice` binding into `graphical-session.target`, D-Bus
  activation-environment handling, clean bidirectional shutdown. We adopt UWSM for that slice and
  extend the *same pattern* to the other roles it deliberately does not cover.
- AI/security integrates through **defined services** (gatekeeperd/agentd), never baked into the
  compositor.

## Prior art / where the novelty is

Research (2026-08-18) found **no existing desktop that generalizes *every* role as an independently
swappable systemd user unit with a contract layer.** The field splits two ways, neither ours:

- **UWSM** solves the *compositor/session* slice cleanly (and we adopt it) — but says nothing about
  bar, launcher, notifier, portal, lock, or greeter.
- Everything else is either **hand-wired dotfiles** (Sway/Hyprland setups with no formal role contract)
  or **monolithic consolidation** (Quickshell-family shells fuse bar+lock+notifier+launcher into one
  QML process — architecturally the *opposite* of role-decomposition). Regolith is a curated,
  opinionated Sway DE, not a decomposed framework.

So the role-decomposition framework itself is the differentiator — build it with confidence, expecting
no off-the-shelf "role manager" beyond UWSM's session slice. The corollary: `bar` and `launcher` have
**no standard contract at all** (`DEFINE` in the table), so — alongside `policy/agent-UI` — they are
where we write the contract, not adopt one.

## What we are NOT building

- **Not a compositor or a multiplexer.** Adopt zellij (TTY) / Sway-class (Wayland). Writing our own is
  the trap. Studied reference for an integrated TUI-DE: `directvt/vtm` (MIT) — clean internals but a
  monolith, so architecture reference only.
- **Not a widget toolkit / TUI framework.** ratatui (Rust) for what we author; `notcurses` (C) is the
  escape hatch *only* if a rung ever needs true in-terminal Sixel/Kitty graphics + z-order compositing —
  skip until needed.
- **Not the notification spec.** On Wayland we implement the existing `org.freedesktop.Notifications`
  contract; we only author the TTY notifier because no TTY standard exists.

## Deferred / open

- **Rung-2 grant-UI front-end toolkit** (slint vs iced vs gtk-rs) — decide when Phase 4 gatekeeperd
  lands its protocol; the core is toolkit-independent.
- **Compositor default** (Sway vs river vs a Hyprland pin) — trades the GlobalShortcuts portal gap
  against wlroots-genericity. Decide when the Wayland rung is actually built; the role model is
  compositor-agnostic either way.
- **UWSM backport pin** — track whether uwsm reaches a trixie point-release or we ride
  `trixie-backports`; a sealed-`/usr` reproducibility concern, not a design blocker.
- **Per-user layer activation** (which user may enable which shell layer) — an oniond policy question,
  not a shell question.
- **Roadmap note:** the three-rung ladder (esp. Rung 1) is an addition to Phase 10 as currently
  written; reconcile roadmap.md when this doc is accepted.
