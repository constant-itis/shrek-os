# Agent launch — the Shrek `shrek-agent` dispatcher (Omarchy shape, sealed guts)

Status: **DESIGN / SPEC.** Backend it drives is built (Phase-6 slices 2–6); this is the thin
user-facing launch layer that does not yet exist. Sits on top of `shrek run` (crates/shrek) and the
sealed egress/broker spine — adds **no authority**, only a chooser + a launcher.

> Omarchy proved the launch UX can be three dumb pieces: a **flat file** naming the agent, a
> **dispatcher** that maps name → command, and a **terminal-spawn**. Shrek lifts that *shape* and
> replaces the *guts*: Omarchy launches the raw CLI with the user's own creds and blind auto-approve;
> Shrek routes a **task** through `shrek run` → the T2/gVisor wall → a broker over NAME-only egress,
> the box holding no secret. (The mechanism is lifted from Omarchy: basecamp/omarchy, MIT.)

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
| `local`      | `model-local`        | `local`            | `shrek-model:8100`        | direct LAN model (e.g. a local 35B), no broker | none |
| `anthropic`  | `model-anthropic`    | `anthropic`        | `shrek-model-proxy:8200`  | `model-proxy` injects the api key + TLS       | `sk-ant-*` (broker-side) |
| `claude`     | `model-claude-cli`   | `anthropic`        | `shrek-claude-cli:8300`   | `claude-broker` shells the logged-in `claude` CLI | CLI owns it (`claude auth login`) |
| `codex`      | `model-codex-cli`    | `anthropic`        | `shrek-codex-cli` broker  | `codex-broker` shells the logged-in `codex` CLI | CLI owns it |

Note `claude`/`codex`/`anthropic` all use the SAME `--provider anthropic` wire — only the sealed egress
name differs. `local` is the only distinct wire. **Fastest path to a working coder on the MBP: `local`
pointed at a LAN model server (e.g. a host on `:8100`) — no credential, no cloud, no broker to stand up.**

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
candy** (it launches nothing; it only shows per-provider usage). The launch path is
keybind/CLI-driven, exactly as in Omarchy.

## 7. Where do the brokers live? — ships with NO model, provides a way to hook one up

Every broker (`model-proxy`, `claude-broker`, `codex-broker`) is deliberately **off the sealed image**,
and `local` needs a model server somewhere too. So each sealed name (`shrek-model`, `shrek-model-proxy`,
`shrek-claude-cli`, `shrek-codex-cli`) must resolve to a backend the host can reach *by that name*. The
decisive constraint is that **Shrek OS is universal** — one image ships to anyone, so it cannot bake in
*any* address (a LAN host, a homelab, a tailnet). **The image ships with NO model wired; it ships a way to
hook one up.**

**The hook-up (`shrek-connect`).** The sealed image carries the *names* (L0) and the dispatch UX (L4);
it carries no address for any of them. `shrek-connect <provider> <addr>` binds a sealed name to an
address *you* choose — a LAN IP, a tailnet `100.x`, a hostname — writing one `/etc/hosts`-format line
into a per-install store on the writable `/home` plane. That is the whole "site config": the seal fixes
the *name*, the dispatcher picks the *choice*, and only this layer holds the *address*.

**Why this is universal and clean (source-verified, preserves the #2814 structural fact).** Sealed
egress names resolve ONCE, HOST-SIDE, at sandbox construction — `gatekeeperd` `resolve_profile_v4`
(`net_plane.rs:244`) runs a plain glibc `getaddrinfo` (`to_socket_addrs`, line 263) and pins the IPv4
into the sandbox's `/etc/hosts` (`t2_plane.rs:598`); the sandbox itself has NO DNS/resolver
(`egress.rs:20`). So name→address is a **pure host-side concern**, the sandbox is untouched, and the
sealed egress invariant does not change one bit. The only question is what the *host's* `getaddrinfo`
reads — and the answer is nss `files`, i.e. `/etc/hosts`:

- `/etc/hosts` is a **baked symlink → `/run/shrek/hosts`** (`image/overlay/etc/hosts`) — the exact
  "mutable `/etc` file → writable plane" idiom the base uses for `/etc/resolv.conf` → `/run`. As of
  **ADR-008 (#3121 fix)** that projection is **ROOT-authored**: `getaddrinfo` (and thus gatekeeperd)
  reads a file uid 1000 cannot write. `shrek-connect` no longer edits it directly — it sends a narrow,
  closed-token `egressd ask bind <provider> <ipv4>` over the supervisor socket, and root composes
  `/run/shrek/hosts` from a sealed baseline + the owner's root-owned bindings.
- **Not MagicDNS.** The egress hosts are sealed-policy *aliases*, not tailnet device names
  (`egress.rs`: "`shrek-model` is the sealed, stable" name), so MagicDNS can't resolve them without
  per-user Tailscale-console records — extra surface that couples a compiled-in alias to mutable console
  state. nss `files` needs zero resolver wiring and is deterministic; the pin is as sealed as the name.
- The image has no `libnss-myhostname`, so `shrek-hosts-compose.service` (a base oneshot ordered like
  `var-lib-swamp.mount`: after `home.mount`, before `local-fs.target`/`nss-lookup.target`) composes
  `/run/shrek/hosts` from a **sealed-in-code `localhost` baseline** plus the owner's bindings on every
  boot — UNCONDITIONALLY, so a fresh, un-hooked-up box resolves `localhost` even before any binding store
  exists. Un-hooked-up model names simply don't resolve → `shrek-agent` catches it pre-flight (reading the
  root-owned projection) and prints a "hook one up" hint (never a bare fail-closed). The binding address
  must be an **IPv4 literal** (glibc `files` needs a literal; a hostname line is silently skipped).

**Broker placement is now the user's call, per install, and orthogonal to the image:**
- **`local`** — point it at any OpenAI-wire model server you can reach: `shrek-connect local
  <lan-ip>` (LAN) or a tailnet address. No credential, no broker. This is the loop-proving path.
- **Credentialed (`anthropic`/`claude`/`codex`)** — run the broker somewhere reachable (a homelab box,
  or later a co-located broker on the trusted plane, the deferred slice-5 standalone case) and
  `shrek-connect` that provider at it. The dispatcher is identical either way; only the address differs.

**Tailscale is an OPTIONAL transport, not part of this base.** A LAN LLM needs no tailnet at all. Only
when you hook up a *remote* brain by `100.x` do you need the host on a tailnet — a separate, optional
slice (package `tailscaled`, persist `/var/lib/tailscale` on `/home`, polkit rule + service drop-in —
the proven NetworkManager/udisks2 pattern; UX mirrors Omarchy's `tailscale` Service.qml). It is
turned on when hooking up a remote brain, never assumed by the image.

## 8. Sealed-OS fit

- Dispatcher + set-default bake into `/usr` (RO); the writable state is the provider *choice*
  (`~/.config/shrek/agent.json`) and the name→address *bindings* (the root-owned
  `/home/.shrek-system/hosts-bindings` store, projected to `/run/shrek/hosts` which `/etc/hosts` symlinks
  to; ADR-008) — the provider *choice* on `/home`, the bindings root-mediated. Nothing installs post-boot.
- **No blind auto-approve.** Omarchy passes `--permission-mode auto`/`--yolo`; Shrek does NOT — the
  bounded coder loop + T2 wall + grant protocol are the approval model. The dispatcher passes no
  approve-everything flag.
- Provider id is a fixed baked enum; a `/home` override may *select* a vendor provider, never inject a
  command. The *address* is never in config or a flag — it lives only in the sealed-name binding
  (`shrek-connect`), which gatekeeperd resolves host-side and the sandbox never sees.

## 9. Built vs. new

- **Built:** `shrek run`, the coder (`--provider local|anthropic`, `--task`, `--model`, `--model-url`),
  every sealed egress profile, all three brokers, the login/health UX (slice-5).
- **New (this doc):** `shrek-agent`, `shrek-default-agent`, the sway keybind (all L4/L5, shipped); and
  the name-resolution layer — `shrek-connect` (the hook-up, now root-mediated over the egressd socket),
  the baked `/etc/hosts` → `/run/shrek/hosts` symlink, `shrek-hosts-compose.service`, and the egressd
  `bind`/`unbind` verb (ADR-008, #3121 — this part is Rust and DOES bump the system-index); and (later)
  the read-only usage panel.

## 10. Proof / dogfood

`image/overlay/usr/lib/shrek/dogfood-persist-probe` carries an **S-agent** group (dispatcher present +
executable, provider→triple map matches the §2 table for all four ids, fail-closes on an unknown id, and
the L5 default round-trips) and an **S-connect** group for the name-resolution layer:
- `shrek-connect` present; its provider→sealed-host-name map matches the L0 policy 1:1;
- a REAL bind/list/forget round-trips a correct line into the root-owned `/run/shrek/hosts` projection
  (via the live egressd supervisor socket — ADR-008);
- `shrek-agent` fail-closes (exit 3) with the "hook one up" hint when the chosen provider is UNbound;
- the real image wiring is in place: `/etc/hosts` is the baked symlink to `/run/shrek/hosts`, and
  `shrek-hosts-compose.service` ran on boot (so `localhost` resolves on a fresh, un-hooked-up box).
A bound real launch (`local` end-to-end against a canned OpenAI-wire responder → `CODER-DONE ok=true`)
is the host-side coder oracle's job (slice-3 §5); the credentialed paths reuse the slice-3/4/5 broker
oracles. The dispatcher only needs to prove it hands them the right args over a resolvable name.

## 11. Open questions for the owner

1. ~~**Broker placement for the MBP**~~ — RESOLVED (§7): (a) homelab-over-Tailscale.
2. **Interactive vs one-shot** — v1 is `coder --task` (bounded, walled). A REPL/`--resume` feel is
   Phase-6 slice-7 (deferred). Ship one-shot first?
3. **Picker surface** — v1 `foot`+`fzf`/`read`, upgrade to `dms ipc` spotlight later? Or wire the menu
   engine (the Omarchy-derived menu surface) and make agent-pick one of its provider-backed submenus?
4. ~~**`local` endpoint**~~ — RESOLVED (§7): the image is endpoint-agnostic and bakes NO address; the
   user binds `shrek-model` (and any broker) per-install with `shrek-connect`, on the writable `/home`
   plane. Ships with no model wired.

---

*Companion to docs/phase6-slice3-provider-abstraction.md (the wire seam),
docs/phase6-slice4-claude-cli-broker.md + slice5-claude-login-ux.md (the sub broker + login),
and Omarchy (basecamp/omarchy, MIT — the menu shape lifted).*
