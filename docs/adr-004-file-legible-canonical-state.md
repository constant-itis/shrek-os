# ADR-004 — File-legible canonical state

**Status:** ✅ Accepted (2026-09-01). Names an invariant the codebase already
largely satisfies (bench records, polkit rules, net bindings are all plain files);
this ADR makes it explicit so future durable-state slices are checked against it
rather than drifting toward daemon-internal opacity.

**Context:** Shrek OS is administered by a human *and* by coding agents with
shell access. A control plane whose truth lives in daemon
memory, D-Bus state, or an opaque SQLite store forces every diagnosis through the
daemon's API surface — the modern-Linux failure mode where the actual state of the
machine is an emergent result of many abstraction layers (unit + drop-in +
generator + dbus activation + tmpfiles + udev + policykit + journal + some state in
`/var/lib/foo`). That is expensive for a human and strictly worse for an agent,
which is exceptionally good at `cat`/`grep`/`diff` over plain files and noticeably
worse at reconstructing state from fifteen layers of indirection.

Slackware is often cited for the opposite property — the system is legible from the
filesystem. The useful part of that observation is **not** the init system; it is
**file-canonical state**. We can adopt the property on our existing base without
touching the base distro.

**Positioning:** *Debian underneath, file-legible Shrek state above.* This is a
state-philosophy invariant. It is **not** an argument to revisit the base
distribution (no Slackware). Debian gives ecosystem simplicity (packages,
dependencies, hardware, security updates); this ADR adds systems-legibility
simplicity on top of it. The two are not mutually exclusive.

## Decision

### Canonical invariant

Every durable Shrek fact MUST have a **stable, human-readable, filesystem-visible
canonical representation**. Daemons (`gatekeeperd`, etc.) MAY own the write path to
enforce atomicity, validation, locking, and cross-record consistency — but daemon
memory, D-Bus state, SQLite, or any other opaque internal store MUST NOT be the sole
source of durable truth.

This corrects the naive reading ("everything is just editable files"). The point is
not that anyone hand-edits records live; concurrent writers and cross-record
consistency are real. The daemon owns the *write path* for atomicity, validation,
and locking. What the invariant forbids is the durable truth existing **only**
inside the daemon. The on-disk record remains canonical and readable; the daemon is
the mechanism that mutates it safely, not a private vault that is the truth.

### Legibility check

`shrek <thing> show` MUST be a faithful interpretation / pretty-printer of the
canonical on-disk state. It MUST NEVER become a separate authoritative computed view
that can disagree with the underlying record. If `shrek X show` and `cat <record>`
can diverge, the legibility this ADR buys is already lost.

### Applies to

As new slices land, this governs every durable-state surface:

- bench records (`/home/.shrek/benches/records`, `SHREK-BENCH` line-text)
- grants
- egress
- seed / catalog bindings
- Workshop recipes (the promote-target of a Bench — see ADR-003)
- Tool Shed / cache metadata, where durable metadata is needed
- future policy / config state

**Runtime / ephemeral state does NOT need to be file-canonical** if it is not a
durable Shrek fact (e.g. the transient `/run/shrek/...` view records that
gatekeeperd re-derives, in-memory supervisor bookkeeping, a container's live netns
state that dies on stop).

### Current conformance (why adopting this is cheap)

The codebase already votes for this invariant:

- `crates/gatekeeperd/src/bench_record.rs` — durable Bench state is a `SHREK-BENCH`
  one-line text record under `/home/.shrek/benches/records` (temp+rename write,
  fail-closed parse). Not a database.
- Capability/authz seams — polkit rules baked as files in
  `/etc/polkit-1/rules.d/49-shrek-*.rules` on RO `/etc`.
- `crates/gatekeeperd/src/net_binding.rs` — network binding records share the same
  plain-record shape.

## Review checklist

For any change touching durable state, code review SHOULD confirm:

1. **Does this slice introduce or mutate durable state?** If no, the invariant does
   not apply (ephemeral/`/run` state is exempt).
2. **If yes, is there a canonical on-disk file for it** (stable path, human-readable,
   documented shape), and is the daemon writing *through* it rather than treating an
   internal store as the source of truth?
3. **Does the corresponding `shrek … show` derive from that file?** It must not be a
   separate computed view that can disagree with the record.
4. **Is the write path atomic** (temp+rename or equivalent) and **fail-closed on
   parse**, so file-canonical does not mean corruptible-by-race?

## Consequences

- **Positive — agent legibility.** A coding agent diagnosing a problem can inspect →
  reason → change declarative state → apply → verify, instead of querying a daemon
  over D-Bus and grepping a journal for hidden generator effects. Shrek gives the
  agent better *evidence*, which is the whole point.
- **Positive — human legibility.** The same `cat`/`grep`/`diff` workflow works for a
  human administrator; the system is auditable from the filesystem.
- **Positive — cheap to hold.** We are already conformant; this is a guardrail
  against future drift, not a migration.
- **Cost — write discipline.** Every durable-state writer must go through an atomic,
  fail-closed path (temp+rename, validated parse). This is the price of allowing the
  file to be canonical without allowing races to corrupt it. Already the pattern in
  `bench_record.rs` / `net_binding.rs`; new writers must follow it.
- **Cost — no free "just cache it in the daemon."** A slice that wants to keep
  durable truth only in daemon memory or a SQLite blob must instead define a
  canonical record. This is intended friction.

## Open questions

- **Cross-record transactions.** Multi-file consistency (e.g. a grant that must land
  atomically alongside a bench record update) currently relies on the daemon
  serializing writes. If a future slice needs a true multi-file transaction, decide
  then whether that is a directory-level temp+rename swap or a small write-ahead
  marker — do not reach for an embedded database as the *canonical* store to solve
  it.
- **Schema evolution.** As record shapes grow, versioned line-text (as `SHREK-BENCH
  1` already does) vs a structured format (YAML) per surface is a per-slice call;
  the invariant only requires it be human-readable and canonical, not a specific
  encoding.
