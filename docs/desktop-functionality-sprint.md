# Desktop Functionality Sprint — make DMS actually work

DMS 1.5.3 (DankMaterialShell, `dms=1.5.3db1` from the AvengeMedia OBS repo) ships a full shell — bar,
control center, and dashboard tabs (overview / media / wallpaper / weather / settings). The sealed
`shrek-desktop` layer wired *some* of the backends DMS expects but not others, so several widgets render
but do nothing (the shutdown button being the one that surfaced this). Each surface talks a standard
freedesktop/Wayland contract; the work is wiring the backend the sealed base never installed.

This is a series of small, individually-verifiable slices. Each slice = add package(s)/config/unit to
`layers/shrek-desktop/mkosi.conf` (+ maybe a DMS setting or a systemd unit enable) → rebuild →
boot-verify → owner-split commit. All are package/config/unit only → **no system-index graph bump**
(the graph is Rust-AST only), except **S5** (stages a binary) and possibly **S3** (may need a Quickshell
recompile).

Deploy loop: `scripts/build-desktop-layer.sh` → `scripts/build-layers.sh desktop` → boot
(`scripts/dogfood-vm.sh` headless PASS oracle, or the daily `shrek-dogfood` domain in virt-manager —
runbook memory #2710). A guest reboot does **not** re-read the layer store; do a full power-off + start.

## Backend status (from the layer package list, 2026-08-24)

Present & expected to work (confirm on the S0 triage boot): audio (pipewire / wireplumber /
pipewire-pulse), Bluetooth (bluez), battery (upower), notifications (Quickshell), tray, screenshot +
clipboard-copy (grim / slurp / wl-clipboard), portals (xdg-desktop-portal + -gtk + -wlr), terminal
(foot), icons (papirus), dconf.

Confirmed missing → dead widgets:

| Dead surface | Missing backend |
|---|---|
| Shutdown / reboot / suspend / logout | `polkit` |
| Wi-Fi / network control center | NetworkManager (may live in the base image — verify) + polkit |
| Lock screen + auto-lock / idle | session-lock backend + `swayidle` + logind idle + PAM auth |
| USB / disk mounting | `udisks2` (+ polkit) |
| Wallpaper → dynamic recolor | `matugen` binary (config references it; binary not staged) |
| Brightness slider | `brightnessctl` (real hardware only) |
| Power-profile switching | `power-profiles-daemon` |
| Weather tab | network egress + a weather API (+ a sealed-egress policy decision) |

## Slices

### Phase A — ground + core

- [ ] **S0 · Triage boot** *(½ loop)* — boot the installed image, click **every** DMS surface, record
  works vs dead. Grounds the sprint in reality instead of package inference. Update the table above with
  findings. Do this first.
- [ ] **S1 · Power / session (polkit)** *(S)* — the reported bug and a category-opener (also unlocks the
  auth half of udisks and NetworkManager). Add `polkitd` (`polkit-1`/`policykit-1`) to the desktop
  layer. The systemd-shipped logind `.policy` files default `allow_active=yes` for
  poweroff/reboot/suspend, so `polkitd` alone likely suffices with **no** graphical agent — verify.
  Confirm `/usr/share/polkit-1/actions/org.freedesktop.login1.policy` is present and the `dev` session
  is `Active=yes` on seat0 (`loginctl session-status`; M1 made it active via pam_systemd).
  **Accept:** Off powers off, Reboot reboots, Suspend suspends.
- [ ] **S2 · Network (NetworkManager)** *(S–M)* — verify NM is running (it may already be in the base
  image; prior boots logged NetworkManager). Wire the DMS network center and **persist connection
  profiles** — volatile `/var` means NM profiles need a `/home`-backed bind (the M1 selective-persistence
  pattern). **Accept:** lists / toggles / connects. Wi-Fi is only fully testable on real hardware;
  the virtio NIC + NM state should still show in the VM.

### Phase B — daily-driver

- [ ] **S3 · Lock + idle** *(M)* — verify whether DMS's packaged Quickshell already has the SESSION_LOCK
  backend (recompile only if not) + `swayidle` + logind idle→lock + lock-on-suspend. **Knot:** the lock
  screen authenticates the `dev` user, so this needs a working **PAM** path. **Accept:** manual lock +
  auto-lock on idle + resume requires unlock.
- [ ] **S4 · Storage / mounts (udisks2)** *(S — nearly free after S1)* — plug a disk → it mounts →
  browsable. VM-testable by attaching a spare virtio disk. **Accept:** attach → mount → open.
- [ ] **S5 · Dynamic theming (matugen)** *(M)* — stage the `matugen` static binary into
  `layers/shrek-desktop/overlay/usr/bin` (same pattern as the staged `quickshell` binary), flip
  `runUserMatugenTemplates` in the DMS settings. **Biggest visual payoff** — closing the wallpaper →
  palette → recolor loop. **Accept:** pick a wallpaper → the whole shell recolors.

### Phase C — real-hardware / needs a decision (lower priority)

- [ ] **S6 · Brightness + power-profiles** *(S)* — `brightnessctl` + `power-profiles-daemon`. No-op in a
  VM; matters on the real Mac hardware.
- [ ] **S7 · Weather + shell egress** *(M + decision)* — the weather tab needs the shell to reach a
  network API, which touches the sealed-egress security model. Defer until we decide whether the shell
  gets a named egress, or just disable the weather tab for now.

## Notes / constraints

- Sealed RO `/usr` + volatile `/var`: daemons and unit enables bake into `/usr` (fine); anything that
  must **survive reboot** (NM connections, etc.) needs an explicit persistent bind from `/home`, per the
  M1 selective-persistence model — do **not** convert the whole `/var` plane to persistent.
- Shell standardization: the running shell is **DMS** (`sway.config` `exec dms run`), not the vestigial
  `ui/` custom Quickshell shell. Wire DMS; the `ui/` tree can be cleaned out once confirmed.
- Rough shape: Phase A ≈ 2–3 deploy loops → power + network work. Phase B is the "feels finished" phase.
  Phase C is optional / hardware-gated.
