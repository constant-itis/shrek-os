# ADR-008 — Root-authoritative name resolution & the owner's bounded provider-bind

Status: ACCEPTED (owner, 2026-09-04) after 2 Fable rounds (R1 GO-WITH-FIXES 8 MF; R2 GO-WITH-FIXES 1 MF,
§3 hold released). All fixes folded with inline `[R1-MFn]`/`[R2-MFn]` markers.
Author: officialbubies <thegambinogold@gmail.com>
Supersedes the uid-1000-owned `/etc/hosts` design shipped by the agent-launch slice (#2816).
Closes mycelium #3121 (filed by ADR-007 Fable round-1 as MF-1).

## 1. Context

### 1a. The defect (verified live at `9155b69`) — Fable-confirmed, keep as-is

`/etc/hosts` is a baked symlink → `/home/.shrek-system/hosts`. `image/overlay/usr/lib/shrek/hosts-seed`
(run every boot by `shrek-hosts-seed.service`) does `chown 1000:1000` on **both** the store directory and
the `hosts` file. So the file every root daemon's `getaddrinfo` reads is **owned by uid 1000**.

glibc NSS resolves `hosts:` with `files` before `dns` (base default — there is **no** `nsswitch.conf`
override in the image). So any root daemon calling `getaddrinfo` reads `/etc/hosts` **directly, before
`systemd-resolved` is ever consulted.** uid 1000 writes `<attacker-ip> <any-hostname>` and steers:

- **NTP** → clock control (partially closed: ADR-007 S5 sealed `NTP=` to literal Cloudflare IPs).
- **TLS trust** via poisoned OCSP/CRL/CA hostnames.
- **apt**, any root `curl`, and **gatekeeperd's own agent-sandbox pinning** (`net_plane.rs:280`).

**Why ADR-007's `ReadEtcHosts=no` did NOT close this host-wide:** that drop-in only stops
`systemd-resolved` from consulting `/etc/hosts`. A glibc `getaddrinfo` caller with `files` first reads
`/etc/hosts` before resolved is in the path at all.

### 1b. The second path — NM polkit (`image/mkosi.postinst` `49-shrek-nm.rules`)

The grant of `NetworkManager.settings.modify.system` to the active seat0 session (added so Wi-Fi
passwords persist) also lets uid 1000 edit a **system** connection's `ipv4.dns`/`ipv6.dns` — an
independent route to steering root name resolution via resolved's upstream DNS. **[R1-MF7]** this is not
optional to fix: gatekeeperd pins **public** profile names (`github.com`, `deb.debian.org`, `pypi.org`,
`crates.io` — `egress.rs`) through the same `getaddrinfo` path; those fall through NSS `files` to `dns`
= resolved's upstream. So a DNS poison via the polkit grant poisons **root gatekeeperd's public-profile
pinning even after the `/etc/hosts` fix.** §9 is therefore an in-scope closure gate, not a companion.

### 1c. The legitimate need the fix MUST preserve — corrected per [R1-MF2]

`shrek-connect <provider> <addr>` binds one of the 4 sealed model-provider names to an owner-chosen
address; gatekeeperd resolves that name via `getaddrinfo` and pins the IPv4 into the agent sandbox. The
system grants no passwordless root, so the owner must set the binding **without sudo**.

**[R1-MF2] the address is IPv4-literal-only — hostnames never worked.** glibc's NSS `files` parser
requires an **IP literal** in the address column; a `/etc/hosts` line `myhost.lan shrek-model` is
**silently skipped** and the name never resolves. Today's `shrek-connect` header (`shrek-connect:16-21`)
and its `valid_addr` (which permits letters and `:`) advertise "a LAN IP, a tailnet address, a hostname"
— the hostname path is a **latent bug**: it reports "connected" and then fails-closed at launch with a
bare "no A record". ADR-008 fixes this: `addr` is a strict IPv4 dotted-quad, canonically re-rendered.

### 1d. What the ADR asserts about existing code (audited)

Confirmed against source: the 4 model brokers are plaintext, server-unauthenticated legs —
`shrek-model` tcp:8100 (LAN model), `shrek-model-proxy` tcp:8200 (key lives in the broker, not the box),
`shrek-claude-cli` tcp:8300 / `shrek-codex-cli` tcp:8301 (the CLIs own their own OAuth; no credential
enters the box). A **5th** sealed name `shrek-swamp-broker` tcp:8400 exists (`egress.rs:198`,
`SWAMP_QUERY_HOST`) — the no-SNAT identity-preserving host (`net_plane.rs:249 no_masquerade_ips`); it is
**not** a model provider and **not** in `shrek-connect`'s set. See §3 for its treatment.

## 2. Decision (summary)

1. **Root owns `/etc/hosts` composition.** `/etc/hosts` retargets to a **root-owned** `/run/shrek/hosts`
   (the `/etc/…`→`/run` idiom already used for `resolv.conf` and egressd's `/run/shrek/egress/*`).
   Composed by root from (a) a **sealed baseline in code** (`localhost`, and `shrek-swamp-broker`'s fixed
   local address if swamp-query ships — §3) plus (b) the owner's **blessed provider bindings**.
   `localhost` no longer lives in a uid-1000-writable file.

2. **The owner's write path becomes a narrow, closed-token IPC verb on egressd.** New uid-1000 verbs:
   `bind <provider-token> <addr>` and `unbind <provider-token>`. `provider-token` ∈ the **closed set**
   `{local, anthropic, claude, codex}`, mapped to sealed names **server-side** in `shrek-policy` (never a
   free hostname from the client). `addr` is the only free field, an **IPv4 literal** validated and
   canonically re-rendered server-side. The owner has **no write access to any file NSS reads**.

3. **The persistent binding store is root-owned**, token-format, at a **new path**
   `/home/.shrek-system/hosts-bindings` (**[R1-MF6]/Q5** — a new path, not the legacy `hosts`, so the
   format break is explicit and rollback stays sane). Dir `root:root 0700` (owned by `tmpfiles.d`), file
   `root:root 0600` (content owned by the compose oneshot) — **[R1-MF4]/N-1**.

4. **Remove resolved from gatekeeperd's privileged pin path (§9, in-scope [R1-MF7], OPTION 4 — owner,
   supersedes posture A):** do NOT seal/cripple NM+resolved DNS (that would harden the owner-controlled
   resolver — the MF-1/§3 anti-pattern). Instead gatekeeperd resolves public egress pins over the shared
   sealed-DoT crate (`shrek-dot`), never resolved; NM/DHCP DNS stay functional for user resolution. Own
   host oracle. (The uid-1000-edits-system-DNS *privilege* question is filed separately, #3157.)

## 3. The trust boundary (the load-bearing part) — rewritten per [R1-MF1]

**The honest mechanism (the earlier draft's §3 was false and is retracted):** the 4 model names ARE
resolved by a root daemon — gatekeeperd, `net_plane.rs:261 resolve_profile_v4` → `:280 to_socket_addrs`
— and each resolved IP becomes an nft **allow endpoint** in the per-sandbox ruleset (`:284-290` →
`create_and_inject`). So a uid-1000-chosen address **does** flow into a root-constructed firewall
allowlist. The claim "no root daemon looks these up / opens no firewall hole" was wrong.

The design is sound on the TRUE grounds:

**Authority A — system name resolution (root's).** localhost + system hostnames + all **public** profile
names (github/debian/pypi/crates) + `shrek-swamp-broker`. uid 1000 contributes **zero** here. Baseline is
sealed-in-code; public names resolve via DNS whose upstream §9 seals.

**Authority B — the agent's own model endpoint (the owner's).** Expressed only as the IPv4 of the 4
sealed model names. When gatekeeperd pins one, the resulting allow is:
- **scoped to the per-sandbox table**, at the **sealed proto:port** for that name (tcp 8100/8200/8300/
  8301 — `egress.rs`), never the host or desktop egress plane;
- over a session that is **already uid-1000's own authority** (it drives that agent).

What the fix DOES bound, precisely: uid 1000 gains **no stored credential** (none transits the box —
§1d), **no new reachability** (the nft aperture is still one sealed port to one pinned IP), and **nothing
against root** (root resolves from a file uid 1000 cannot write). What it does NOT eliminate — and the
earlier draft wrongly hand-waved as "content uid 1000 already holds" — is a **real redirect-MITM of the
plaintext, server-unauthenticated sandbox↔broker leg**: binding a model name to an attacker box lets that
box **read the model responses** and, worse, **inject forged model output back into the agent session**
(an integrity attack — steer the agent, not merely observe it). uid 1000 holding the *prompt* does not
make that free: the responses and the bidirectional channel are new surface. This is a genuine gap, not
an accepted triviality.

**Residual R-1 — RESOLVED (owner, filed as tracked follow-up):** the broker leg is plaintext +
server-unauthenticated by the existing egress design (brokers trusted-by-placement). ADR-008 makes the
leg's endpoint uid-1000-choosable, which turns that latent property into a reachable redirect-MITM
primitive. Owner decision: **do NOT accept it as trivial** — file broker-leg server-authentication
(pinned server identity / mTLS on the sandbox↔broker hop) as a real follow-up security item, kept OUT of
ADR-008's scope (§11) so #3121 stays focused, but tracked, not dismissed.

**`shrek-swamp-broker` [R1-MF3]:** deliberately **excluded** from the bindable token set — if it were
bindable, uid 1000 could steer the **un-masqueraded, identity-carrying** destination (a strictly worse
primitive: it escapes SNAT, `net_plane.rs:243`). Its address is a **fixed local bridge**, not an
owner choice, so it belongs in the **sealed baseline**, not the owner store. **R-2 — RESOLVED (owner):
swamp-query is bench-only**, not wired on shipped/dogfood boxes (only `swamp-broker-find-proof.sh` appends
the line by hand), so the **v1 sealed baseline omits it** (localhost only). Migration strips any legacy
`shrek-swamp-broker` line (§7) — intended. If swamp-query later ships, add its fixed local address to the
baseline then (it stays non-bindable regardless).

## 4. The hosts store & projection

- **Persistent store:** `/home/.shrek-system/hosts-bindings` **[R1-MF6]**. Dir `root:root 0700` declared
  by `tmpfiles.d/shrek-home.conf` (the single ownership authority — **[R1-MF4]**, the compose oneshot
  never chowns the dir). File `root:root 0600` **[N-1]**, line format `<provider-token> <addr>` (tokens,
  not resolvable names — a store corruption can name only the 4 sealed providers). Written atomically
  (tmp+rename inside the 0700 dir) by egressd only; both writers take `store::lock_store` **[N-2]**.
- **Projection:** `/run/shrek/hosts` **root:root 0644**, real hosts syntax:
  ```
  127.0.0.1 localhost
  ::1 localhost
  <ipv4> shrek-model            # only if `local` is bound
  … one line per bound provider (token→sealed-name mapped at compose time)
  # shrek-swamp-broker NOT in v1 baseline — swamp-query is bench-only (R-2)
  ```
  Composed atomically, IPv4 literals canonically re-rendered **[R1-MF2]**. World-readable; writable by
  root only.
- **`/etc/hosts`** baked symlink retargets → `/run/shrek/hosts` (was `/home/.shrek-system/hosts`).
- **localhost** and any fixed baseline names come from a **`const` in code**, never a writable file — a
  fresh box resolves localhost and uid 1000 cannot break it.

## 5. The protocol extension (the reviewable grammar change)

egressd's invariant (`supervisor.rs:83`): uid 1000 may express only verb + profile-token; a 3rd field is
a hard parse error. `bind` relaxes this for **one branch only**:

- New `Request` variants `Bind(token, addr)`, `Unbind(token)`; **not** `is_privileged` (the root-only
  `confirmed-*` ceremony verbs stay a disjoint set).
- Parser: a dedicated `bind` branch accepts exactly **verb + provider-token + addr** (3 tokens); a **4th**
  field is a hard error there too. Every other verb keeps the `b.is_some() → "too many fields"` default
  (`supervisor.rs:171`). `unbind` = verb + provider-token. **[R1-MF8b]** add abuse tests mirroring
  `parse_rejects_abuse` (3rd field on non-bind verbs, 4th field on bind, control chars, non-token).
- `provider-token` ∈ `{local, anthropic, claude, codex}` from the `shrek-policy` table `shrek-agent`'s
  `host_for` mirrors (single source of truth; dogfood cross-check catches drift).
- `addr` = strict IPv4 dotted-quad via `Ipv4Addr::from_str`, **re-rendered canonically** into store +
  projection (kills inet_aton hex/octal/short-form parse-differentials) **[R1-MF2]**. IPv6 rejected
  (gatekeeperd fail-closes on no-A-record, `net_plane.rs:301` — a v6 bind is a self-DoS).
- **No `REQ_MAX` bump [R1-MF2]:** `bind anthropic 255.255.255.255\n` ≈ 31 bytes ≪ the 128-byte uid-1000
  cap. `client.rs LINE_MAX = 128` stays paired with `REQ_MAX`; pin the invariant **[R1-MF8a]**.
- Reuses the existing **SO_PEERCRED** identity gate, **rate limiter** (a bind flood exhausts the same
  6/30s window — no oracle; note onboarding's `bless weather` + a bind share it, **[N-4]** fine), and
  audit. **[R1-MF8c]** `append_event` (`supervisor.rs:350`, fields `at verb profile result`) has no addr
  slot — an audit of a bind that omits the bound address is not an audit; carry the addr in the `result`
  field (whitespace/control-free by grammar). Journal likewise.
- `authorize()`: `bind`/`unbind` allowed for `peer_uid == desktop_uid`, denied otherwise (identity gate;
  no ceremony tier — they carry no firewall-plane authority, §3). **[N-4]** `unbind` of an unbound token
  → idempotent `OK`.

## 6. Boot & ordering, layer placement — answers Q2 per [N-2]

`egressd` ships in the **base** overlay (`build-in-container.sh` installs to
`image/overlay/usr/libexec/shrek/`), so a base oneshot can call `egressd compose-hosts` in **every**
variant — provided that subcommand touches **no nft** and needs **no CAP_NET_ADMIN** (pure store-read +
`/run` compose). One composer, two callers (boot oneshot + live `bind`/`unbind`), both taking
`store::lock_store`.

- **`shrek-hosts-compose.service`** (base, replaces `shrek-hosts-seed.service`): `ExecStart=egressd
  compose-hosts`. Ordering carried over verbatim **[N-5]**: `DefaultDependencies=no`,
  `After=home.mount`, `Before=local-fs.target nss-lookup.target`, `Conflicts=umount.target`,
  `Before=umount.target`. Composes `/run/shrek/hosts` before anything resolves names.
- **[R2-MF1] first-boot ordering — the projection NEVER depends on the store existing.** `compose-hosts`
  runs `Before=local-fs.target`, but the dir-ownership authority (`tmpfiles.d`) runs *after*
  `local-fs.target` — so on a virgin disk `/home/.shrek-system` does not yet exist when the oneshot runs.
  Therefore `compose-hosts` **always** composes and atomically installs `/run/shrek/hosts` from the sealed
  baseline **plus whatever bindings are readable** — store absence/unreadability can **never** block the
  projection (this is what the old seed's `mkdir -p` implicitly guaranteed for localhost). It **may**
  `mkdir -p` the store dir at the root default (NO chown/chmod — tmpfiles reconciles ownership later the
  same boot) before persisting a binding. localhost is guaranteed every boot regardless of store state.
- Live mutations run in egressd (desktop plane) and recompose via the same routine — the boot oneshot and
  the daemon never both write `/run/shrek/hosts` concurrently (lock_store + atomic rename).

## 7. Migration / retrofit (A/B-safe) — hardened per [R1-MF4/MF5/MF6]

A pre-fix box has a **uid-1000-owned** `/home/.shrek-system/hosts` in real hosts syntax, inside a dir the
tmpfiles line now re-declares `root:root 0700`. On every boot, `compose-hosts` idempotently:

1. **Re-own the dir** is handled by tmpfiles (single authority, [R1-MF4]); the oneshot never chowns it.
2. **Read the legacy `hosts` file defensively [R1-MF5]:** `lstat` + require a **regular file**; open
   `O_NOFOLLOW|O_NONBLOCK`. A **symlink** (root would write through it to an attacker path) or a **FIFO**
   (root read blocks → boot hang at `Before=local-fs.target` = DoS) or any non-regular type → **hostile**:
   ignore it, compose from the sealed baseline only.
3. From a valid legacy file, extract **only** lines whose hostname is one of the **4 model names**,
   rewrite as `<provider-token> <ipv4>` into the new `hosts-bindings` store (tmp+rename), and **discard
   every other line** — including any `shrek-swamp-broker` line (§3) and any attacker line a compromised
   pre-fix box planted. localhost comes from the baseline. **[N-R2-2]** a model name bound twice in the
   legacy file → **first occurrence wins** (matches glibc files first-match), the rest dropped.
4. Compose `/run/shrek/hosts` from the sealed baseline + readable bindings. **[R2-MF1] unconditional and
   independent of step 3:** a failed/absent/hostile store never blocks this step — localhost is always
   installed.
5. **[R1-MF6] rollback compatibility:** leave a sane, hosts-syntax, **localhost-bearing** file at the
   **legacy** `/home/.shrek-system/hosts` path (the old `hosts-seed` seeds only if absent, so without this
   a rollback to an old base finds the token-format file, skips every line, and **loses localhost**; the
   old `hosts-seed` would also re-chown to 1000). Document the rollback-boot window (old base = old
   defect briefly) as **accepted**.

## 8. `shrek-connect` + reader migrations

- `shrek-connect <provider> <addr>` → local IPv4 validation (advisory), then execs `egressd ask bind
  <provider> <addr>` (fixed argv, no shell; OK→0 / refused→1 / unreachable→2, mirroring `client.rs`).
  Drops all file I/O, `chown`, `require_writable`, and the hostname claim in its header **[R1-MF2]**.
- `--forget <provider>` → `egressd ask unbind <provider>`.
- `--list` → reads `/run/shrek/hosts` (root:0644), reports each name's address — **no new read verb**
  (matches S3 "read the projection for status"; **[N-3]/Q3**).
- **[R1-MF8d] two more readers must move to `/run/shrek/hosts`:** `shrek-agent` (~:152, the "no brain
  connected" advisory greps the store directly — breaks on a 0700 root store) and
  `dogfood-persist-probe` (~:820, asserts the old symlink target). Move both in S3, or S5 dogfood goes
  red. `SHREK_HOSTS` override → point at the projection.

## 9. The NM→resolved path (§1b, in-scope [R1-MF7]) — OPTION 4 (supersedes posture A)

**Owner decision 2026-09-04 (supersedes the locked posture-A design):** do NOT seal/cripple NM+resolved
DNS. That would harden the owner-controlled resolver — the exact anti-pattern MF-1/§3 already rejected.
Instead **remove resolved from gatekeeperd's PRIVILEGED egress-pin path**, so a uid-1000 NM `ipv4.dns`
edit can never steer egress POLICY, while NM/DHCP DNS stay fully functional for ordinary USER resolution.
Same lesson, generalized: *don't sanitize an owner-controlled resolver harder — stop using it as a
security oracle.* ADR-007 already did this for the desktop egress plane (sealed DoT); S4 extends it to
gatekeeperd.

Mechanism (S4a + S4b): the sealed DoT client (`egressd::dot`) is extracted into a shared **`shrek-dot`**
crate; gatekeeperd depends on it. `net_plane::resolve_profiles_v4` no longer calls `getaddrinfo`
(files+resolved). It resolves each rule host by CLASS — disjoint, so neither can steer the other:
- a sealed ALIAS (`shrek_policy::provider_bind::is_sealed_alias_host` — the 4 owner-bindable model
  brokers + the swamp broker) resolves ONLY from the root-owned `/run/shrek/hosts` projection; unbound ⇒
  fail-closed ("no brain connected"), never leaking the alias label to a public resolver;
- every OTHER (public DNS) name — github/debian/pypi/crates — resolves ONLY over the shared sealed-DoT
  client; the hosts file is NEVER consulted, so a poisoned hosts line / NM DNS edit CANNOT steer a public
  pin.

**Closure gate (its own host oracle, `scripts/gatekeeperd-egress-resolve-s4-proof.sh`):** a POISONED
hosts entry `6.6.6.6 github.com` is demonstrably IGNORED — gatekeeperd pins github.com's REAL IP over
sealed DoT (proven live, 4/4). No NM/resolved sealing, no dogfood dependency on resolved config.

**Left OPEN (filed separately — mycelium #3157, NOT ADR-008 scope):** whether uid 1000 should be able to
alter the system connection's DNS at all. It no longer touches egress policy, but still influences OTHER
root programs that naively use the system resolver (apt via getaddrinfo→resolved; a root `curl`; NTP is
already immune, ADR-007 S5). A distinct privilege review — NOT to be solved by crippling DHCP DNS.

## 10. Scope & delivery ordering (slices)

- **S1** — `shrek-policy`: closed provider-token table + `token→sealed-name` map + `valid_bind_addr`
  (strict IPv4, canonical render) + unit tests. Rust → system-index bump.
- **S2** — egressd: `hosts-bindings` store (`store.rs`, 0600 file) + `compose-hosts` routine
  (`/run/shrek/hosts`, sealed baseline, lock_store) + `Bind`/`Unbind` variants, parser branch (+abuse
  tests), `authorize` arm, journal/events addr wiring. Host oracle: bind→line, unbind→gone, idempotent
  unbind, non-provider token→refused, non-IPv4/hex-octal addr→refused+canonicalized, legacy
  symlink/FIFO/attacker-line store→ignored+stripped. Rust → system-index bump.
- **S3** — base `shrek-hosts-compose.service` (replaces seed; **removes the chown-to-1000**),
  tmpfiles dir-ownership reconcile, `/etc/hosts` retarget, `shrek-connect` + `shrek-agent` +
  `dogfood-persist-probe` reader migrations, rollback-compat legacy-path file. **[N-R2-1]** update the now-
  stale `resolved.conf.d/10-shrek-sealed.conf` comment (`#3121 … unowned`) in this diff.
- **S4 (Option 4, §9)** — the second half of #3121. **S4a** extract the sealed DoT client into a shared
  `shrek-dot` crate (behavior-preserving; egressd re-exports it). **S4b** repoint `gatekeeperd`'s
  `net_plane::resolve_profiles_v4` off `getaddrinfo`/resolved to files-then-DoT (aliases from the root
  hosts projection, public names over sealed DoT). Host oracle: a poisoned hosts entry does NOT steer a
  public pin. Rust → system-index bump. (The NM-polkit *privilege* question is filed separately, #3157.)
- **S5** — sealed-VM dogfood: localhost resolves at base boot; `shrek-connect local <ipv4>` binds and
  gatekeeperd pins it; a uid-1000 write to `/etc/hosts`/the store is refused; NTP/apt/public-profile
  resolution reads root-owned data; a poisoned NM DNS does not steer a public pin; binding survives
  reboot + A/B swap; rollback boot keeps localhost.

## 11. Non-goals

- Encrypting `/home` (tracked with the owner-provisioning FDE gap).
- Broker-leg server-authentication (R-1 residual — **filed as a tracked follow-up security item**, out of
  ADR-008 scope but NOT dismissed; the plaintext sandbox↔broker leg is a real redirect-MITM gap §3).
- Changing the agent sandbox's zero-resolver posture (unchanged — gatekeeperd still pins at construction).
- Making the 4 provider binds ceremony-gated (they are the owner's routine Authority-B — §3).

## 12. Decisions settled by Fable round 1 (were open questions)

- **Q1 → IPv4-literal only** (hostnames never worked in NSS `files`; IPv6 self-DoSes) — [R1-MF2].
- **Q2 → base `egressd compose-hosts` oneshot** (egressd is base overlay; subcommand touches no nft/CAP) —
  [N-2].
- **Q3 → `--list` reads the `/run` projection**, no read verb — [N-3].
- **Q4 → §9 is in-scope (S4), mandatory closure gate** — [R1-MF7].
- **Q5 → rename store to `hosts-bindings` + leave a rollback-compat localhost file at the legacy path** —
  [R1-MF6].

## 13. Owner decisions (resolved)

- **R-1 → file, don't accept.** The plaintext, server-unauthenticated sandbox↔broker leg is a real
  redirect-MITM gap (response confidentiality + output-injection integrity), not a triviality. Broker-leg
  server-auth is filed as a tracked follow-up security item, kept out of ADR-008 scope. §3 no longer
  claims "no new confidentiality loss."
- **R-2 → bench-only.** swamp-query does not ship on the target image; v1 baseline is localhost only.
  Migration strips any legacy `shrek-swamp-broker` line.

## 14. Changelog

- v1 → v2: folded Fable round 1 (MF-1 §3 honest rewrite, MF-2 IPv4-only, MF-3 swamp-broker, MF-4 dir
  ownership vs tmpfiles, MF-5 no-follow migration, MF-6 rename+rollback, MF-7 NM-polkit in-scope, MF-8
  completeness; N-1..N-6). Owner resolved R-1 (file broker-leg server-auth as tracked follow-up; §3 stops
  claiming "no new confidentiality loss" — the redirect-MITM is a real response-confidentiality +
  output-injection gap) and R-2 (swamp-query bench-only; baseline localhost-only). Ready for Fable round 2
  (§3 is the hold item).
- v2 → v3: Fable round 2 = GO-WITH-FIXES, §3 hold RELEASED ("now true, honest, drawn on the right
  boundary"; all 8 R1 MFs + both residuals verified against source). Folded the single round-2 must-fix
  MF-R2-1 (first-boot ordering: `compose-hosts` installs `/run/shrek/hosts` from the baseline
  UNCONDITIONALLY — store absence never blocks localhost, since tmpfiles runs after the oneshot on a
  virgin disk; §6/§7) + N-R2-1 (stale resolved.conf comment → S3) + N-R2-2 (dup legacy line → first-wins).
  **No round 3 — ready for owner LOCK.**
- POST-LOCK AMENDMENT (owner, 2026-09-04, during S4 build): §9 **OPTION 4 supersedes posture A**. The
  second half of #3121 is fixed not by sealing NM/resolved DNS but by removing resolved from gatekeeperd's
  privileged pin path — the sealed DoT client is extracted to a shared `shrek-dot` crate (S4a) and
  gatekeeperd resolves public pins over it, aliases from the root hosts file, resolved never (S4b). Same
  MF-1/§3 lesson generalized: don't harden the owner-controlled resolver, stop using it as an oracle. The
  distinct "may uid 1000 edit system-connection DNS at all" privilege question is filed OUT of scope
  (#3157) — not to be solved by crippling DHCP DNS. §2.4, §9, §10-S4 updated to match.
