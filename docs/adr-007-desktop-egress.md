# ADR-007 — Desktop egress plane & the user-blessed connectivity boundary

Status: **COMPLETE (2026-09-04) — S1–S6 all shipped; the plane is VM-proven end to end (desktop-egress-s6-vm-proof.sh 13/0).** Includes the **S6.1 fix #4 hardening** (2026-09-04): the ceremony commit is routed through the egressd daemon over root-gated IPC and `gatekeeperd` drops `CAP_NET_ADMIN` (§14 S6.1). ACCEPTED owner 2026-09-03 after Fable round-3 GO (clean); mirrors the ADR-005 flow.
Three Fable rounds folded: R1 GO-WITH-FIXES (MF-1..MF-5) → R2 GO-WITH-FIXES (R2-MF-A/B/C) → R3 **GO**, with the
two-line §7 insertion-point reconciliation applied. See the `[R1-MFn]`/`[R2-MF-x]` markers inline and the §14 changelog;
Q3/Q5/Q6/Q7/Q8 resolved (Q6b `desktop-updates` endpoint still TBD, does not block S1). **Note:** the uid-1000-owned
`/etc/hosts` defect surfaced by MF-1 is filed separately as **mycelium #3121** — this ADR now closes its OWN exposure
(sealed DoT), and #3121 owns the broader host-wide fix.
No core code yet. Predecessors this reuses: the agent egress plane (`crates/shrek-policy/src/egress.rs`,
`crates/gatekeeperd/src/net_plane.rs`), the console consent ceremony (`crates/gatekeeperd/src/consent.rs`),
and the writable provisioning store (ADR-005 §4). Companion to the "sealed, deny-by-default, explicit
consent" philosophy already applied to the agent/Bench plane (ADR-003).

## 1. Context

Shrek OS ships a sealed dm-verity system where the agent/Bench plane already egresses **deny-by-default**:
a workload reaches only the endpoints named by a **sealed** `EGRESS_PROFILE`, pinned to A-records at
construction, everything else dropped (`egress.rs:207` `EGRESS_PROFILES`; `net_plane.rs:142` the forward
chain's trailing `ip saddr {cont_ip} drop`). The **desktop session has no such governance today** — and the
gap already forced a defensive decision:

> **S7 (docs/desktop-functionality-sprint.md:144):** "Weather tab curls a third-party API on a timer —
> undeclared recurring phone-home from a system shell. Owner decision: disable the tab rather than open a
> shell egress hole. Seeded OFF in default-settings.json; dogfood asserts the seed."

So the desktop's answer to "a widget wants the network" has been **disable it**, because the only
alternatives were "open a hole" or "ship a phone-home." That is the wrong long-term shape: a daily-driver
desktop legitimately wants weather, an accurate clock, and system updates. The owner's framing resolves it:

> **The desktop shell runs deny-by-default egress, and the USER blesses the specific destinations their
> desktop experience needs.** "Egress and a decision" — the plane *plus* the human consent to open it.

This ADR defines that plane and the bless UX. It is the same governance the agent plane already has, but
**user-owned** rather than policy-owned: the human, not a sealed tier matrix, decides what their desktop
may reach — within sealed, pre-pinned capability profiles, with a raw-destination escape hatch for power
users.

### 1a. Why the desktop can't just reuse the agent plane verbatim

Three asymmetries (all verified in code):

1. **The desktop is the host session, not a sandbox.** The agent plane pre-creates a `netns` + `veth` /30
   per sandbox and hangs a per-sandbox `nft` table off it (`net_plane.rs:46-86`). The desktop is `dev`
   (uid 1000) autologged into tty1 under logind's `session-c1.scope` on the **host** netns
   (`shrek-desktop` → `sway`; `getty@tty1.service.d/autologin.conf:22`) under a logind session scope on the
   **host** netns. It needs the real NIC via NetworkManager. So enforcement is a **system `nft` filter scoped
   to the desktop session** (by `skuid` / cgroup), not a netns-per-sandbox plane. (We match on `skuid 1000`,
   not a baked scope name — an autologin session scope is a numeric `session-N.scope` and `skuid` matching
   does not depend on it.)
2. **Consent must persist.** The ceremony records **nothing** durable — each approval is ephemeral wire
   framing (`consent.rs:346`). A blessed weather widget must stay blessed across reboots, so the bless set
   is **persisted** to the `/home` store (ADR-005 §4 pattern), not re-prompted every boot.
3. **DNS egress is deliberately absent.** The agent plane resolves profile hostnames to pinned IPs once,
   at construct, and writes `/etc/hosts` (`net_plane.rs:315` `etc_hosts()`; `egress.rs:23` "no DNS
   egress"). Weather/NTP by hostname need resolution somewhere. §5 picks how.

## 2. Decision (summary)

1. **A desktop egress plane**: a system `nft` table, applied before the compositor reaches the network,
   that **denies all egress from the desktop session (uid 1000) by default** and allows only (a) a sealed
   **system baseline** and (b) the user-**blessed** capability profiles. Reuses the `EgressRule` /
   `EgressProfile` / `resolve()` model and the `nft` allow-then-drop shape from the agent plane; does
   **not** reuse the netns/veth wiring.

2. **Tiered bless.** The user blesses **capability profiles** — `weather`, `web-browsing`, and (future)
   others — each backed by a **sealed** endpoint set (`DESKTOP_EGRESS_PROFILES`, a new sibling of
   `EGRESS_PROFILES`). An **advanced raw-destination editor** underneath lets a power user add
   `host:proto:port` entries; raw entries are subject to the same pin-at-bless discipline as profiles
   (§5), never a DNS hole.

3. **Minimal baseline pre-blessed.** Out of the box, **time-sync (NTP)** and **layer-store updates** are
   allowed so the clock is correct and the system is updatable on first boot **without any user action**.
   These are **system-service** egress (their own service uids), governed by the sealed baseline, not the
   uid-1000 bless set. **Weather and web-browsing are opt-in** (deny until blessed) — this is the direct,
   principled replacement for S7's "disable the tab."

4. **Bless surface = onboarding pick + Settings panel.** First-run onboarding offers the initial bless
   ("What should your desktop be allowed to reach?"); a DMS Settings → Connectivity panel changes it later.
   The blessed set persists to the writable `/home` store and is applied over sealed config by bind-mount
   (ADR-004 / ADR-005 §4 discipline).

5. **`web-browsing` is a broad bless, flagged as such.** A browser reaches arbitrary hosts, so blessing it
   cannot be pin-scoped — it lifts the deny for the browser's scope. It is presented as the
   high-consequence grant it is (mirrors the agent plane's `C-broad` tier), never bundled into a
   low-friction toggle.

## 3. The trust boundary (the load-bearing part)

**Who may bless, and how is that act trusted?**

The desktop user *is* `dev` (uid 1000) — the same subject that runs the widgets wanting egress. So a naive
"Settings toggle flips the firewall" means the exact process being contained also authors its own
containment. Two candidate boundaries:

- **(A) Console-ceremony bless** — reuse `consent.rs`: blessing a profile is a `network --bless <profile>`
  verb that triggers SAK → dedicated VT → diff render → typed confirm (`consent.rs:358`
  `run_socket_consent()`). Strongest (trusted path, SAK defeats a spoofed prompt), but heavy for a routine
  "turn on weather," and the ceremony is **ephemeral** so we'd bolt persistence on separately.
- **(B) Supervisor-mediated Settings bless** — the DMS Settings panel asks a small **root supervisor**
  (the only writer of the sealed-over-`/home` egress store and the `nft` applier) to add/remove a blessed
  profile. The supervisor, not uid 1000, owns the store (root:root 0700) and the ruleset; uid 1000 can only
  *request*. Lower friction; the boundary is "root writes the store, dev requests," same split the Bench
  plane uses (root owns pool/quota/record, drops to dev for container ops, `bench_plane`).

**DECIDED (Q3, Fable round-1):** **tiered by consequence.** Low-consequence, pin-scoped profiles
(`weather`, other sealed capability profiles) → **(B)** supervisor-mediated, no VT ceremony — the endpoints
are sealed and pinned, so the blast radius of a mis-bless is one known API. High-consequence grants
(`web-browsing` = broad egress; **any raw-destination** add) → **(A)** the full console ceremony, because
those genuinely widen the boundary to attacker-chosen or arbitrary hosts. This mirrors `decide_apply()`
already gating high-authority verbs behind a typed 6-digit code vs. a bare `y` (`consent.rs:129-145`).
Forcing *every* bless through the SAK ceremony is rejected: routing a routine "turn on weather" through
VT-flips manufactures exactly the approval fatigue the ceremony's own escalating cooldown
(`consent.rs:286-303`) treats as an attack class, cheapening the ceremony for the grants that need it.

**`[R1-MF5]` Tier-B admission rule + legibility (load-bearing — the supervisor cannot tell the Settings
panel from malware; both are uid 1000 on a socket, so a compromised session CAN self-bless any tier-B
profile with no human in the loop).** Tier-B is therefore honestly "pre-approved capability, deferred-on" —
the real human consent for a tier-B profile happened when it was **sealed into `DESKTOP_EGRESS_PROFILES`**,
not at the toggle. Three normative requirements make that defensible:
- **(a) Admission criterion (normative):** a profile qualifies for tier-B **only if silently blessing it
  from a fully-compromised uid-1000 session is an acceptable outcome.** `weather`→open-meteo passes (one
  keyless read-only API, no attacker-readable storage, no reflection/redirect). Anything whose endpoint can
  store-and-reflect attacker data, or is an open relay/proxy, is **not** tier-B → ceremony tier.
- **(b) Legibility:** every bless/unbless is journaled and raises a desktop notification (ADR-004 spirit) —
  a silent self-bless is at least *visible* after the fact.
- **(c) Channel hardening:** the supervisor `SO_PEERCRED`-gates the request socket to uid 1000 and
  rate-limits bless requests (a compromised session cannot brute-flip profiles faster than a human notices).

**What crosses the boundary:** only a **profile name** (or, for raw, a `host:proto:port` triple that the
supervisor resolves-and-pins itself). Never a live IP set authored by uid 1000 — the sealed profile table
is the only source of a capability profile's endpoints (`egress.rs:227` `resolve()` is strict fail-closed;
an unknown name yields no rule, so a typo can't widen anything). **`[R1-MF1]` This guarantee holds ONLY if
name→IP resolution does not consult a uid-1000-authored source.** It currently *would*: `/etc/hosts` is a
baked symlink to the `/home/.shrek-system/hosts` store, which `hosts-seed` **chowns to uid 1000 every boot**,
and `resolve_profile_v4()`→glibc `getaddrinfo` honors NSS `files` first — so uid 1000 could write
`<attacker-ip> api.open-meteo.com` and have the root supervisor pin the attacker IP (same trick steers
`@ntp_pinned` = clock control). §5 mandates the fix (resolve off-NSS + `ReadEtcHosts=no`); until it lands,
the "name is the only source" boundary is **void**. The underlying uid-1000-owned-hosts defect is filed as a
**separate cross-cutting security item** (§14) because it grants uid 1000 name-resolution authority over
*every* root daemon, not just this plane.

## 4. The desktop egress store

Parallel to the provisioning store (ADR-005 §4), **supervisor-owned**, on persistent `/home`:

```
/home/.shrek-system/egress/
  blessed              # legible list of blessed profile names, one per line (ADR-004: LF, sorted)
  raw                  # advanced tier: user-added host<TAB>proto<TAB>port lines (may be absent/empty)
  pinned/              # resolve-and-pin output the applier consumes (never authored by hand)
    <profile>.hosts    #   host -> Ipv4Addr lines, one file per blessed profile (agent-plane etc_hosts shape)
  .applied             # completion sentinel + last-apply stamp (gate-crash → deny, never open, per ADR-005)
  fault                # legible per-entry rejection reasons (unknown profile, unresolvable host, …)
```

- Ownership **root:root 0700** throughout — same trust domain as `/home/.shrek-system/provisioning` and
  `/home/.shrek-system/NetworkManager`; **never** chowned to uid 1000. uid 1000 requests changes via the
  supervisor; it cannot write this tree.
- Plain legible files (ADR-004): the blessed set is auditable, diffable, greppable. No binary, no DB.
- **Applied by feeding the `nft` named sets, NOT by writing `/etc/hosts`. `[R1-MF1]`** The pinned IPs the
  applier consumes go straight into `@weather_pinned` / `@ntp_pinned` (§7). We do **not** materialize pin
  fragments into `/etc/hosts`: unlike `locale.conf`/`vconsole.conf`/`localtime` (real sealed RO files
  ADR-005 binds over), `/etc/hosts` is a **symlink into the writable, uid-1000-owned** `/home/.shrek-system/hosts`
  store — bind-mounting the path would resolve the symlink and shadow the owner's `shrek-connect` bindings
  (the exact bind-over-symlink trap ADR-005 §5a documents for `/etc/localtime`), and worse, hangs the pin
  off a uid-1000-writable file (MF-1). The `nft` set delivers *enforcement*; it does **not** deliver
  *name resolution* to the widget. Any config bound over sealed RO `/etc` targets follows ADR-005 unchanged.
- **`[R2-MF-A]` Name→IP delivery to blessed widgets — the store is 0700, so widgets can't read `pinned/`.**
  The supervisor additionally publishes a **world-readable projection** at `/run/shrek/egress/pinned`
  (root-written, 0644, tmpfs — a projection of `pinned/`, not the authority): `<name> <ip>` lines for each
  currently-blessed profile. A blessed widget resolves the sealed name against this map and dials with
  `--resolve <name>:443:<ip>` semantics (curl `--resolve`, or the equivalent connect-to-IP-but-SNI/verify-name
  path), so **TLS certificate + hostname verification still runs against the sealed name** (§5's transport
  floor holds; raw-IP dialing would break hostname verification and is forbidden). The widget performs **no
  DNS** — the map is the only name source it needs, and it carries only IPs the supervisor already pinned into
  the `nft` allow set, so map and enforcement can never disagree in the widget's favor.

## 5. DNS & pinning (the sealed-model tension)

Weather/NTP/updates are named by hostname; the sealed model forbids ad-hoc DNS egress. Options considered:

- **(i) Resolve-and-pin at bless time** — when the user blesses `weather`, the **supervisor** resolves the
  sealed profile's hostnames to A-records once, writes `pinned/weather.hosts`, and adds those IPs to the
  desktop `nft` allow set. No standing DNS egress. **Con:** API/NTP IPs rotate (CDN, NTP pools), so a pin
  goes stale → periodic **re-pin** needed (a timer, or re-pin on network-up + on failure).
  *(Superseded detail: this option originally proposed reusing `resolve_profile_v4()` and writing a bound
  `/etc/hosts` fragment — both replaced by the `[R1-MF1]`/`[R2-MF-C]` sealed-DoT resolver and the §4
  `/run/shrek/egress/pinned` projection. Don't build the struck version.)*
- **(ii) Bless a resolver** — allow uid-1000 DNS egress to the configured upstream (or to `resolved`'s
  upstream) as its own baseline. Simplest for rotating endpoints. **Con:** DNS is itself an exf/covert
  channel; it reintroduces the hole the sealed model closed, for the whole desktop.

**DECIDED (Q5, Fable round-1):** **(i) resolve-and-pin, with a bounded re-pin.** Keep the
no-**uid-1000**-DNS invariant (see the honesty note below). The supervisor re-pins blessed profiles on
`network-online` and on a slow timer (and immediately on a connect-failure signal), refreshing
`pinned/*.hosts` + the `nft` set atomically. This preserves "every reachable IP was, at pin time, an
A-record of a **sealed** hostname" — a typo or a compromised widget can never point the pin at an arbitrary
host, because the hostname list is sealed (profiles) or itself ceremony-blessed (raw tier). Weather tolerates
a stale pin (worst case: a refresh fails, widget shows "offline"); NTP tolerates it (multiple servers, slow
drift). Fable round-1 empirically confirmed both sealed endpoints pin cleanly today: `api.open-meteo.com` →
a **single stable A record** (their own infra, not a rotating CDN); `time.cloudflare.com` → two stable
anycast IPs. Neither churns, so re-pin thrash is not a concern for the shipped profiles.

**`[R1-MF1]` + `[R2-MF-C]` The re-pin resolver is a sealed DoT client against sealed upstream IPs — it
consults NO uid-1000-authored source, at any layer.** Round-1 mandated "not NSS `files`"; round-2 showed
that is necessary but not sufficient, because uid 1000 can steer the *upstream* too: `mkosi.postinst:190-195`
grants seat0 the `NetworkManager.settings.modify.system` polkit action with no admin, so uid 1000 can set the
system connection's DNS servers — which both `resolved`-varlink and `/etc/resolv.conf`-based "direct DNS"
ultimately consult. So the supervisor's re-pin does **not** go through `resolved`, NM, or `/etc/resolv.conf`
at all. It is a small **DoT client baked into the supervisor** that queries a **sealed set of resolver IPs**
(e.g. `1.1.1.1`/`9.9.9.9` on `853`, part of the sealed policy, `contradicts` nothing uid 1000 can touch) and
validates the DoT cert against the sealed resolver name. `resolved` may keep resolving for its own consumers;
it is simply not in the pin's trust path. The image still ships sealed `resolved.conf` (`ReadEtcHosts=no`,
plus `LLMNR=no`/`MulticastDNS=no` so no single-label sealed host is ever LAN-spoofable). This makes §3's
"name is the only source" boundary **actually true**, closing R2-MF-C inside this ADR rather than deferring
it to the cross-cutting fix (#3121 still lands for the broader hosts-poisoning surface). Hard requirement of S2.

**`[R2-MF-C]` NTP bootstrap breaks the DoT↔clock cycle (owner catch).** DoT needs a valid TLS handshake,
which needs a roughly-correct clock, which needs NTP — but NTP is exactly what we're trying to reach. Circular.
Break it: **`desktop-ntp` uses sealed resolver-independent IPs directly and does NO resolution.** Cloudflare's
`time.cloudflare.com` anycast IPs (`162.159.200.1`/`162.159.200.123`) are stable and are **sealed as literal
IPs** in `desktop-ntp` (SNTP has no cert to verify a name against anyway, so pinning the IP loses nothing).
timesyncd corrects the clock off those sealed IPs at boot → the clock is sane → the supervisor's DoT re-pin
of `weather`/`updates` can then succeed. `desktop-ntp` is thus the one profile that is **pre-pinned at seal
time, never resolved at runtime**; everything else DoT-resolves after the clock is good.

**`[R1-MF2]` Honesty about "no DNS."** The correct invariant is **no *uid-1000* DNS egress**, not "no DNS at
all": the supervisor's re-pin and `resolved` itself still perform standing DNS under their own uids. The
prior §7 "resolved stub only; no upstream DNS from uid 1000" comment was **wrong** — a uid-1000 query to the
`127.0.0.53` stub causes `resolved` to do the upstream lookup under *its* uid, sailing through `policy
accept`, i.e. an unrestricted DNS exfil channel for the whole session. §7 therefore **drops the stub allow
by default** and opens it only inside the blessed `web-browsing` scope (a browser genuinely needs live DNS).

**Security floor (state it plainly).** Re-pin resolution over DoT is transport-authenticated to a sealed
resolver; the pinned weather IP's floor is then the widget's own TLS (HTTPS + cert + hostname verification
via the §4 `--resolve` path). SNTP via timesyncd has no NTS and is spoofable regardless — the sealed-IP pin
only narrows which host may answer, accepted for a clock. The failure-triggered re-pin is **rate-bounded** so
a malicious widget cannot weaponize forced re-pins as a low-rate DNS side channel.

**`[R2-MF-A]` Residual DNS surface nft cannot close (accepted, named).** With the stub dropped and the pin
off `resolved` entirely, a *compromised* uid-1000 process can still call `resolved`'s unprivileged
`org.freedesktop.resolve1` `ResolveHostname` D-Bus method directly; `resolved` then performs the upstream
query under its own uid and no uid-1000 packet ever reaches the nft hook. This is a **low-bandwidth covert
channel, not an unrestricted egress path** (it returns records, it doesn't POST data), and it is un-closable
by a packet filter while `resolved` serves the session at all. Accepted residue for S1–S6; the real
mitigations (a bus policy restricting `resolve1` to non-session uids, or dropping `resolved` for the desktop
session in favor of the map) ride with the cross-cutting #3121 work, not this ADR. Named here so round-3
doesn't re-flag it as missed.

## 6. Sealed profiles this ADR creates

New sealed table `DESKTOP_EGRESS_PROFILES` in `shrek-policy` (sibling of `EGRESS_PROFILES`, same
`EgressProfile`/`EgressRule` types, dm-verity-sealed, strict fail-closed `resolve()`):

| profile        | tier            | endpoints (sealed hostnames)                    | default | notes |
|----------------|-----------------|-------------------------------------------------|---------|-------|
| `desktop-ntp`  | system baseline | **sealed literal IPs** `162.159.200.1`,`162.159.200.123` (Cloudflare anycast) — **DECIDED Q6a + `[R2-MF-C]`** | **on** | pre-pinned at seal time, **NO runtime resolution** (breaks the DoT↔clock cycle, §5); shipped `timesyncd.conf` sets `NTP=` to those IPs directly (SNTP verifies no name anyway) |
| `desktop-updates` | system baseline | the layer-store / sysupdate source — **OPEN Q6b (TBD; ship NTP+weather first)** | **on** | governs the update fetch; endpoint not yet defined in-repo — profile stubbed, wired in S5 once the source exists |
| `weather`      | user-blessed    | `api.open-meteo.com` (keyless, no account) — **DECIDED Q6c** | **off** | replaces the S7-disabled tab; DMS `weather` dashTab re-enables **only** when blessed |
| `web-browsing` | user-blessed (broad) | *unpinnable — broad egress*                | **off** | console-ceremony bless; lifts deny for the browser scope only |

**Baseline vs. bless, by identity.** `desktop-ntp` and `desktop-updates` are **system-service** egress
(timesyncd's uid; the updater's uid/root), always-allowed by the sealed baseline — that is what "minimal
baseline pre-blessed" means: the box keeps time and updates with zero user action. The **uid-1000** desktop
session is deny-by-default and only `weather` / `web-browsing` / raw entries open it.

## 7. Enforcement — the desktop `nft` filter

**`[R1-MF3]` Two-phase, so "the drop stands" is structurally true, not aspirational.** A single
supervisor-built table is fail-**open** on the path that matters: if the supervisor is slow or crashes there
is *no table at all* (nothing "stands"), and the ordering it leaned on gates nothing —
`shrek-desktop-ready.service` exits 0 even on timeout ("proceeding anyway",
`shrek-desktop-ready.service:25`) and autologin orders `After=`/`Wants=` only
(`autologin.conf:17-18`). So a crashed supervisor → getty starts anyway → uid-1000 session with full host
egress. Fix = two phases:

**`[R2-MF-B]` ONE static baked table with empty named sets — not two tables, not `add rule`.** Round-1's
"phase 1 base table + phase 2 supervisor adds" is unbuildable as literally stated: two output-hooked tables
means a `drop` in the base kills packets an accept in the other can't rescue (blessed traffic permanently
dead), and one table with `nft add rule` **appends after** the trailing drop (allows never match) — and any
flush/rebuild on re-pin transiently removes the static drop, re-creating the fail-open window. Correct shape:

- **A single static table `inet shrek_desktop_egress`, baked in `/usr`, containing ALL rules** — loopback,
  baseline, blessed-profile allows, trailing drop — where every allow references an **`nft` named set that
  starts EMPTY**. An empty set matches nothing, so an unblessed/unpinned profile's allow is inert = fail-closed
  (the same "empty ≠ accept-all" property S1 unit-tests in `shrek-policy`). The rule *order* is fixed at bake
  time; the drop is always last and is never the supervisor's to touch.
- **The supervisor is a set-element-only writer.** Bless/re-pin = `nft add element @weather_pinned { <ip> }`
  (and delete on unbless) — atomic per-set, no rule ordering, no table/chain flush, ever. An apply error
  leaves the baked drop + empty sets exactly in place → no egress, never open.
- **The one runtime *rule* insert is the browser-cgroup pair** (Q7), which needs a live cgroup id: inserted
  **above rule 0** by handle on browser launch (per the §7 NOTE — the scoped stub-accept must precede the
  broad rule-0 stub-drop), deleted on exit. Everything else is elements.
- Loaded by an early oneshot that `getty@tty1` **`Requires=` AND `After=`** (a `Requires=` without ordering
  is a dependency that can still race) — the fail-closed precedent ADR-005 carves out for the credential path
  (`adr-005-provisioning.md:396-397`). If the static table can't load, tty1 never starts. This does **not**
  violate never-emergency-mode: that rule governs the non-secret provisioning plane, not the deny-by-default
  egress boundary. **Recovery-vs-hole tradeoff (state it):** only `getty@tty1` carries the drop-in, so a
  human could log in on tty2 if the static load fails — that is both the recovery path *and* an ungoverned
  session in that exact failure mode. Accepted for now (a bricked tty1 with no egress-governed session is the
  rare hardware/policy-load fault; tty2 needs the console); revisit if tty2 autologin ever ships.

Shape of the baked ruleset (reusing the agent plane's allow-then-drop discipline, `net_plane.rs:94-160`,
adapted from netns to session-scope; `policy accept` never affects intra-chain ordering — first matching
verdict wins):

```
table inet shrek_desktop_egress {
  chain output {
    type filter hook output priority 0; policy accept;   # scope, don't hijack the global policy

    # 0. [R2-MF-A] STUB DROP FIRST — the resolved stubs are loopback, so this MUST precede rule 1's
    #    loopback accept or uid 1000 gets whole-session DNS (the MF-2 exfil channel). Browser scope
    #    re-opens the stub above this drop (rule 2b). Cover both stub IPs (resolved >=v252 adds .54), udp+tcp.
    meta skuid 1000 ip daddr { 127.0.0.53, 127.0.0.54 } th dport 53 drop

    # 1. [R1-MF4] loopback for the session — family-agnostic, covers 127/8 AND ::1 (stub already dropped above)
    meta skuid 1000 oif "lo" accept

    # 2a. system baseline — always allowed, matched by service identity (uid), pinned sets (start EMPTY)
    meta skuid systemd-timesync ip daddr @ntp_pinned      udp dport 123 accept   # @ntp_pinned = sealed literal IPs
    meta skuid <updater-uid>    ip daddr @updates_pinned   tcp dport 443 accept

    # 2b. web-browsing, if blessed: broad egress for the BROWSER SCOPE only (cgroup match, Q7).
    #     Inserted ABOVE rule 0 at browser launch (the ONE runtime rule — see NOTE). Browser needs live DNS,
    #     so the stub is re-opened here and NOWHERE else:
    #     meta skuid 1000 socket cgroupv2 level 4 "user.slice/user-1000.slice/user@1000.service/shrekbrowser.slice" ip daddr { 127.0.0.53, 127.0.0.54 } th dport 53 accept
    #     meta skuid 1000 socket cgroupv2 level 4 "user.slice/user-1000.slice/user@1000.service/shrekbrowser.slice" accept
    #     (S6: the slice is UN-hyphenated `shrekbrowser.slice` — a `-` is a systemd cgroup hierarchy separator
    #      that would nest it under a synthetic `shrek.slice`; a `--user --scope` launch lands it at level 4
    #      under `user@<uid>.service`. The path MUST be a QUOTED nft string or the lexer chokes on `user@1000`.)

    # 3. desktop session: blessed, pinned endpoints (element added to @weather_pinned on bless; empty until then)
    meta skuid 1000 ip daddr @weather_pinned tcp dport 443 accept        # inert until 'weather' blessed

    # 4. default-deny for the desktop session — everything else uid 1000 tries is dropped (v4 AND v6)
    meta skuid 1000 drop
  }
}
# NOTE: rule 2b's stub-accept must be evaluated before rule 0's stub-drop for browser packets. Since 2b is
# scoped by cgroup and 0 is not, order them so the scoped accept precedes the broad drop: bake 0 to also carry
# a negative cgroup guard, OR (simpler) place the browser-scope stub accept physically above rule 0. The
# supervisor inserts 2b before rule 0 by handle at launch. S2 must prove: non-browser uid-1000 stub = DROP,
# browser-scope stub = ACCEPT.
```

- **`policy accept` + explicit uid-1000 drop**, not `policy drop`: we govern the desktop session, we do not
  seize the host's global egress policy (same care as the agent plane keeping its forward hook at
  `policy accept`, `net_plane.rs:147`). System daemons not named above are unaffected.
- **`[R1-MF4]` loopback + IPv6 posture:** the session-loopback allow is `oif "lo"` (covers 127/8 *and* `::1`,
  so localhost-binding desktop software doesn't break — the resolved stub is already excepted by rule 0), and
  the trailing `skuid 1000 drop` is family-agnostic so it catches any v6 escape (kernel-originated v6 ND/RS
  carries no socket uid, so it isn't matched and passes — no legitimate uid-1000 v6 baseline is killed).
  Pinning stays **IPv4-only** (agent-plane parity, `egress.rs:14,20-21` "IPv4-only"); the widget dials the
  pinned v4 IP via the §4 map, so a v6-preferring network still egresses via the pinned v4 endpoint.
- **`[R2-MF-B]` named sets, element-only writes:** `@ntp_pinned` is populated at **seal time** with the
  sealed literal IPs and never re-pinned (`[R2-MF-C]`); `@updates_pinned` / `@weather_pinned` start **empty**
  and the supervisor adds/removes **elements** (never rules, never a flush) on bless/re-pin. There is
  deliberately **no `ct state` established-accept** in front of the drop: unbless/re-pin must kill established
  flows on the next packet — do not "optimize" a conntrack accept in later, it would defeat revocation.
- **`[R1-MF3]`+`[R2-MF-B]` Fail-closed** holds by construction: the baked static table (drop last, sets
  empty) is `Requires=`+`After=`d by getty; a supervisor apply-error touches only set elements, never the
  drop; never open. (Round-1 fail-open trace cite: `shrek-desktop-ready.service:25` "proceeding anyway".)
- **DECIDED (Q7) — cgroup match on `shrek-browser.slice`, NOT a sub-uid, stated for what it is.** Mechanics
  work: `socket cgroupv2 level N "<scope>" accept` placed above `skuid 1000 drop` matches first (kernel ≥5.13,
  trixie fine). Anchor on a **stable slice** (`shrek-browser.slice` under `user@1000.service`), because
  `socket cgroupv2` resolves to a cgroup **id at rule-load time** — a per-launch transient scope gets a fresh
  id and the stale rule silently stops matching; the supervisor (re)installs the rule on browser launch.
  **Honest limit:** a cgroup is *not* an adversarial boundary against uid 1000 — any uid-1000 process can
  `systemd-run --user --slice=shrek-browser.slice <anything>` and inherit the broad accept. Cgroup-scoping is
  **accident/UX containment**, not attacker containment; only a dedicated sub-uid would be an adversarial
  boundary, and that costs the entire session integration (Wayland socket, `XDG_RUNTIME_DIR`, portals,
  PipeWire) — not worth it at S-scope. The web-browsing **ceremony text must say plainly: "this effectively
  grants your desktop session broad egress."**
- **`[R1-MF2]` baseline uid resolvability:** `meta skuid systemd-timesync` requires the name to resolve in
  the sealed `/etc/passwd` at rule-load. Debian creates the user statically at package install, but S2's host
  oracle MUST verify (`getent passwd systemd-timesync` on the merged root + `nft -c` parse); safer still,
  seal a numeric uid via `sysusers` and match numerically.

## 8. Bless UX (config + UI)

- **First-run onboarding** (new Quickshell surface, sibling of the installer first-run): after owner enroll,
  a "Connectivity" step — "Your desktop starts sealed. Choose what it may reach:" with the baseline shown as
  **already on and explained** (time, updates) and **weather** / **web-browsing** as opt-in cards. Blessing
  weather here is the low-friction path (tier-B); blessing web-browsing triggers the console ceremony.
- **DMS Settings → Connectivity panel** (steady-state): the same list, plus the **advanced raw-destination
  editor** (add `host:proto:port`; each add is ceremony-blessed and resolve-and-pinned — **S4**). Shows
  per-profile status (blessed / pinned-IPs / last-refresh / fault) — legible, matching the store.
- **[S3] How the UI reads + writes.** Per-profile status is a **root-written `/run/shrek/egress/state`
  projection** (`root:root 0644`, one line per sealed profile: tier + blessed + pinned-IPs + last-refresh +
  fault-**kind**; closed tokens only, never the free-text fault reason), refreshed at every store mutation.
  The UI polls that file — it never reads the `0700` store and never polls the socket. Mutations go the
  other way, through the unprivileged **`egressd ask <verb> [profile]`** client (fixed argv, no shell) to
  the S2 supervisor socket; the daemon stays the sole authority. The state file is the **only** display
  truth — the panel never flips a control on an action's reply; it marks the control busy until the next
  poll confirms. A blessed-but-unpinned profile (a first-run bless before the clock/network converged)
  renders as **"Blessed — waiting for network,"** with a single user-initiated *Try now* (re-pin) — never a
  UI auto-retry (the rate limiter would starve the owner's own clicks). Baseline and `web-browsing` are
  **shown but not toggled** here (baseline revoke + `web-browsing` bless are the console ceremony, S4); the
  baseline status is explanation-only and never wired to `unbless` (defense in depth — the daemon refuses a
  baseline/broad socket bless regardless).
- **Baseline visibility (Q8):** `desktop-ntp` / `desktop-updates` appear as **"System baseline — on,"**
  inspectable (pinned IPs, last-refresh) exactly like any profile — not hidden. They are **revocable only
  through the console ceremony**, with an explicit consequence warning (clock-skew cert breakage, unpatched
  image), never a casual toggle.
- **DMS weather dashTab coupling**: `default-settings.json` keeps `weather: enabled=false` as the *shipped*
  default; the tab is enabled **iff** `weather` is in `blessed`. The dogfood seed-off assertion (S7) is
  replaced by "weather tab off **unless** blessed" — the seed and the plane stay consistent.

## 9. Variant gating

- **INSTALLABLE (product)**: full plane — baseline on, uid-1000 deny-by-default, bless surfaces live.
- **DOGFOOD (proof)**: plane on; a baked test bless set drives the sealed-VM assertions (§11). No console
  ceremony in headless dogfood — bless via the supervisor's seed seam (mirror ADR-005's
  `SHREK_PROVISION_SEED_MANIFEST`).
- **LIVE_INSTALLER**: **no desktop egress plane** — the live medium is ephemeral and already NOPASSWD-dev;
  governing its egress is out of scope (it has no persistent bless store). Time on the live medium uses the
  same baseline NTP if present, else nothing.
- **plain CI**: plane inert (no `/home` store), same as other `/home`-plane features.

## 10. A/B sysupdate safety

- The sealed `DESKTOP_EGRESS_PROFILES` table and the supervisor/applier ride the image (A/B-updated like any
  Rust/unit change). The **blessed set + pins live on `/home`** and survive A/B (same as the provisioning
  store and NetworkManager keyfiles) — an update never re-prompts the user or drops their weather bless.
- A profile whose sealed endpoints change across an update re-pins on next boot; a **removed** profile that
  is still in `blessed` is a legible `fault` (ignored, not fatal) — never an emergency-mode trigger. (The
  never-emergency rule governs the **supervisor/bless plane**: its apply-fail degrades to the static drop, it
  does not wedge boot. It is not "no `Requires=`/`Wants=` anywhere" — ADR-005 permits `Wants=` via targets
  and deliberately uses getty `Requires=` for the fail-closed *credential* path, `adr-005-provisioning.md:390-397`;
  MF-3's Phase-1 static drop reuses exactly that getty-`Requires=` precedent.)

## 11. Scope & delivery ordering (slices)

1. **S1 — sealed profiles + policy + ONE static baked table**: `DESKTOP_EGRESS_PROFILES` in `shrek-policy`
   (weather + baseline entries; `desktop-ntp` = **sealed literal IPs** `[R2-MF-C]`), strict fail-closed
   `resolve()`, unit tests — **plus the single baked `shrek_desktop_egress` table `[R2-MF-B]`** with ALL rules
   present (rule-0 stub-drop, loopback, baseline, blessed allows, trailing drop) referencing **empty** named
   sets, and its early oneshot that `getty@tty1` `Requires=`+`After=`. Unit tests must include: **an empty
   set / empty profile renders an inert allow, NOT accept-all** (leans on `EgressProfile::is_empty()`,
   `egress.rs:72`) — matters for stubbed-on `desktop-updates` (Q6b). Zero runtime bless logic. (Rust →
   **system-index bump**.)
2. **S2 — store + supervisor + applier**: the `/home/.shrek-system/egress` store + the root:0644 tmpfs
   projection `/run/shrek/egress/pinned` `[R2-MF-A]`, the root supervisor (bless/unbless/re-pin verbs;
   **re-pin via a baked DoT client to sealed resolver IPs — NOT `resolved`/NM/`resolv.conf`/`getaddrinfo`
   `[R1-MF1]`+`[R2-MF-C]`**; sealed `resolved.conf` `ReadEtcHosts=no`/`LLMNR=no`/`MulticastDNS=no`), and an
   **element-only** set writer (`nft add/delete element`, never `add rule`, never flush `[R2-MF-B]`);
   browser-cgroup rule is the sole insert-before-drop. Supervisor socket `SO_PEERCRED`-gated to uid 1000 +
   bless rate-limit + journaled bless/unbless with desktop notification `[R1-MF5]`. Host-oracle proof (no VT):
   bless → element added → ruleset shape; **poisoned `/etc/hosts` AND a uid-1000 NM DNS-server change both
   fail to steer the pin** (sealed-DoT proof); **non-browser uid-1000 stub = DROP, browser-scope stub =
   ACCEPT** `[R2-MF-A]`; unknown profile → fault, no element; apply-fail → baked drop + empty sets stand
   (fail-closed); `getent passwd systemd-timesync` resolves + `nft -c` parses the named-uid rule.
3. **S3 — bless UX** — **DONE (2026-09-04)**: onboarding Connectivity step + DMS Settings Connectivity
   panel (Quickshell), wired to the supervisor. Tier-B path for `weather`. **Shipped** (see §14 S3 entry):
   the unprivileged `egressd ask` socket client (the UI's fixed-argv front door); a root-written
   `/run/shrek/egress/state` read projection (closed tier + fault-kind tokens only) so the panel is
   legible without reading the `0700` store or polling the socket; **intent-first bless** + a boot
   re-resolve self-heal so a first-run weather bless made before the clock/network is up persists as
   "blessed, waiting" and completes later rather than failing dark; the ui-v2 `Egress` service + panel;
   and the ui-installer first-run onboarding step. Proven headless: `scripts/desktop-connectivity-proof.sh`
   (panel renders + real client↔supervisor round-trip) and `scripts/installer-preview.sh`.
4. **S4 — ceremony tier** — **DONE (2026-09-04)**: `web-browsing` + raw `host:proto:port` adds through the
   console ceremony (`consent.rs` reuse), with the persistence bolt-on. **Shipped** (see §14 S4 entry):
   a NEW gatekeeperd `DESKTOP-EGRESS` verb family reusing the shared SAK/VT ceremony core; a root-only
   ceremony-commit surface (tier/grammar re-validation + store lock); a
   concatenated `@raw_pinned` nft set (`ipv4_addr . inet_proto . inet_service`) keeping the element-only
   invariant for raw; reconcile survival + a `browser-up` actuator (MF-7); the `shrek connectivity` client
   + the DMS Connectivity ceremony button, raw editor, and SAK banner. Built after a Fable build-plan pass
   (GO-WITH-FIXES → 7 must-fixes folded). **Superseded transport (S6.1 fix #4):** the commit surface is no
   longer a transient `egressd confirmed-*` CLI that runs nft under gatekeeperd's caps — gatekeeperd now
   **relays** the confirmed op to the running egressd daemon over its root-gated socket (`egressd ask
   confirmed-*`), the daemon commits it, and the `geteuid==0` gate becomes the daemon's **root-peer**
   authorization. See §14 S6.1.
5. **S5 — baseline wiring — DONE (2026-09-04)**: ships the sealed `timesyncd.conf` drop-in
   (`/usr/lib/systemd/timesyncd.conf.d/10-shrek-sealed-ntp.conf`) with `NTP=` set to the **sealed literal
   Cloudflare IPs** matching `desktop-ntp`'s `@ntp_pinned` (Q6a + `[R2-MF-C]`; no name, no resolution — the
   boot-time clock source that lets the DoT re-pin of weather/updates succeed afterward) and `FallbackNTP=`
   **emptied** so a sealed-IP failure never silently falls back to the compiled-in ntp.org *name* pool
   (unsealed egress under timesyncd's uid). This drop-in is the actual NTP enforcement: the nft floor drops
   uid 1000 only, and timesyncd runs under its own uid, so config — not the packet filter — pins it.
   `desktop-updates` **stays an inert fail-closed stub** (empty `@updates_pinned`, zero policy rules) —
   wiring it is **deferred on Q6b** (see §12). **Ordering matters:** NTP-good → DoT-repin → weather.
   *Proof* (`scripts/desktop-ntp-proof.sh`, 8/0): a 3-way anti-drift assertion (`NTP=` == `DESKTOP_NTP` ==
   baked `@ntp_pinned`), name-free + empty-fallback config checks, the `desktop-updates` inert-stub check,
   a netns liveness load of the baked set, a real SNTP round-trip proving the sealed IP is a sane clock
   source, and a live sealed-DoT weather re-pin succeeding once the clock is sane. **Not proven here (S6):**
   the cold-boot sequence live — a wrong-RTC box corrects off the sealed IPs then re-pins — under a real
   compositor.
6. **S6 — sealed-VM dogfood — DONE (2026-09-04)**: `scripts/desktop-egress-s6-vm-proof.sh` (13/0) proves the
   whole plane live in a booted, Secure-Boot/dm-verity-sealed VM under the real compositor — the two things
   netns/host-oracles **cannot** show (Q3 gate): the live browser cgroup matcher and the SAK/VT ceremony.
   Asserted: NTP cold-boot recovery off the sealed literal IPs, **name-free**, from a forward-skewed RTC (§5
   `[R2-MF-C]`); weather reaches its pinned endpoint from uid 1000 via the `/run` map + SNI/verify-name (TLS
   verifies the sealed name) while an unblessed dest **drops**; the LIVE `shrekbrowser.slice` cgroup path
   equals the baked matcher constant, a process **inside** the slice reaches the DNS stub while the same probe
   **outside drops** (rule-0), a relaunch still accepts, and `confirmed-unbless` tears the accept-pair down so
   the slice drops again; a DoT re-pin refreshes `@weather_pinned` by element (no flush); **daemon-death
   fail-closed** (`kill -9` egressd → unblessed still drops, blessed weather still reaches — replaces the
   in-VM-unsimulatable apply-fail, which the host oracle already covers, `[MF-5]`); and a raw
   `host:proto:port` added through the **console SAK ceremony** pins `@raw_pinned`. **Found + fixed four
   ship-blocking defects the oracle missed** (see §14 S6): the cgroup path/level, the unquoted nft path token,
   the bare-name `nft` spawn under `env_clear()`, and (initially) a missing `CAP_NET_ADMIN` on the
   ceremony-commit exec — the last **since superseded** by the **S6.1 fix #4 redesign** (routing the commit
   through the egressd daemon so gatekeeperd carries no `CAP_NET_ADMIN`; see §14 S6.1). Re-proven 13/0.

## 12. Open questions (for owner + Fable)

- **Q3 — DECIDED (Fable round-1):** keep the tiered split (supervisor-mediated for pinned profiles,
  console-ceremony for broad/raw). Ceremony-for-everything is rejected (manufactures approval fatigue). Sound
  *given* the `[R1-MF5]` admission rule + `[R1-MF1]` off-NSS resolution are folded — both are. (§3)
- **Q5 — DECIDED (Fable round-1):** resolve-and-pin with bounded re-pin; keep DNS closed to uid 1000. Both
  shipped endpoints verified stably pinnable. Honesty amendments (no-*uid-1000*-DNS, TLS-verify floor,
  rate-bound failure re-pin) folded. (§5)
- **Q6a — DECIDED (owner 2026-09-03):** seal `time.cloudflare.com` (single anycast, re-pinned) for
  `desktop-ntp` rather than fragile NTP-pool IPs.
- **Q6b — DEFERRED (owner-ratified 2026-09-04):** the actual layer-store / sysupdate source endpoint for
  `desktop-updates` (the host the A/B `Type=url-file` `[Source]` fetches signed root/verity/UKI images from)
  is **not set up yet**. Intended direction: its own owner-controlled domain; the **candidate** is
  `shrekos.iambu.dev` (under `iambu.dev`, which the owner controls) — a candidate, **not a commitment**.
  Until the domain + distribution channel exist, `desktop-updates` **stays an inert empty stub** (empty nft
  set = fail-closed, not accept-all). This did **not** block S5 (S5 wired only the NTP baseline). When the
  domain is stood up, wire it as a sealed profile (stable name, DoT-pinned like `weather`) — that is a
  signed-image rebuild + A/B rollout, so the final host must be chosen *before* baking.
- **Q6c — DECIDED (owner 2026-09-03):** seal **open-meteo** (`api.open-meteo.com`, keyless, no account,
  privacy-forward) as the `weather` endpoint.
- **Q7 — DECIDED (Fable round-1):** cgroup match on a stable `shrek-browser.slice`, NOT a dedicated sub-uid
  (sub-uid's full session-integration cost isn't worth it at S-scope). Stated as accident/UX containment, not
  an adversarial boundary; the ceremony text says web-browsing effectively grants broad session egress. (§7)
- **Q8 — DECIDED (Fable round-1):** baseline is **user-visible + inspectable always** (legibility, ADR-004:
  shown as "System baseline — on," with pinned IPs / last-refresh like any profile), but **revocation is
  routed through the console ceremony** with a consequence warning — not a casual off-toggle (NTP-off on a
  2012 RTC → clock-skew cert failures; updates-off on an immutable image → unpatched-forever). Reflected in §8.

## 13. Non-goals

- FDE, the owner credential, and Wi-Fi firmware enablement (own workstreams, per ADR-005 §non-goals).
- Governing **system daemon** egress beyond the named baseline (this ADR scopes the **desktop session**;
  a full host-wide egress policy is a larger, separate effort).
- Replacing the **agent/Bench** egress plane — this ADR is a *sibling*, reusing its types and `nft`
  discipline, touching none of its netns/veth/sandbox code.
- **Fixing the uid-1000-owned `/etc/hosts` store itself** — this ADR *works around* it (off-NSS resolution,
  `ReadEtcHosts=no`) so the desktop plane is sound, but the underlying defect is broader than this plane and
  is tracked separately (§14).

## 14. Changelog & cross-cutting items

**Fable round-1 (2026-09-03): GO-WITH-FIXES → all folded.** Inline `[R1-MFn]` markers show where each landed.
- **MF-1** (§3, §4, §5, §11-S2): pin resolution must not consult uid-1000-authored NSS `files`; resolve
  off-NSS + ship `ReadEtcHosts=no`; deliver pins via the `nft` named set, never by binding over the
  `/etc/hosts` symlink.
- **MF-2** (§5, §7): drop the default resolved-stub allow — it was a whole-session DNS exfil channel; the
  stub opens only inside the blessed web-browsing scope. Invariant reworded to "no *uid-1000* DNS."
- **MF-3** (§7, §10, §11-S1): two-phase enforcement — a baked **static sealed drop table** `getty@tty1`
  `Requires=`s (ADR-005 credential-path precedent), supervisor only *adds* allows; fail-closed by construction.
- **MF-4** (§7): loopback allow via `oif "lo"` (covers `::1`); IPv6 egress posture stated (pin v4-only).
- **MF-5** (§3, §11-S2): tier-B admission criterion (normative) + journaling/notification + `SO_PEERCRED` +
  rate-limit, since any uid-1000 process can invoke the supervisor silently.
- Rulings folded: **Q3** keep tiering; **Q5** resolve-and-pin (endpoints empirically pinnable); **Q7** cgroup
  match on `shrek-browser.slice`, stated as UX- not attacker-containment; **Q8** baseline visible +
  ceremony-revocable.
- Cite corrections: forward-hook policy `net_plane.rs:147` (was :162); dropped the baked `session-c1.scope`
  name (§1a); corrected the ADR-005 `Requires=`/`Wants=` characterization (§10).

**Fable round-2 (2026-09-03): GO-WITH-FIXES → three residuals folded.** Inline `[R2-MF-x]` markers.
- **R2-MF-A** (§4, §5, §7): the R1 MF-2 fold was internally broken — (a) the `oif "lo"` accept sat above
  everything and re-granted the loopback resolved stub session-wide, vacating the stub removal → added an
  explicit **rule-0 stub drop** (`{127.0.0.53,127.0.0.54}` udp+tcp) *above* the loopback accept, browser
  scope re-opens it above that; (b) with the stub gone the weather widget had no resolution path → added the
  **`/run/shrek/egress/pinned` root:0644 projection** + `--resolve name:443:<ip>` dialing so TLS still
  verifies the sealed name; (c) named the residual `resolved` D-Bus DNS covert channel as accepted (rides #3121).
- **R2-MF-B** (§7, §11-S1/S2): the R1 two-phase table split was unbuildable (two tables → blessed traffic
  dead; one table + `add rule` → allows after the drop; flush → fail-open window) → **one static baked table,
  all rules present referencing empty named sets, supervisor writes set ELEMENTS only** (no rules, no flush);
  browser-cgroup is the sole insert-before-drop; getty `Requires=`+`After=`; tty2 recovery/hole tradeoff stated.
- **R2-MF-C** (§3, §5, §6, §11): MF-1's boundary survived via the NM polkit DNS-upstream surface → re-pin now
  uses a **baked DoT client to sealed resolver IPs**, independent of `resolved`/NM/`resolv.conf`; closes the
  boundary inside this ADR. **Owner catch folded:** `desktop-ntp` is **sealed literal IPs, never resolved**,
  to break the DoT↔clock circular dependency (clock-from-NTP must precede DoT-needs-clock).
- Nice-to-haves folded: superseded §5 option-(i) detail annotated; browser stub covers tcp+udp+`.54`; sealed
  `resolved.conf` also `LLMNR=no`/`MulticastDNS=no`; cite drift `egress.rs:14,20-21`, `shrek-desktop-ready.service:25`.

**S3 — bless UX (2026-09-04): built after a Fable design pass (GO-WITH-FIXES → folded).**
- **State read model, not a socket read.** A root-written `/run/shrek/egress/state` projection (0644, closed
  `tier`/`blessed`/`pins`/`refreshed`/fault-**kind** tokens — never the free-text fault reason) is the panel's
  legible view, refreshed at *every* store mutation (supervisor + the resolve/apply CLIs), so a timer/CLI
  re-pin can't leave it stale. Keeps the socket mutation-only (no single-accept-loop poll contention) and the
  `0700` store unread. `BlessTier` carries the three-way tier so S4's ceremony distinction is already encoded.
- **`egressd ask` client.** The unprivileged uid-1000 socket front door (fixed argv, no shell) the DMS panel +
  onboarding exec. Convenience, not capability — the daemon re-validates everything; the client only rejects
  obvious garbage locally and uses the oracle-gated socket path (compiled out of the shipped build).
- **Intent-first bless + boot self-heal.** The supervisor now records the durable bless *before* resolving, so
  a resolve failure (a first-run bless before the clock/network is up) leaves the profile legibly "blessed,
  pin-deferred" instead of silently unblessed; boot `reconcile` re-resolves a blessed-but-pinless one-click
  profile — the only place a root-side retry belongs (a UI/socket retry would starve the owner's own clicks
  against the rate limiter). DoT resolution is an injectable seam so this is unit-tested without a network.
- **Accepted uid-1000 residuals (by design, all visible in the events log, none crossing the boundary):** a
  malicious uid-1000 process can silently `unbless weather` (deny-direction), burn the 6/30s budget to annoy
  the owner's real clicks, or trigger ≤6 DoT re-pins/30s (bounded traffic to sealed resolvers). The weather
  dashTab / display truth binds to the root-written state file, not spoofable by uid 1000.

**S4 — ceremony tier (2026-09-04): built after a Fable build-plan pass (GO-WITH-FIXES → 7 must-fixes folded).**
- **The seam.** The SAK/VT ceremony lives in gatekeeperd; the store/apply/DoT authority lives in egressd.
  The ceremony's confirmed commit **execs** the root-only `egressd confirmed-{bless,unbless,add-raw,remove-raw}`
  (absolute sealed binary, `env_clear()`, argv from the *validated plan* not the wire), so rustls/DoT never
  enter gatekeeperd and egressd stays the single store/apply authority. Rejected: linking egressd into
  gatekeeperd (two sealed DoT clients) and a second store writer (validation would drift).
- **"Root exec = trusted" ≠ no validation (MF-1).** The destination string originates from a uid-1000 socket
  request; the ceremony proves human *intent*, not that the string is well-formed. So `confirmed-*` re-checks
  `geteuid()==0` (a uid-1000 caller can never bypass the ceremony by exec'ing the verb), `bless_tier==Ceremony`
  for `confirmed-bless` (the ceremony verb is not a `weather`/baseline front door — tier-matrix integrity),
  and the raw grammar; and it takes the store lock (MF-4) so it never interleaves with the running daemon.
- **One raw grammar (MF-2), in `shrek-policy`, used by BOTH the gatekeeperd precheck and egressd.** RFC-1123
  host or IPv4 literal, no leading `-` (argv-option injection), no whitespace/control (the host lands in the
  world-readable `/run` state — an unescaped value would be a display-truth line injection), `tcp|udp` only
  (`th dport` needs a transport header), port `1..=65535`. `raw` is the ADR-§4 flat TSV file (S2 built a dir;
  migrated). A literal host is pinned verbatim (no DoT), like `desktop-ntp`.
- **Raw enforcement (MF-2/Q2): one CONCATENATED nft set** `raw_pinned { ipv4_addr . inet_proto . inet_service }`
  + one baked rule `ip daddr . meta l4proto . th dport @raw_pinned accept`. Per-element proto+port with ZERO
  runtime rules, so the element-only `[R2-MF-B]` invariant holds for raw exactly as for the pinned profiles.
  Removal recomputes the set as the UNION of the remaining entries (MF-5 — never a per-entry element delete,
  which would kill a tuple two raw hosts share). Validated against the live kernel in an unshared netns.
- **Intent-first (MF-3) + reconcile survival (Q4).** `confirmed-add-raw` stores the triple before resolving,
  so a ceremony approved before the network is up persists as "blessed, waiting" and heals on the next
  reconcile rather than vanishing (which would force a redo of the whole SAK ceremony). Boot `reconcile` now
  re-resolves the raw union and re-asserts a blessed `web-browsing` — the ceremony tier survives reboot/A-B.
- **Browser-rule lifecycle (MF-7).** `web-browsing` enforcement is the cgroup accept pair, which nft can only
  insert once `shrek-browser.slice` exists — almost never true at bless time. A uid-1000 `browser-up` socket
  verb actuates an *already-ceremony-blessed* record at launch (installs the pair once the slice exists);
  it grants nothing not already root-blessed, so it is tier-safe on the socket. Symmetric teardown on
  `confirmed-unbless` removes the pair by handle (MF-5 — the panel can't read "off" while egress persists).
- **UX.** The DMS panel LAUNCHES the ceremony (`shrek connectivity …` → gatekeeperd `DESKTOP-EGRESS`), it
  never flips a broad/raw control itself; a "press the Secure Attention key" banner explains the console
  switch (so the first session doesn't read it as "broken"); the `/run` state file stays the sole display
  truth. The advanced raw editor is shipped in the same slice (Fable OK'd staging it later; it was small).
- **Not proven headless (the sealed-VM gate, Q3/S6):** the SAK/VT ceremony under a running compositor and
  the live browser cgroup matcher. Everything fails closed until then; the host oracle + unit tests + the
  headless render proof cover the engine, the precheck/tier gate, the raw nft, and the panel.

**S5 — baseline wiring (2026-09-04): the NTP baseline sealed; `desktop-updates` deferred.**
- **The clock source is config, not the packet filter.** The nft floor drops uid 1000 only; timesyncd runs
  under its own system uid, so the filter does not constrain it. The pin is therefore the sealed
  `timesyncd.conf.d/10-shrek-sealed-ntp.conf` drop-in: `NTP=162.159.200.1 162.159.200.123` (the same
  literals as `DESKTOP_NTP`/`@ntp_pinned`, `[R2-MF-C]` — no name, no resolution) with `FallbackNTP=` emptied
  so a sealed-IP failure never silently resolves the compiled-in ntp.org *name* pool under timesyncd's uid.
- **Three-way anti-drift.** `NTP=` (config), `DESKTOP_NTP` (shrek-policy), and the baked `@ntp_pinned` set
  must all name the same two IPs; the proof asserts set-equality across all three so a future edit to one
  cannot silently diverge.
- **`desktop-updates` stays a fail-closed stub — Q6b deferred (§12).** No wiring this slice: empty
  `@updates_pinned`, zero policy rules. The endpoint (candidate `shrekos.iambu.dev`, not committed) does not
  exist yet; wiring it is a future signed-image rebuild.
- **Proof (`scripts/desktop-ntp-proof.sh`, 8/0):** static config↔policy↔nft consistency (name-free `NTP=`,
  empty `FallbackNTP=`, inert `desktop-updates`); a netns liveness load of the baked table; a real SNTP
  round-trip proving the sealed IP is a sane clock source; and a live sealed-DoT `weather` re-pin succeeding
  once the clock is sane (the NTP-good → DoT ordering). Config/docs/bash only — no Rust, no system-index bump.
- **Not proven here (the sealed-VM gate, S6):** the cold-boot sequence live — a wrong-RTC box corrects off
  the sealed IPs with zero resolution and only then re-pins weather — under a real compositor.

**S6 — sealed-VM dogfood (2026-09-04): the whole plane proven live; four ship-blocking defects found + fixed.**
- **Proof (`scripts/desktop-egress-s6-vm-proof.sh`, 13/0).** A booted Secure-Boot/dm-verity VM under the real
  Sway/Quickshell compositor (reuses the dogfood docker/qemu scaffold + the bench-consent SAK cue-loop). A
  marker-gated guest probe (`dogfood-egress-probe`, dispatched by `dogfood-persist-probe`) drives every leg and
  the host tallies `SHREK-DOGFOOD S6 <check>=` serial markers. Proves the Q3 gate — the live browser cgroup
  matcher and the SAK/VT ceremony — plus NTP recovery, weather reach/drop, repin, revocation, and daemon-death
  fail-closed. The probe asserts the LIVE cgroup path equals the baked constant *before* trusting accept/drop.
- **The finish line caught what the netns/host oracles structurally could not — four real defects, all fixed:**
  1. **cgroup path/level** (`confirmed.rs::browser_cgroup`): shipped as `…/shrek-browser.slice` at level 2, a
     path a real `--user` launch never produces → the matcher was dead. Fixed to the measured
     `user.slice/user-<uid>.slice/user@<uid>.service/shrekbrowser.slice` at **level 4** (un-hyphenated name; §7).
  2. **unquoted nft path** (`apply.rs`): the cgroup path was passed to nft as a bare token — the lexer chokes on
     `user@1000` (`syntax error, unexpected number`), so `browser-up` always returned `apply-failed`. Fixed:
     pass it QUOTED. Verified against the live kernel (bare fails, quoted loads).
  3. **bare-name `nft` spawn** (`apply.rs::ShellNft`): `Command::new("nft")` fails under gatekeeperd's
     `env_clear()` ceremony-commit exec (empty PATH → ENOENT). Fixed to the absolute `/usr/sbin/nft` (the doc
     comment already claimed the absolute path; the code didn't match).
  4. **missing `CAP_NET_ADMIN`** (`gatekeeperd.service`): the ceremony-commit exec of `egressd confirmed-*`
     edits the ROOT-netns table, but the broker's bounding set deliberately omitted net caps, so raw ceremony
     adds failed with "Operation not permitted." Initially granted `CAP_NET_ADMIN` (marginal — CAP_SYS_ADMIN
     already makes the broker root-equivalent). **This grant was SUPERSEDED by the S6.1 fix #4 redesign below**
     — the commit no longer runs in a gatekeeperd child at all, so the broker carries no net cap. The S4 oracle
     ran the commit as unrestricted root and missed the original defect.
- **`[MF-5]` assert-set (per the S6 Fable build-plan pass):** dropped the in-VM-unsimulatable `apply-fail` leg
  (destroying the baked table to induce it would demolish the very floor the assert claims stands — the host
  oracle already proves it) and replaced it with a **daemon-death fail-closed** leg; added **revocation** and
  **double-launch** legs and a `/run`-map == `@weather_pinned` set-equality check. **`[MF-4]`** skews the RTC
  **forward** (a past skew hits systemd's behind-epoch clamp = vacuous) but TLS-safe, with an explicit qemu NIC.

**S6.1 — ceremony commit routed through egressd IPC; gatekeeperd drops `CAP_NET_ADMIN` (fix #4, owner-ratified 2026-09-04).**
The S6 finish-line fix #4 granted the broker `CAP_NET_ADMIN` so its forked `egressd confirmed-*` child could
edit the ROOT-netns table. That was the *minimal* correct fix but the *wrong shape*: it made gatekeeperd a
transitive nft mutator and widened its cap posture. **The redesign removes the transient CLI mutation path
entirely.** On a confirmed ceremony gatekeeperd now execs `egressd ask confirmed-*` — a **capless socket
client** — which relays the op to the already-running egressd daemon; the daemon (the sole nft mutator,
already `CAP_NET_ADMIN` via `egressd.service`) does the store write + apply **in its own process**.
- **New trust boundary:** the daemon gains privileged `confirmed-{bless,unbless,add-raw,remove-raw}` socket
  verbs, `authorize`d on a **root peer** (`peer_uid == 0`) — the direct analog of the old CLI's `geteuid()==0`
  gate. A uid-1000 peer can never reach them, so the socket does not become a second front door for the
  ceremony tier; the daemon still re-validates tier + re-parses the raw triple through the one sealed grammar
  (defense in depth). The root peer's request line is admitted up to `REQ_MAX_PRIV` (raw triple ≤300); every
  non-root peer stays held to the tight `REQ_MAX`.
- **Net effect:** `gatekeeperd.service` `CapabilityBoundingSet` loses `CAP_NET_ADMIN`; **no transient process
  ever edits the desktop table under the broker's caps**; and the **MF-4 cross-process race is deleted** — the
  daemon is single-writer, so the store lock no longer arbitrates a broker-forked committer against the daemon.
  The `egressd confirmed-*` CLI subcommands are gone; `confirmed.rs` keeps only the shared reconcile engine the
  daemon handlers and boot `reconcile` call.
- **Rate-limiter exemption (a real regression the first re-run caught — 12/1 → fixed → 13/0):** the daemon
  rate-limits every *uid-1000* mutating attempt (6/30s, anti-oracle). Routing the commit through the socket
  first put the ROOT ceremony verbs under that SAME shared budget — so in a busy 30s window (weather
  bless/repin + `confirmed-bless` + two `browser-up`s) the follow-on `confirmed-unbless` was silently
  `ERR rate-limited`, the accept-pair never tore down, and `revoke-drop` failed. Fix: **privileged
  (root-peer) commits bypass the limiter** — they are unreachable by uid-1000 (no oracle vector) and already
  physically throttled by the SAK/VT ceremony; rate-limiting them is an MF-5 hazard, not a protection. The old
  CLI path took the store lock directly and never touched the limiter, so this preserves prior behavior.
- **Re-proven:** the S6 sealed-VM proof re-run is **13/0** — `revoke-drop` and `sak-raw` now exercise the IPC
  relay end to end (the probe's setup/revoke legs use `egressd ask confirmed-*`; `sak-raw` drives the real
  gatekeeperd ceremony → `egressd ask confirmed-add-raw` → daemon commit). Rust changed → both repos push.

**CROSS-CUTTING SECURITY DEFECT (filed separately as mycelium #3121, NOT owned by this ADR):**
`image/overlay/etc/hosts` is a baked symlink into `/home/.shrek-system/hosts`, which
`image/overlay/usr/lib/shrek/hosts-seed` **chowns to uid 1000 on every boot**. Because glibc NSS resolves
`files` before DNS, uid 1000 thereby holds **name-resolution authority over every root daemon on the box**
(not just this plane) — it can steer any root service's hostname lookups to arbitrary IPs. This ADR only
*defends its own* pin path against it (MF-1). The root fix (revisit the bench-step-4 decision to chown that
store to uid 1000; give root a merged/authoritative hosts composition; make `shrek-connect` supervisor-
mediated) belongs to its own security work item. Related surface to audit at the same time: the NM polkit
rule (`image/mkosi.postinst:190-195`) letting the seat0 user modify system NetworkManager connections
(incl. resolved's upstream DNS) without admin.
