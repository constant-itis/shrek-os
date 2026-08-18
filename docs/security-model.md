# Shrek OS — Security model

> The wall is a theorem, not a hope. This doc names the primitive that proves each line.

This is the companion to [`threat-model.md`](threat-model.md). Where that doc enumerates
**assets (A1–A7), adversaries (ADV-1–12), boundaries, and OPENs**, this one maps each threat
to the **enforcing primitive and the layer that carries it**, and — per the Phase-0 decision
to resolve inline — **takes positions on the load-bearing OPENs as amendments** to
[`architecture.md`](architecture.md) / [`isolation.md`](isolation.md). Amendments are marked
`⇒ AMENDS` and collected in §9 for review before propagation.

Reading order: §1 the two theorems · §2 the enforcement stack · §3 threat→primitive table ·
§4–§8 the amendments (the spine) · §9 amendment summary · §10 residual register.

---

## 1. The two theorems

Everything here defends exactly two properties. If either inverts, the model is void.

```
THEOREM 1 — the wall.        semantic authority ≤ data authority
  A protected byte is unreachable by an agent BOTH directly (VFS) AND by inference
  (embedding/summary/index/relationship). Enforced by KERNEL primitives on every subject
  that could otherwise leak it — including swampd itself. Never depends on model judgment.

THEOREM 2 — the tripwire is not the wall.
  Semantic/DLP classification is ADVISORY. A false negative is a FAILED WARNING, never a
  LEAKED SECRET — because anything whose leakage would be catastrophic lives behind THEOREM 1,
  not behind the classifier.
```

The whole security posture is: **make the set of "catastrophic if leaked" data equal to the
set behind the deterministic wall.** Then classifier accuracy is a quality metric, not a
security boundary. §4 (A4) and §5 (swampd) exist to make Theorem 1 actually hold; §7 (DLP,
fail-closed) exists to keep Theorem 2 from silently becoming the wall.

---

## 2. The enforcement stack

Each layer, the primitive it uses, what it *guarantees*, and — critically — what it does
**not** (so no layer is trusted for more than it delivers).

| Layer | Primitive | Guarantees | Does NOT guarantee |
|---|---|---|---|
| Boot trust | signed UKI (Shrek key) · dm-verity · MOK Secure Boot · TPM PCRs | base image + kernel are authentic and unmodified at boot | writable-partition integrity (§4); anti-rollback unless §8/C1 |
| Sealed policy | static baked into the verity image + mutable grants under fs-verity with a TPM NV monotonic counter (§4) | the *definition* of every wall is authentic and rollback-fresh | correctness of a policy the operator authored (right-sizing = `agents.md`) |
| Agent wall | Landlock + seccomp (per-process, incl. swampd-as-subject) | named domains are unreachable to the confined process, kernel-enforced | survival of a kernel LPE at T0/T1 (N1) |
| System MAC | AppArmor (path-based) | belt-and-suspenders system confinement | the primary agent wall (that is Landlock) |
| Isolation tier | ns/cgroups (T0) · nspawn (T1) · gVisor (T2) · KVM microVM (T3) | blast-**wall** strength = f(tier); floor is downward-forbidden | the blast-**radius** (that is caps/mounts, §2 isolation.md) |
| Capability radius | virtio-fs/bind mount-set + tap+nftables egress | only profile-granted paths/hosts are reachable inside the box | that the granted profile is itself safe (§3, ADV-1) |
| Broker | gatekeeperd re-check (floor + caps⊆profile), privilege-separated | an unprivileged/compromised agentd cannot widen a sandbox | anything if it reads policy from a mutable source (§4/B3) |
| Semantic tripwire | DLP classifier at the egress chokepoint | best-effort warning on readable-but-sensitive export | prevention of a determined evader (N8) — advisory by construction |
| Provenance | hash-chained append-only log + anchored head | tamper-evidence + forensic reconstruction | confidentiality unless read-ACL'd (§8/F2); prevention |

The ordering is deliberate: **each row is only trusted for its own guarantee, and the row
below it does not inherit the row above's authority.** A microVM (isolation tier) is trusted
for wall strength and *nothing about the radius* — the radius is the row below it.

---

## 3. Threat → enforcing primitive

Every adversary and boundary from threat-model.md, mapped. **Status:** `CLOSED` (a primitive
fully answers it), `MITIGATED` (bounded, residual named), `AMEND` (needs a decision in §4–§8),
`ACCEPTED` (non-goal, threat-model §9).

| Threat | Enforcing primitive | Layer | Status |
|---|---|---|---|
| ADV-1 malicious/subverted agent | caps⊆profile (2×) + wall denies A1 mount; egress = only channel | radius + broker | MITIGATED (→ §7 egress/DLP) |
| ADV-2 sandbox tier/caps escape | tier matrix + downward-forbidden floor; caps tier-independent | isolation + radius | MITIGATED (→ §6/B1 trust-band) |
| ADV-3 compromised swampd | swampd is a **subject** of Landlock, default-deny read scope | agent wall | CLOSED for A1 bytes (→ §5) |
| ADV-4 supply-chain on layer/image | Verity + signature + reproducible-build pin | sealed policy + update | MITIGATED (valid-but-malicious = N2) |
| ADV-5 evil-maid / boot | signed UKI + dm-verity + MOK + TPM | boot trust | MITIGATED (→ §4 writable parts, §8/C1 rollback) |
| ADV-6 malicious app | Flatpak portals + routing rule; native ⇒ mandatory sandbox | isolation | CLOSED (§8/D1 — no unconfined path) |
| ADV-7 DLP evasion | *by design advisory*; wall holds the catastrophic set | tripwire | ACCEPTED (N8) + §7 placement |
| ADV-8 confused-deputy via gatekeeperd | independent re-check from **sealed** policy source | broker | CLOSED via §4/§6 (B4 rename race → MITIGATED) |
| ADV-9 indirect prompt injection | wall ignores agent intent; caps bound the blast | radius + wall | MITIGATED (→ §7, same channel as ADV-1) |
| ADV-10 warm-pool contamination | reset-to-clean between tenants; no cross-band reuse | isolation | MITIGATED (§8/B5) |
| ADV-11 provenance-log adversary | append-only + anchored head + read-ACL | provenance | MITIGATED (§8/F1, F2) |
| ADV-12 degrade-to-unsafe | **fail-closed** wall; supervised gatekeeperd | broker | wall CLOSED / availability MITIGATED (§7) |
| A4 policy-store integrity | sealed static (image) + counter-fresh mutable + guarded grant API | sealed policy | CLOSED (§4) |
| A5-disc reference leakage | *boundary of the byte-wall*; scope `discover:false` honestly | — | ACCEPTED (§8/A5-disc) |

The rest of this doc resolves every `AMEND`.

---

## 4. AMENDMENT — the sealed policy plane (A4, the spine)

**Problem (threat-model §6.4, §7.3):** the *definition* of every wall — agent capability
profiles, Landlock/AppArmor rulesets, the (trust×caps) matrix + floor, `oniond` trust roots,
nftables egress lists, swampd's indexable-tree allow-list — has, today, weaker integrity than
the walls it defines. dm-verity seals only the base root; this policy lives on writable
`/home`,`/var`. An attacker who writes there **reconfigures the wall without defeating a single
enforcement primitive.** This is cheaper than every inference attack in the threat model
combined.

**Decision — split policy into two integrity classes, each protected by the primitive that
actually fits it, neither on a plain writable partition:**

```
STATIC POLICY  (identical for all installs of a Shrek version — version-static)
  Landlock ruleset templates · AppArmor profiles · the (trust×caps) matrix · the floor table
  · oniond trust roots · swampd indexable-tree allow-list template · base agent-profile schemas
  ⇒ BAKED INTO THE bootc IMAGE, under the dm-verity sealed root. This is what architecture.md
    §3's routing rule ALREADY dictates — "base + security-critical + boot-path → baked into the
    IMAGE" — and static policy is the most security-critical, version-static bytes in the system.
    NOT a confext: confext is the routing rule's OPTIONAL-composable middle row, lives under
    writable /var, and is deletable/unmergeable — the wrong integrity class for the wall's
    definition. Changing static policy = a signed image update, verified by the boot chain (§2).
    Enforcers FAIL CLOSED on absent/unverifiable static policy: a missing matrix or floor table
    ⇒ no sandbox construction, ever (never a permissive default).

MUTABLE POLICY  (per-machine, per-user: the grants the operator actually made)
  "agent `coder` granted read ~/Projects/foo + net github.com" · per-agent trust-band bindings
  ⇒ a DEDICATED policy directory (NOT under $HOME). Each grant file is fs-verity-sealed
    (immutable once written: write-new → FS_IOC_ENABLE_VERITY → atomic swap). But fs-verity gives
    per-FILE integrity, NOT set-level integrity — so the load-bearing object is a signed MANIFEST
    of expected {path → verity-digest} carrying a POLICY GENERATION number, and freshness is
    anchored to a TPM NV MONOTONIC COUNTER: every grant/revoke bumps the counter, the manifest is
    bound to the counter value by a TPM-RESIDENT, NON-EXPORTABLE keyed-hash (or the NV index is
    write-gated by a TPM policy so ONLY the grant path can update the binding). The manifest is
    signed by a key of the same custody class — the binding/signing key is NEVER a file on /var,
    or the offline attacker (ADV-5) re-mints HMAC(old_manifest ‖ current_counter) around the
    counter and anti-rollback is void. gatekeeperd verifies manifest-against-counter on EVERY
    policy load — NOT at boot — and ACCEPTS ONLY generation == counter. Grant/revoke updates the
    pair with FIXED ORDERING: REVOKE = bump-counter-then-write-manifest (the old broader manifest
    must die instantly); GRANT = write-manifest-then-bump, rolling forward if it observes
    gen == counter+1. A crash mid-update leaving manifest ≠ counter FAILS CLOSED (agents paused),
    repaired only through the grant path. Counter writes happen ONLY on grant/revoke, never on
    load (loads are reads). TPM-absent ⇒ §8/AS3 documented-degrade (software counter, lower
    assurance, no silent claim of a guarantee it can't back).

WRITE PATH  — the grant API is the ONLY mutator of authority, and is itself guarded:
  - UNREACHABLE from any sandbox at any tier: its socket is a PATHNAME socket (never abstract-
    namespace) that is never a member of any mount-set, and NO sandbox at any tier shares the host
    network namespace (dedicated netns even under C-net; the tap crosses, abstract sockets do not).
    Broker-side fds are CLOEXEC. A sandbox cannot even name it.
  - A grant/revoke REQUIRES operator confirmation over the TRUSTED-PATH channel (§8/TP1), so an
    injected agent (ADV-9) that merely *asks* — or drives the UI — cannot self-widen the wall.
  - Every grant/revoke is a first-class provenance event (§8/F1), logged with the counter value.
```

**Why this closes the threat:**

- Offline tamper of a grant file breaks its fs-verity digest → detected at read. Deleting,
  adding, or reordering grant files breaks the signed manifest → detected at load. **Rollback**
  — the offline attacker who snapshots the policy dir *and* the anchor while a broad grant exists
  (or before a revocation) and later restores both — is caught because the restored manifest's
  generation is BELOW the TPM NV counter (which cannot be decremented on-device) AND the old
  manifest cannot be re-bound to the current counter because the binding key never leaves the
  TPM. *(fs-verity plus
  a boot-sealed root alone would MISS this — seal/measure are confidentiality-gated-by-PCR and
  attestation, not freshness. The monotonic counter checked every load is the primitive that
  actually delivers anti-rollback.)*
- **gatekeeperd loads BOTH the granted profile and the matrix/floor from these sealed sources**
  (static from the verity image, mutable via the counter-checked manifest), independent of
  agentd — closing OPEN B3. The independence is only as *fresh* as the counter check, which is
  why the counter is load-bearing for B3 too.
- The single write path is privileged, sandbox-unreachable, trusted-path-gated, and audited.

```
⇒ AMENDS architecture.md — static policy is BAKED INTO THE bootc IMAGE per the EXISTING §3
  routing rule (no routing change; the rule already covers it). ADD to the §1 layer stack that
  policy is AUTHORITY sealed with the base, and mutable grants are counter-anchored state, NOT
  ordinary writable state. Cross-refs isolation.md §7 (gatekeeperd source), base-selection.md
  (dm-verity, TPM NV monotonic counter).
```

**Residual:** correctness of a policy the *operator* authored (an over-broad grant) is
unchanged — that is right-sizing, delegated to `agents.md` (N7). Sealing guarantees the policy
is *authentic and unmodified*, not *wise*.

---

## 5. AMENDMENT — swampd is default-DENY (Theorem 1's keystone)

**Problem (threat-model §7.4):** architecture.md §5 correctly makes swampd a *subject* of the
wall (Landlock the daemon so protected bytes never enter its address space) — but the syntax it
shows (`deny ~/Vault/**`, §6 `denied: [...]`) is a **deny-list, i.e. default-allow.** A newly-
created secret directory not on the list is silently indexed. That re-introduces the exact
misconfiguration class §5 claims to eliminate, and it makes Theorem 1 depend on the deny-list
being exhaustive forever.

**Decision:**

```
swampd's Landlock ruleset is generated ALLOW-LIST-FIRST, default-deny.
  swampd is granted read ONLY to an explicit set of indexable trees. The DEFAULT allow-set
  TEMPLATE (e.g. ~/Projects, ~/Documents, ~/Downloads) and the never-indexable exclusion (the
  human-only domains) are STATIC POLICY (§4). Per-machine additions (e.g. "also index ~/Music")
  go through the §4 MUTABLE grant path — counter-anchored, trusted-path-gated — NEVER a plain
  writable config file. Everything else — including any directory created after the ruleset was
  built, and every human-only domain — is denied by construction.
  The human-only domains are NEVER members of the allow set; they cannot be added by any path.
```

Deny-list examples in architecture.md §5/§6 are **illustrative of the intent** ("agents don't
see Vault"), but the *generated ruleset that the kernel enforces on swampd is default-deny.* A
new `~/NewSecrets/` is unreadable to swampd the instant it exists, with zero config change —
the property architecture.md §5 promised ("stronger than don't-index-these-paths") is now
actually delivered.

```
⇒ AMENDS architecture.md §5 — restate swampd confinement as default-deny allow-list, not
  deny-list. The allow-set TEMPLATE + the human-only exclusion are STATIC POLICY (§4, signed);
  per-machine additions use the §4 mutable grant path. AMENDS §6 — the `denied:` example is
  illustrative; the enforced form is an
  allow set. (Agent profiles in §6 keep `denied:` as human-readable intent but COMPILE to a
  default-deny Landlock ruleset — same principle applied to agents, not just swampd.)
```

**Residual — reference leakage (A5-disc), unchanged by this fix and honestly bounded:** swampd
default-deny stops A1 *bytes* from being indexed. It does **not** stop a *readable* file in an
allowed tree from *quoting or naming* A1 ("vault hint: see Identity/answers.txt"). The byte-wall
protects bytes, not references to them held elsewhere. See §8/A5-disc for the honest scoping of
`discover:false`.

---

## 6. AMENDMENT — the trust-band and the broker recheck (B1, B3, B4)

The isolation matrix and floor are only as sound as the **trust band** they index on, and the
recheck is only as sound as its **source** and its **resolution primitives**.

**B1 — trust-band inference must derive from integrity, and fail high.** isolation.md §9 defers
*how* agentd derives the band to `agents.md`; the security requirement this doc pins:

```
The trust band is derived ONLY from an integrity-checked source:
  - a signature over the code/manifest by a trusted authority (T-first/T-pinned), OR
  - a measured provenance record in the sealed log (§4), NEVER an attacker-writable label.
UNKNOWN or UNVERIFIABLE provenance ⇒ T-hostile.  (fail-safe HIGH, per isolation.md §5.2:
  escalation is upward-only; the floor may only ever be raised by uncertainty, never lowered.)
```

This makes provenance *spoofing* (threat-model §7.2, the #1 soft spot) a non-event: forging
"T-first" requires forging a trusted signature (= N2, out of scope for the wall) or writing the
sealed log (= defeating §4). Absent both, unverifiable code lands at T-hostile with a T2 floor.

**B3 — recheck source independence: closed by §4.** gatekeeperd reads granted profile + matrix
+ floor from the sealed policy plane (§4), not from anything agentd or an agent can write. The
two checks are independent by construction.

**B4 — mount-set TOCTOU: pin the subtree root, resolve beneath it.** A symlink swap OR a rename
race on a writable parent component would land protected bytes. (Rename race: `~/Projects`
writable while the grant is `~/Projects/foo` — resolve `foo`→inode X, then `rename(foo,tmp);
rename(evil,foo)` before use. `RESOLVE_NO_SYMLINKS` stops the symlink swap but NOT the rename.)

```
⇒ gatekeeperd pins the granted-subtree ROOT as an O_PATH directory fd, then resolves every
  component BENEATH that pinned fd (openat2 RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS) and mounts
  FROM the resulting fd — never re-resolving a pathname through the mount namespace.
  The INITIAL pin of the subtree root is itself a component-wise RESOLVE_NO_SYMLINKS walk from a
  pinned trusted-ancestor fd — so even the root fd is acquired without a symlink-swap window.
  INVARIANT: a granted mount source has NO attacker-writable parent component — policy dirs and
  mount roots live where only the sealed write path or the owning principal can rename them.
  Under that invariant the rename race has no writable component to win. Residual (accepted): a
  grant whose root lies INSIDE another sandbox's writable subtree can suffer cross-grant content
  poisoning WITHIN that granted authority (not a wall breach — landing A1 still needs a symlink,
  which the symlink-free walk denies); such overlapping grants are flagged at grant time.
  (Exact openat2 flag behavior is kernel-version-dependent — verify at implementation, incl. the
  initial pin; the REQUIREMENT is resolve-beneath-a-pinned-root + non-writable-parents.)
```

```
⇒ AMENDS isolation.md §7 — step 3 (recheck) reads from the sealed policy plane (§4); add a
  step: mount construction pins the subtree-root fd and resolves beneath it, never re-resolving
  a pathname from the mount namespace.
  AMENDS isolation.md §9 — the trust-band OPEN is scoped: whatever agents.md chooses, it must
  be integrity-sourced and fail to T-hostile on doubt.
```

---

## 7. AMENDMENT — the egress channel, DLP placement, and fail-closed (ADV-1/9, E1, G1)

These three are one story: what happens at the **boundary between readable data and the
outside**, and what happens when the enforcer is **down**.

**Egress is the residual channel (threat-model §7.5).** A subverted or injected agent (ADV-1/
ADV-9) with `C-net` can push any *readable* data to an allowed host. The wall already denied A1,
so this is never A1 — but readable-sensitive data (PII in `~/Projects`) rides the allow-list.
This is intrinsic: an agent that may read X and may reach host H can send X to H. The controls
are (a) minimize the channel, (b) place the tripwire where the workload can't defeat it.

**E1 — DLP runs OUTSIDE the workload, at the egress chokepoint.**

```
⇒ The DLP/semantic tripwire runs in the network broker / gatekeeperd egress path, NOT inside
  the sandbox. The workload cannot disable, blind, or feed it. It inspects export at the
  nftables/proxy chokepoint that already realizes C-net (isolation.md §6). In-sandbox DLP is
  forbidden — a hostile workload would neutralize it first.
```

nftables egress fragility (shared-CDN IPs, DNS rebinding, SNI spoofing) is real: the allow-list
should be enforced at an **authenticated egress proxy** for hosts where IP-level rules are
insufficient, not resolved-IP alone. (Implementation detail deferred to `update-model.md`/the
Phase-5 net plumbing; the *requirement* — no raw IP allow-listing for security-critical egress —
is fixed here.)

**Ciphertext blind spot (honest scope).** Placing DLP at the chokepoint stops a workload from
*disabling* the tripwire; it does NOT let it read *encrypted* egress from any **non-cooperative**
workload. Any workload with `C-net` that brings its own TLS stack — hostile code at T0/T2 as much
as a T3 guest — declines a MITM proxy's cert and tunnels its own TLS to an allowed SNI, or
SNI-passthrough leaves the (spoofable) SNI rule as the only control. So plaintext inspection
holds ONLY for cooperating workloads that accept the proxy CA; for any non-cooperative C-net
workload, DLP sees ciphertext/metadata only. Encrypted exfil past an allowed host is accepted
residual (N8); the wall, not the tripwire, holds the catastrophic set.

**G1 — the two planes: OS fails OPEN, the wall fails CLOSED.** threat-model §7.7 caught the
latent inconsistency: architecture.md §9 guarantees the OS survives swampd/agentd being down,
but never says agents *cannot run* when the broker is down. An implementation could satisfy §9
and still let agents run unconfined during a gatekeeperd outage — fail-open on the wall.

```
⇒ AMENDS architecture.md §9 — split the guarantee into two planes:

  AVAILABILITY PLANE  (fails OPEN):  boot · login · desktop · FS · net · apps · shell · dev.
    Works fully with swampd/agentd/gatekeeperd stopped. Only ENHANCED capability (semantic
    search, agents, provenance enrichment) disappears. (unchanged — this IS §9's point.)

  AGENT-EXECUTION PLANE  (fails CLOSED):  `shrek run` / any sandbox construction / any agentd
    action REQUIRES a live gatekeeperd serving from the sealed policy plane (§4). If gatekeeperd
    is unavailable, sandbox construction is IMPOSSIBLE — there is NO unconfined fallback, ever.
    Degrading enhanced capability must never degrade the wall.
```

The planes are independent: killing the agent stack removes agent *features* (fail-open for the
human's own OS) while making agent *execution* impossible (fail-closed for the wall). Both at
once, no contradiction. This is the security reading of the critical-failure test.

**But fail-closed is itself a DoS surface (ADV-12), and must be supervised.** Requiring a live
gatekeeperd for agent execution means an attacker who can crash-loop it denies the *entire* agent
plane — a *stronger* denial than the fail-open it replaces. This is accepted for the WALL (a
denied agent leaks nothing) but is a real AVAILABILITY residual, not "closed." gatekeeperd MUST
run under a supervised, rate-limited restart contract — **supervised by systemd** (already in the
TCB; not a bespoke supervisor daemon that would be new privileged surface) — recovering from the
sealed state (§4) without operator intervention. A crash degrades to "agents paused," never
"agents unconfined." The one exception is the §4 wedge (a crash mid grant/counter update leaving
manifest ≠ counter): that is "agents paused pending operator repair through the grant path," not
silently dead — restart re-loads existing sealed state and mints no grants, so it self-heals every
case *except* a half-written transaction. Note the §4 interaction: per-load TPM counter checks put
TPM latency / dictionary-lockout on the grant/verify path — the policy NV index should be
DA-exempt (TPMA_NV_NO_DA) and `/dev/tpm*` is never in any mount-set, so a host-side actor cannot
induce lockout as a DoS. ADV-12's wall half is closed; its **availability half is MITIGATED** (§10).

---

## 8. AMENDMENT — remaining OPENs resolved

Shorter decisions on the rest; each closes or explicitly scopes a threat-model OPEN.

**D1 — "trusted native" is signed, not unconfined.** architecture.md §1 lists "trusted native"
beside Flatpak/OCI with no confinement contract (ADV-6).

```
⇒ AMENDS architecture.md §1 — "trusted native" ≡ native code SIGNED by a Shrek-trusted authority
  AND run under a MANDATORY capability profile + at least a Tier-0 Landlock/seccomp sandbox.
  "Trusted" means the SIGNATURE is trusted (provenance), NOT that the code runs unconfined.
  There is no unconfined execution path on Shrek. A native app with no profile = no run.
```

**C1 — anti-rollback vs recovery.** bootc keeps the prior A/B image for recovery; nothing stops
a forced downgrade to a validly-signed, known-vulnerable version (threat-model §6.7).

```
⇒ AMENDS base-selection.md — a monotonic security version counter (SVN) in a SEPARATE TPM NV
  index of the same primitive class as §4's policy counter (NOT the same index — a policy grant
  must never bump the image floor). Boot REFUSES any image below the current SVN floor. THREE
  rules make this coexist with bootc's A/B safety net instead of bricking it:
   (1) The SVN floor advances ONLY when a new slot commits greenboot-HEALTHY — never at install
       or first-boot. So the last-known-good A/B slot is always ≥ floor, and an automatic
       health-check rollback (the thing A/B exists for) always targets an at-or-above-floor image.
       A headless box that fails a staged update rolls back normally; it does not brick.
   (2) Ordering: commit the new slot as default FIRST, then bump the floor. A crash between the
       two is safe (floor still allows the committed slot); the reverse order would briefly strand
       the not-yet-default new slot below floor.
   (3) Recovery may repair/re-install but MUST land at ≥ the current SVN — it boots a CURRENT-SVN
       recovery image, never an old one. Evil-maid (ADV-5) HAS physical presence, so
       "human-attested recovery" alone would not stop a physically-present rollback; forbidding
       recovery from lowering the floor is what actually bounds ADV-5's rollback goal. Hence ADV-5
       stays MITIGATED (§10), not closed — the floor is monotonic even under a physical actor.
  Availability residual (named): once the floor advances, the OLD slot is below floor, so
  post-commit corruption of the current slot (bitrot / later verity failure) can leave no bootable
  LOCAL image — recovery then needs current-SVN media/network per rule (3). Bounded, consistent
  with ADV-5 MITIGATED.
```

**TP1 — the trusted-path grant channel.** §4's grant API requires "operator trusted-path
confirmation." That channel is defined HERE (it does not otherwise exist — and MOK/MokManager is
*pre-boot*, so it cannot be the grant-time channel). Without a real definition, an injected agent
(ADV-9) driving the UI could complete a grant.

```
⇒ A grant/revoke is confirmed on a COMPOSITOR-PRIVILEGED surface that no client can occlude,
  screenshot, or mimic (a secure/attention path the compositor renders, not an ordinary window).
  It displays the EXACT grant text (agent, paths, hosts) plus a per-request NONCE. Confirmation
  input is accepted ONLY from a physical-input path that no sandbox, portal-holding process, or
  RemoteDesktop/input-injection portal can synthesize. The confirmation is logged (§8/F1) with the
  nonce. An agent may REQUEST a grant; only a human at the trusted path can COMPLETE one.
  (Wayland specifics — which compositor primitive realizes the unoccludable surface + input-path
  gating — are deferred to the desktop phase; the REQUIREMENT is fixed here.)
```

**B5 — warm-pool reset.** Pooled T2/T3 instances (isolation.md §6/§9) must not leak residual
state across tenants.

```
⇒ Pooled instances are reset to a known-clean, attested baseline between tenants; an instance is
  NEVER reused across a different trust band or a wider caps profile without full teardown. A pool
  is partitioned by (trust-band, caps-class); cross-partition reuse is forbidden, not reset-and-hope.
```

**F1 — audit-log chain-head anchoring.** The hash chain (architecture.md §8) is tamper-*evident*
only if the head is anchored where the attacker can't rewrite it.

```
⇒ The log is append-only media; the chain head is periodically TPM-extended (or remote-attested
  where a collector exists). A full-log rewrite (recomputing all hashes) is then detectable because
  the head no longer matches the sealed anchor. Policy-change events (§4) are logged here.
```

**F2 — audit-log confidentiality.** `reason`/`operation`/summary fields can quote sensitive
content (threat-model §7.6 corollary).

```
⇒ The log has its own read-ACL; `shrek history`/`shrek audit` require authority ≥ the data the
  entry concerns. Log CONTENT obeys Theorem 1: an entry about an A1 artifact stores a reference/
  hash, not quoted A1 bytes. A low-trust reader must not harvest A1 material from the log.
```

**A5-disc — `discover:false` honestly scoped.** The byte-wall cannot stop a *readable* file from
naming protected material (threat-model §6.8).

```
⇒ AMENDS architecture.md §6 — `discover:false` is guaranteed for A1's OWN bytes and for A1
  metadata WITHIN swampd's scope (swampd default-deny, §5): the index never reveals A1 exists via
  A1 itself. It is NOT guaranteed against third-party readable files that reference A1. That residual
  is ACCEPTED and named (the fix would be content-level DLP on the index, which is a tripwire, not a
  wall — Theorem 2 forbids relying on it). Honest claim: "protected bytes and their own metadata are
  undiscoverable; references to them authored elsewhere in readable data are not."
```

**AS3 — TPM policy + absent fallback.** base-selection.md names TPM but not what is sealed or
what happens when it is absent.

```
⇒ AMENDS base-selection.md — specify PCR sealing for (a) the boot measurement and (b) the §4
  mutable-policy digest root and (c) the §8/C1 SVN (a separate NV index from (b)). Anti-rollback
  leans on the TPM 2.0 property that a newly (re)created NV counter initializes to ≥ the max any
  counter has ever held — which is what actually defeats destroy-and-recreate; this is
  conformance-dependent and MUST be live-verified on target TPMs (⚠ VERIFY), and the NV-index
  owner/policy auth custody must be named (who may create/define the index). TPM-ABSENT fallback:
  dm-verity + Secure Boot still hold BOOT and OFFLINE-ROOT integrity; measured-policy and
  anti-rollback degrade to a DOCUMENTED lower assurance (software counter, no hardware anchor).
  Shrek must REFUSE to advertise "sealed"/measured guarantees it cannot back when the TPM is
  absent — no silent downgrade of a claimed property.
```

---

## 9. Amendment summary (for review before propagation)

These have been **propagated** into the sibling docs (architecture.md §1/§5/§6/§9,
isolation.md §7/§9, base-selection.md), each carrying a pointer back here as the controlling
source. The `swamp.md` / `update-model.md` rows will be honored when those docs are authored.

| # | Amends | Change |
|---|---|---|
| §4 | architecture.md §1,§3; isolation.md §7; base-selection.md | **Sealed policy plane**: static policy **baked into the bootc image** (per the existing §3 routing rule, not confext); mutable grants fs-verity-sealed + **TPM NV monotonic-counter** freshness checked *every load*; single **grant API** — sandbox-unreachable, trusted-path-gated, audited; enforcers **fail closed on absent static policy** |
| §5 | architecture.md §5, §6 | swampd (and agent profiles) enforce **default-deny allow-lists**, not deny-lists; allow set is static signed policy |
| §6 | isolation.md §7, §9 | recheck reads sealed policy; mounts **pin the subtree root & resolve beneath it** (parent-rename race, not just symlink) + non-attacker-writable-parent invariant; trust band integrity-sourced, **fails to T-hostile** on doubt |
| §7 | architecture.md §9 | **two planes**: availability fails-open, agent-execution fails-**closed**; gatekeeperd **systemd-supervised restart** so fail-closed isn't a permanent DoS; DLP at the egress chokepoint (**blind on any non-cooperative C-net workload's ciphertext** — accepted) |
| §8/TP1 | (desktop phase) | define the **trusted-path grant channel**: unoccludable compositor surface, exact grant text + nonce, input no sandbox/portal can synthesize; agent requests, human completes |
| §8/D1 | architecture.md §1 | "trusted native" = signed **and** sandboxed; no unconfined execution path exists |
| §8/C1 | base-selection.md | monotonic **anti-rollback** SVN (TPM NV counter); floor **advances only on greenboot-healthy commit**; recovery repairs to **≥ current SVN**, never below |
| §8/A5-disc | architecture.md §6 | scope `discover:false` honestly to A1 bytes+own-metadata; reference leakage is accepted residual |
| §8/AS3 | base-selection.md | specify **PCR sealing** targets + TPM-absent documented-degrade (never silent) |
| §8/B5,F1,F2 | (new detail in swamp.md / update-model.md) | pool reset by (band,caps); log head TPM-anchored; log read-ACL + Theorem-1 content rule |

---

## 10. Residual-risk register (post-model)

What remains after every primitive above, restating threat-model §9 as an accepted ledger:

```
CLOSED   ADV-3 (A1 bytes via index)      — swampd default-deny subject-of-wall (§5)
         A4 policy integrity            — static baked in image + counter-fresh mutable + guarded grant API (§4)
         ADV-8/B3 broker independence   — sealed source recheck, fresh via the NV counter (§4/§6)
         ADV-6 unconfined native        — no unconfined execution path (§8/D1)
         G1 fail-open-on-WALL           — two-plane split; the wall cannot fail open (§7)

MITIGATED (bounded, residual named)
         ADV-1/ADV-9 exfil via egress   — minimized channel + out-of-band DLP (§7); the
                                          allow-listed host remains a channel by construction
         ADV-2 provenance spoof         — integrity-sourced trust band, fail-high (§6/B1)
         B4 mount TOCTOU                — subtree-pin + resolve-beneath (§6); holds under the
                                          non-attacker-writable-parent invariant
         ADV-5 rollback                 — SVN anti-rollback, healthy-commit floor (§8/C1); evil-maid
                                          with physical presence bounded, not eliminated
         ADV-12 agent-plane availability— fail-closed wall is a DoS surface; supervised restart
                                          bounds it (§7). Wall half CLOSED; availability MITIGATED
         ADV-10 pool contamination      — partitioned reset (§8/B5)
         ADV-11 log tamper/leak         — anchored head + read-ACL (§8/F1,F2)

ACCEPTED (non-goal — threat-model §9, unchanged)
         N1 kernel LPE at T0/T1 · N2 malicious-but-validly-signed layer · N3 operator vs self
         N4 microarchitectural side channels · N5 physical key/data extraction at rest
         N6 DoS of enhanced capability · N7 intent within a granted profile
         N8 confidentiality of data the operator placed OUTSIDE the wall (DLP-only, evadable)
         A5-disc reference leakage from third-party readable files (§8/A5-disc)
```

The model is strongest exactly where Theorem 1 reaches — **byte-level reachability of the
human-only domains by agents and sandboxed code** — and every remaining risk is now either a
named, bounded channel or an explicit non-goal, not a silent gap. `security-model.md`'s job was
to convert threat-model.md's OPENs into decisions; §4–§8 are those decisions.
