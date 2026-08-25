# Agent launch — the Shrek `shrek-agent` dispatcher (Omarchy shape, sealed guts)

Status: **DESIGN / SPEC.** Backend it drives is built (Phase-6 slices 2–6); this is the thin
user-facing launch layer that does not yet exist. Sits on top of `shrek run` (crates/shrek) and the
sealed egress/broker spine — adds **no authority**, only a chooser + a launcher.

> Omarchy proved the launch UX can be three dumb pieces: a **flat file** naming the agent, a
> **dispatcher** that maps name → command, and a **terminal-spawn**. Shrek lifts that *shape* and
> replaces the *guts*: Omarchy launches the raw CLI with the user's own creds and blind auto-approve;
> Shrek routes a **task** through `shrek run` → the T2/gVisor wall → a broker over NAME-only egress,
> the box holding no secret. (Reference for the Omarchy mechanism: mycelium #2813.)

---

## 1. What this adds — and what it reuses verbatim

Adds three artifacts, all baked read-only into the desktop overlay (`layers/shrek-desktop/overlay`):

1. `usr/bin/shrek-agent` — the **dispatcher** (POSIX sh). Reads the chosen provider, resolves it to a
   `(egress-profile, coder --provider, model-url?)` triple, collects a task, spawns a terminal running
   `shrek run … -- coder …`.
2. `usr/bin/shrek-default-agent` — the **set-default** helper. Writes the choice to the user config
   file. (No package install step — unlike Omarchy's `mise use -g`; on a sealed image the brokers/CLI
   ride the image or the broker host, nothing installs post-boot.)
3. A sealed default config + a `/home` override merge (§4).

Reuses verbatim, unchanged: `shrek run` front door (`--project DIR --egress NAME… -- WORKLOAD`), the
coder workload (`crates/coder`), the sealed egress table (`crates/shrek-policy`), every broker
(`model-proxy`, `claude-broker`, `codex-broker`), the T2 wall, gatekeeperd egress resolution. **No Rust
changes — this is shell + config only. No system-index bump.**

## 2. The provider matrix (the core of the dispatcher)

Provider (the messages wire the coder speaks) and egress (the destination/broker) are **orthogonal**.
The coder only knows `--provider local|anthropic`; the *broker* behind the egress decides sub vs
api-key vs Codex. So the dispatcher is a table mapping a **user-facing provider id** → the real args:

| User picks   | `shrek run --egress` | `coder --provider` | Dials (sealed name)       | Broker / auth model                          | Credential |
|--------------|----------------------|--------------------|---------------------------|----------------------------------------------|------------|
| `local`      | `model-local`        | `local`            | `shrek-model:8100`        | direct LAN model (e.g. evo-x2 35B), no broker | none |
| `anthropic`  | `model-anthropic`    | `anthropic`        | `shrek-model-proxy:8200`  | `model-proxy` injects the api key + TLS       | `sk-ant-*` (broker-side) |
| `claude`     | `model-claude-cli`   | `anthropic`        | `shrek-claude-cli:8300`   | `claude-broker` shells the logged-in `claude` CLI | CLI owns it (`claude auth login`) |
| `codex`      | `model-codex-cli`    | `anthropic`        | `shrek-codex-cli` broker  | `codex-broker` shells the logged-in `codex` CLI | CLI owns it |

Note `claude`/`codex`/`anthropic` all use the SAME `--provider anthropic` wire — only the sealed egress
name differs. `local` is the only distinct wire. **Fastest path to a working coder on the MBP: `local`
pointed at evo-x2 (192.168.1.152:8100) — no credential, no cloud, no broker to stand up.**

## 3. The three pieces (mapped from Omarchy #2813)

| Omarchy | Shrek |
|---|---|
| `~/.config/omarchy/defaults/agent` (flat name) | `~/.config/shrek/agent.json` (§4) — same shape as `theme.json`/`menu.jsonc` |
| `bin/omarchy-agent` → raw CLI + `--yolo` | `usr/bin/shrek-agent` → `shrek run … -- coder --task …` (bounded, walled) |
| `bin/omarchy-default-agent` → `mise use -g` + write file | `usr/bin/shrek-default-agent` → validate provider is a known id + write file |
| `omarchy-launch-tui --app-id=…` | `foot --app-id=shrek.agent … -e shrek-agent …` (sway.config already spawns `foot`) |
| keybind `SUPER+SHIFT+CTRL+A` | a Shrek keybind in `sway.config` (§6) |

## 4. User config — flat, `/home`-writable, merged over a sealed default

Sealed default baked at `usr/share/shrek/agent/default.json` (RO `/usr`); user override at
`~/.config/shrek/agent.json` (writable `/home`), per-key merge, same convention the theme/menu systems
use. `FileView{watchChanges:true}` if a panel ever reads it.

```json
{
  "provider": "local",
  "project_root": "~/work",            // where `shrek run --project` sessions live (T2 write-through anchor)
  "last_project": null
}
```

`provider` ∈ the fixed id set in §2 (fail-closed on anything else — mirrors `shrek run`'s fail-closed
`--egress` and the coder's fail-closed `--provider`). Everything else is optional. **No secret ever
lives here** — credentials are broker-side (api key) or CLI-owned (`claude auth login`); this file only
records *which* provider, never *how to authenticate*.

**Deliberately NOT a field: the endpoint/address.** There is no `model_url` knob here. `model-local`'s
egress rule pins the sealed host `shrek-model:8100` (crates/shrek-policy); the box can reach ONLY that
name. Aiming `local` at a specific machine (e.g. the LAN 35B) is done by resolving the sealed name
`shrek-model` to that host BELOW this layer (network/name resolution / broker placement, §7) — never by
a config field up here. L5 chooses a *profile*; the address is sealed under it. (The coder still accepts
`--model`/`--model-url` for the host-side live smoke, but those are not user-facing config here.)

## 5. The dispatcher logic (`shrek-agent`)

```
shrek-agent [--pick] [--provider ID] [--project DIR] [--task "…"]
```

1. Resolve provider: `--provider` > `~/.config/shrek/agent.json` > sealed default. Fail-closed on an
   unknown id (never silently fall back to a different backend — the egress the session seals must match).
2. `--pick` → present the fixed provider list (v1: a `foot`-hosted `fzf`/`read` menu, or `dms ipc`
   spotlight later) and, on choose, call `shrek-default-agent <id>` to persist it.
3. Resolve project dir: `--project` > `last_project` > prompt under `project_root`. (`shrek run`
   REQUIRES a named `--project DIR` beneath a parent — it is the T2 write-through anchor; there is no
   "just open a shell in cwd" like Omarchy.)
4. Collect the task: `--task` or prompt one line. (The coder is a **bounded task-solver, not a REPL** —
   `coder --task` is required. An interactive/`--resume` session is Phase-6 slice-7, deferred.)
5. Look up the `(egress, --provider, model-url?)` triple from the §2 table.
6. Spawn the terminal:
   ```
   foot --app-id=shrek.agent -e \
     shrek run --project "$PROJECT" --egress "$EGRESS" -- \
       coder --provider "$WIRE" --task "$TASK" ${MODEL:+--model "$MODEL"} ${MODEL_URL:+--model-url "$MODEL_URL"}
   ```
   The coder streams its anchored markers (`CODER-STEP`/`CODER-TOOL`/`CODER-DONE`) into the terminal;
   the user watches the walled agent work and the write-through lands in `$PROJECT` on the host.

## 6. Launch entries (sway + optional DMS)

`sway.config` (`layers/shrek-desktop/overlay/usr/share/shrek/desktop/sway.config`) — add a bind next to
the existing `$mod+Return exec foot`:
```
bindsym $mod+a exec shrek-agent --pick
bindsym Mod1+a exec shrek-agent --pick
```
Optional later: a DMS spotlight/menu entry, and — the Omarchy `agents/` panel as **read-only usage
candy** (mycelium #2813: it launches nothing; it only shows per-provider usage). The launch path is
keybind/CLI-driven, exactly as in Omarchy.

## 7. The MBP-standalone knot — where do the brokers live? (OWNER DECISION)

Every broker (`model-proxy`, `claude-broker`, `codex-broker`) is deliberately **off the sealed image**
and Phase-6 slice-5 §8 explicitly deferred the standalone/headless-broker case. So for anything other
than `local`, `shrek-model-proxy` / `shrek-claude-cli` must resolve to a broker running SOMEWHERE the
sandbox can reach by that sealed name. Two clean answers:

- **(a) Homelab broker over Tailscale.** The sealed names resolve to homelab hosts (100.x). `local` is
  free today: `shrek-model:8100` → evo-x2 35B. The api-key/sub brokers run on a homelab box
  (claude-remote / brent-oneplus). Matches the built "desktop-class broker host" assumption 1:1. MBP
  needs a network link + the name→IP mapping; zero new broker work for `local`.
- **(b) Co-located broker on the MBP trusted plane.** The broker runs on the MBP host (outside the
  sandbox, in the trusted plane), the OAuth browser callback completes locally. This is the deferred
  slice-5 standalone case — needs the login UX + broker-lifecycle nailed. Best "it's *my* laptop's
  Claude sub" feel, more work.

**DECISION LOCKED (2026-08-25): (a) homelab-broker-over-Tailscale** (mycelium #2814). The stack is
already on the `theleonodor` tailnet; evo-x2 `100.x:8100` IS the `local` provider the day the Shrek host
joins; the target MBP (weak 2012 box) fits co-located brokers worse. Source-verified why this is clean:
sealed egress names resolve ONCE, HOST-SIDE, at sandbox construction (`gatekeeperd`
`resolve_profile_v4`, `net_plane.rs:244` → pins the IPv4 into the sandbox's `/etc/hosts`); the sandbox
has NO DNS/resolver (`egress.rs:20`). So **Tailscale is a pure host-side concern — only the Shrek host
joins the tailnet, the sandbox is untouched, and the sealed egress invariant does not change one bit.**
The dispatcher spec above is identical either way; only host-side name resolution changes.

Wiring Tailscale into the immutable image = the SAME proven pattern as NetworkManager/udisks2: package
`tailscaled`, persist `/var/lib/tailscale` on the writable `/home` plane, add
`/etc/polkit-1/rules.d/49-shrek-tailscale.rules` + a service drop-in for the sealed-RO write-block. UX =
mirror Omarchy `shell/plugins/panels/tailscale/Service.qml` (shell out to `tailscale
status --json`/`up`/`down`/`set --exit-node` host-side, never in a sandbox), built AFTER the plumbing.
Locked sequence: (1) `local`→evo-x2 over Tailscale [agent-launch slice, front half]; (2) Tailscale
host-plumbing slice; (3) Tailscale panel.

## 8. Sealed-OS fit

- Dispatcher + set-default + default config bake into `/usr` (RO); the only writable state is
  `~/.config/shrek/agent.json` on `/home` — same model as theme/menu. Nothing installs post-boot.
- **No blind auto-approve.** Omarchy passes `--permission-mode auto`/`--yolo`; Shrek does NOT — the
  bounded coder loop + T2 wall + grant protocol are the approval model. The dispatcher passes no
  approve-everything flag.
- Provider id is a fixed baked enum; a `/home` override may *select* a vendor provider, never inject a
  command or endpoint (except `model_url`, which is plaintext-http and only meaningful for `local`).

## 9. Built vs. new

- **Built:** `shrek run`, the coder (`--provider local|anthropic`, `--task`, `--model`, `--model-url`),
  every sealed egress profile, all three brokers, the login/health UX (slice-5).
- **New (this doc):** `shrek-agent`, `shrek-default-agent`, `usr/share/shrek/agent/default.json`, the
  sway keybind, and (later) the read-only usage panel. All shell/config → **no system-index bump**.

## 10. Proof / dogfood

Extend `image/overlay/usr/lib/shrek/dogfood-persist-probe` with an S-agent group:
- dispatcher present + executable (0755) and set-default writes/round-trips the config file;
- provider→triple mapping matches the §2 table for all four ids (assert the exact `--egress`/`--provider`
  strings) and fail-closes on an unknown id;
- the `local` path end-to-end against a canned OpenAI-wire responder (the deterministic oracle already
  used for the coder) — `shrek-agent --provider local --task …` drives a `CODER-DONE ok=true`.
The credentialed paths reuse the existing slice-3/4/5 broker oracles; the dispatcher only needs to prove
it hands them the right args.

## 11. Open questions for the owner

1. ~~**Broker placement for the MBP**~~ — RESOLVED (§7): (a) homelab-over-Tailscale (mycelium #2814).
2. **Interactive vs one-shot** — v1 is `coder --task` (bounded, walled). A REPL/`--resume` feel is
   Phase-6 slice-7 (deferred). Ship one-shot first?
3. **Picker surface** — v1 `foot`+`fzf`/`read`, upgrade to `dms ipc` spotlight later? Or wire the menu
   engine (docs/omarchy-portability.md Appendix) and make agent-pick one of its provider-backed submenus?
4. **`local` endpoint** — hardcode evo-x2 as the default `model_url`, or leave `shrek-model` name
   resolution to the network layer and keep the config endpoint-agnostic?

---

*Companion to docs/phase6-slice3-provider-abstraction.md (the wire seam),
docs/phase6-slice4-claude-cli-broker.md + slice5-claude-login-ux.md (the sub broker + login),
docs/omarchy-portability.md (the shape lifted). Mechanism source: mycelium #2813.*
