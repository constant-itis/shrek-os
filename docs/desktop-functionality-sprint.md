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
- [x] **S1 · Power / session (polkit)** *(S)* — **DONE** (dogfood-verified + interactive: Off powers off).
  `polkitd` alone was **not** enough — the sealed image breaks packaged-daemon integration in three
  places, all now handled (see the build-model reference in mycelium): (1) `polkit.service` runs
  `User=polkitd`, but a layer's `sysusers.d` applies after `systemd-sysusers` and runtime `/etc` is a
  read-only confext overlay, so the user is **baked into the base `/etc`** at build (`image/mkosi.postinst`,
  uid/gid 701) alongside the `/etc/polkit-1/rules.d` dir; (2) the layer's `polkit.service` + dbus
  own-name policy merge *after* systemd/dbus read config and the merge issues no reload, so
  `shrek-desktop-polkit.service` does a post-merge `daemon-reload` + dbus reload + `systemctl start
  polkit.service` so polkitd **owns its bus name** up front (dbus on-demand activation alone fails); (3)
  the login1 `.policy` `allow_active=yes` then grants an **active seat0** session with no graphical
  agent. The dogfood oracle now proves the whole chain (user → unit active → owns name → `pkcheck`
  grants power-off/reboot/suspend to the session leader).
  **Accept:** Off powers off, Reboot reboots, Suspend suspends. ✓
- [x] **S2 · Network (NetworkManager)** *(S–M)* — **DONE** (dogfood-verified, PASS=29/0). NM is a **base**
  daemon (`network-manager` in `image/mkosi.conf`), already active + connected in the VM (M4). For the
  active seat0 user NM already defaults `allow_active=yes` for list / toggle / scan / connect
  (`network-control`, `enable-disable-{network,wifi}`, `wifi.scan`, `settings.modify.own`); the sealed
  image broke only two things, both now handled: (1) **authorization** — saving a *system* connection
  (`settings.modify.system`) defaults `auth_admin_keep`, an admin prompt the sealed single-user session
  has no graphical polkit agent to answer, so Wi-Fi passwords silently failed to save. A baked rule
  `/etc/polkit-1/rules.d/49-shrek-nm.rules` (`image/mkosi.postinst`, read-only `/etc` so baked at build)
  grants that one action to the active+local session. (2) **persistence** — read-only `/etc` means the
  keyfile store is redirected to `/home` (`20-shrek-persistent-keyfile.conf`), but `NetworkManager.service`
  ships `ProtectHome=read-only`, which mounts `/home` read-only *inside the daemon namespace* even for
  root — so the redirected store silently no-op'd on write. A drop-in
  (`NetworkManager.service.d/10-shrek-home-keyfile.conf`) adds `ReadWritePaths=` for just the keyfile
  store (rest of `/home` stays read-only to NM). The dogfood oracle now proves the whole chain: keyfile
  path on `/home` → polkit grants `settings.modify.system` to the seat0 leader → a saved system
  connection lands in the persistent `/home` store. **Accept:** lists / toggles / connects + profiles
  persist. ✓ (Wi-Fi is only fully testable on real hardware; the virtio NIC + NM state show in the VM.)

### Phase B — daily-driver

- [ ] **S3 · Lock + idle** *(M)* — verify whether DMS's packaged Quickshell already has the SESSION_LOCK
  backend (recompile only if not) + `swayidle` + logind idle→lock + lock-on-suspend. **Knot:** the lock
  screen authenticates the `dev` user, so this needs a working **PAM** path. **Accept:** manual lock +
  auto-lock on idle + resume requires unlock.
- [x] **S4 · Storage / mounts (udisks2)** *(S — nearly free after S1)* — **DONE** (dogfood-verified,
  PASS=32/0). udisks2 added to the **base** (not a layer): it has an `/etc` footprint (`udisks2.conf`)
  and a D-Bus-activated service, both integrated natively at base build — no sysext post-merge dance.
  Two things needed handling: (1) **authorization** — removable-media actions default `allow_active=yes`,
  but a NON-removable disk (any internal disk, and the virtio test disk) is a "system" device whose
  `filesystem-mount-system` defaults `auth_admin_keep` (no graphical agent in the sealed session). A baked
  rule `/etc/polkit-1/rules.d/49-shrek-udisks.rules` grants the `*-system` mount/unlock/eject/power-off
  actions to the active+local session. (2) **writable mount base** — udisks uses `/run/media/$USER` for
  session callers but falls back to `/media/$USER` for session-less ones (a root/service mount), and
  `/media` is on the read-only dm-verity root → `mkdir /media/<user>` failed. A `media.mount` tmpfs unit
  (always-enabled) gives `/media` a writable base, same sealed-root pattern as volatile `/var`. The
  dogfood oracle attaches a labelled `SHREKUSB` virtio disk and proves the chain: udisksd owns its bus
  name → polkit grants `filesystem-mount-system` to the seat0 leader → the disk mounts via udisks and its
  seeded marker reads back. **Accept:** attach → mount → open. ✓
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
