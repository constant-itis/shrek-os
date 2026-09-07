//! egress_capability — the DATA-DRIVEN desktop-egress capability layer (ADR-009 v2, S1).
//!
//! ADR-007's `desktop-egress` vocabulary is compiled typed literals ([`crate::desktop_egress`]);
//! ADR-009 moves the *iterable* blessable space (starting with `weather`) OUT of Rust and into
//! root-authored flat MANIFESTS the owner extends without reflashing. This module is the S1 pure
//! grammar+catalog logic for that layer: it PARSES a manifest, ENFORCES the tier/port invariant that
//! makes plaintext destinations structurally unclickable, MERGES sealed+owner manifests into a catalog
//! with sealed-always-wins collision resolution, and exposes the two §4.4 isolation predicates
//! (`is_sealed_deliverable_host`, `is_system_reserved_host`). It does NO I/O: `egressd` (S2) loads the
//! two on-disk dirs and feeds parsed manifests to [`build_catalog`]; here everything is unit-testable
//! from in-memory strings.
//!
//! What is DATA vs what stays COMPILED (ADR-009 §4.1): the two baseline profiles (`desktop-ntp`,
//! `desktop-updates`) and `web-browsing` (an enforcement SHAPE — the cgroup accept-pair, not a
//! destination list) stay compiled in [`crate::desktop_egress`], unchanged — moving them to data buys
//! nothing and loses guarantees. `weather` becomes the first manifest (§4.3); it also remains in the
//! compiled table for S1 so nothing downstream destabilizes, and [`WEATHER_MANIFEST`] is the sealed-
//! source manifest fixture proving the grammar accepts it (the on-disk `/usr` file is S5 content).
//!
//! The load-bearing invariants this module ENFORCES (each surfaced as an explicit check, mirroring the
//! `desktop_egress`/`egress` idiom of a named predicate over an inferred rule-shape):
//!
//!   * `[R-MF1]` FAIL-CLOSED, WHOLE-FILE. An unknown `schema` value, an unknown key, a malformed line,
//!     a missing required key, or a duplicate single-valued key ⇒ the ENTIRE manifest is rejected with
//!     a typed [`ManifestError`] — NEVER a partial/best-effort parse. A rejected manifest ⇒ the
//!     capability is absent ⇒ no bless is possible (ADR-009 §4.3). Returns `Err`, never panics.
//!   * `[R-MF2]` THE TIER INVARIANT. `tier one-click` REQUIRES every `host` rule be `tcp` AND port
//!     `443` — checked at PARSE time (ADR-009 §4.3). A one-click manifest with any non-443 or non-tcp
//!     rule (`ip-api.com tcp 80`, or any udp) is rejected. This is the rule that makes `ip-api.com:80`
//!     STRUCTURALLY unclickable: a one-click toggle can never be authored for a plaintext/UDP host.
//!     `tier ceremony` has no port restriction (the console ceremony carries the higher-consequence
//!     authority).
//!   * `[R-MF3]` SEALED ALWAYS WINS. On a name collision between a sealed and an owner manifest, the
//!     sealed entry is kept and the OWNER entry FAULTS (ADR-009 §4.3/§4.4-layer-1): an owner manifest
//!     can NEVER shadow a sealed one, by construction. [`build_catalog`] tags every entry with its
//!     [`Source`] so the §4.4 sealed-source restriction is a data property, not a call-site convention.
//!   * `[R-MF4]` OWNER PINS NEVER REACH ROOT RESOLUTION. [`is_sealed_deliverable_host`] is true ONLY
//!     for a host of a SEALED-source, non-baseline, `deliver hosts` capability — so the S2 `/etc/hosts`
//!     composer (`hosts.rs`) lifts ONLY those, and an owner-manifest pin can never enter the file root
//!     reads (ADR-009 §4.4 STRUCTURAL layer). Fail-closed: off-catalog / owner-source / baseline ⇒ false.
//!   * `[R-MF5]` INSTALL-REFUSE OF SYSTEM-RESERVED HOSTS. [`is_system_reserved_host`] is true for any
//!     host already consumed by sealed/root machinery — enumerated from the agent egress tables
//!     ([`crate::egress`]), the provider-bind alias set ([`crate::provider_bind`]), the sealed desktop
//!     capability hosts (weather's open-meteo hosts), and the baseline profile hosts
//!     (`desktop-updates`). An owner manifest naming any such host is refusable (ADR-009 §4.4 layer 2);
//!     the ceremony-refusal WIRING is S3 — S1 supplies the predicate + tests.

use crate::desktop_egress::{is_baseline_profile, valid_raw_host, DESKTOP_EGRESS_PROFILES};
use crate::egress::{EgressProfile, Proto};

// ---- errors -------------------------------------------------------------------------------------

/// The typed, legible reason a manifest was rejected (`[R-MF1]`). Every variant carries enough to
/// render an owner-facing fault line; the loader NEVER returns a partial manifest, so a caller sees
/// either a fully-valid [`Manifest`] or exactly one of these. Not a panic — a `Result::Err`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManifestError {
    /// The `schema` line was absent, not first, or carried a value other than [`SCHEMA_ID`].
    BadSchema,
    /// A line used a key outside the closed key set (§4.3). Carries the offending key.
    UnknownKey(String),
    /// A required key (`name`/`title`/`purpose`/`feature`/`tier`/`deliver`) was missing.
    MissingKey(&'static str),
    /// A single-valued key appeared more than once (fail-closed: no silent last-wins).
    DuplicateKey(&'static str),
    /// A structurally malformed line (empty key, missing value, bad `host` arity, blank line, etc.).
    /// Carries a short reason for the fault render.
    MalformedLine(String),
    /// A `name`/`feature` token was not the closed lowercase `[a-z0-9-]` (no-leading-dash) class.
    BadToken(String),
    /// The `tier` value was neither `one-click` nor `ceremony`.
    BadTier(String),
    /// The `deliver` value was neither `hosts` nor `none`.
    BadDeliver(String),
    /// A `host` line's `<name> <proto> <port>` failed the reused raw-host/proto/port grammar.
    BadHostRule(String),
    /// `[R-MF2]`: a `tier one-click` manifest carried a host rule that is not `tcp:443`. Carries the
    /// offending destination so the fault names exactly why the capability is not one-clickable.
    OneClickNonTls(String),
    /// A manifest declared no `host` lines but `deliver hosts` — or vice-versa incoherence the loader
    /// treats as malformed rather than guessing intent.
    NoHostRules,
}

impl ManifestError {
    /// A stable, legible one-line reason — the fault text the owner-facing panel renders (never free
    /// text from uid 1000; this is root-authored, per ADR-009 §4.3). Closed phrasing.
    pub fn reason(&self) -> String {
        match self {
            ManifestError::BadSchema => {
                format!("first line must be `schema {SCHEMA_ID}`")
            }
            ManifestError::UnknownKey(k) => format!("unknown key `{k}`"),
            ManifestError::MissingKey(k) => format!("missing required key `{k}`"),
            ManifestError::DuplicateKey(k) => format!("duplicate key `{k}`"),
            ManifestError::MalformedLine(why) => format!("malformed line: {why}"),
            ManifestError::BadToken(t) => format!("token `{t}` is not lowercase [a-z0-9-]"),
            ManifestError::BadTier(t) => format!("tier must be one-click|ceremony, got `{t}`"),
            ManifestError::BadDeliver(d) => format!("deliver must be hosts|none, got `{d}`"),
            ManifestError::BadHostRule(h) => format!("invalid host rule `{h}`"),
            ManifestError::OneClickNonTls(h) => {
                format!("one-click tier forbids non-tcp/443 host `{h}` (structurally unclickable)")
            }
            ManifestError::NoHostRules => "no host rules but deliver hosts (or the inverse)".to_string(),
        }
    }
}

// ---- the manifest type --------------------------------------------------------------------------

/// The only `schema` value this loader (version 1) accepts. Any other value ⇒ [`ManifestError::BadSchema`]
/// (fail-closed: a future schema is REFUSED by an old loader, never best-effort parsed).
pub const SCHEMA_ID: &str = "shrek-egress-capability/1";

/// The bless tier a capability requires (ADR-009 §4.3). Distinct from [`crate::desktop_egress::BlessTier`]:
/// that enum covers the compiled profiles (incl. the `Baseline` always-on tier and the `web-browsing`
/// broad `Ceremony`); a *manifest* only ever declares one of these two — a manifest is never a baseline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// Grantable one-click over the uid-1000 socket. `[R-MF2]`: EVERY host rule must be `tcp:443`.
    OneClick,
    /// SAK/VT console ceremony only (higher consequence). No port restriction on host rules.
    Ceremony,
}

impl Tier {
    /// The sealed tier as the closed-set token that crosses the `/run` projection to the UI (§4.3),
    /// mirroring [`crate::desktop_egress::BlessTier::as_str`]. Closed set, never free text.
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::OneClick => "one-click",
            Tier::Ceremony => "ceremony",
        }
    }
}

/// Whether a granted capability's pins are lifted into the §5 `/etc/hosts` composition. A closed
/// two-value key (§4.3): `hosts` lifts (sealed-source only, `[R-MF4]`), `none` does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deliver {
    /// Pins lifted into host-wide resolution — ONLY honored for sealed-source manifests (`[R-MF4]`).
    Hosts,
    /// Pins never lifted into `/etc/hosts`; the consumer reaches the pinned IP directly / via `/run`.
    None,
}

/// One `host <name> <proto> <port>` rule of a manifest. Owns its host `String` (a manifest is parsed
/// at runtime from a file, unlike the compiled `&'static str` [`crate::egress::EgressRule`]); otherwise
/// the same `(host, proto, port)` triple. Reuses the raw-host grammar + proto/port parse verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostRule {
    pub host: String,
    pub proto: Proto,
    pub port: u16,
}

/// A fully-parsed, fully-valid capability manifest (ADR-009 §4.3). Construction goes ONLY through
/// [`parse_manifest`], so every invariant (`[R-MF1]`/`[R-MF2]`) holds for any value that exists — an
/// invalid manifest is a [`ManifestError`], never a half-built struct. Fields mirror the flat schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    /// The capability name — the closed token the grant store / socket request keys on (`weather`).
    pub name: String,
    /// Card title (root-authored, trustworthy for the panel to render — §4.3).
    pub title: String,
    /// Card purpose line.
    pub purpose: String,
    /// The feature badge token (`dms:weather`) — a colon-namespaced closed token (see [`valid_feature`]).
    pub feature: String,
    /// One-click vs ceremony (`[R-MF2]` already enforced against `rules` at parse time).
    pub tier: Tier,
    /// Whether pins are lifted into `/etc/hosts` (`[R-MF4]`; sealed-source only downstream).
    pub deliver: Deliver,
    /// The `host` rules, in file order. Non-empty (a capability that reaches nothing is malformed here).
    pub rules: Vec<HostRule>,
}

impl Manifest {
    /// Deny-by-default exact membership — the manifest analog of [`crate::egress::EgressProfile::allows`].
    pub fn allows(&self, host: &str, proto: Proto, port: u16) -> bool {
        self.rules
            .iter()
            .any(|r| r.host == host && r.proto == proto && r.port == port)
    }
}

// ---- token grammar ------------------------------------------------------------------------------

/// The closed capability-token class (§4.3): lowercase, `[a-z0-9-]`, non-empty, NO leading dash (an
/// argv-option-injection defense identical to the raw-host rule). Same class as a sealed profile name
/// (`weather`, `desktop-ntp`) so `name` is drawn from the same vocabulary the compiled table uses.
pub fn valid_capability_token(t: &str) -> bool {
    !t.is_empty()
        && !t.starts_with('-')
        && t.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The `feature` token grammar. The ADR example is `dms:weather` — a colon-namespaced token — so a
/// feature is one OR two [`valid_capability_token`] segments joined by a single `:` (e.g. `dms:weather`
/// or a bare `weather`). Each segment obeys the closed class; the colon is the ONLY extra byte, and at
/// most one, so no free text and no leading-dash injection ever reaches the rendered card.
pub fn valid_feature(t: &str) -> bool {
    let mut segs = t.split(':');
    let (Some(a), b, rest) = (segs.next(), segs.next(), segs.next()) else {
        return false;
    };
    if rest.is_some() {
        return false; // at most one colon
    }
    match b {
        Some(b) => valid_capability_token(a) && valid_capability_token(b),
        None => valid_capability_token(a),
    }
}

// ---- the parser ---------------------------------------------------------------------------------

/// Parse ONE flat capability manifest, fail-closed and whole-file (`[R-MF1]`) with the tier invariant
/// enforced at parse time (`[R-MF2]`). LF-terminated flat text, one file per capability, closed keys:
///
/// ```text
/// schema shrek-egress-capability/1
/// name <token>
/// title <text>
/// purpose <text>
/// feature <token>
/// tier one-click|ceremony
/// deliver hosts|none
/// host <name> <proto> <port>        (repeated, >= 1)
/// ```
///
/// Rules (all fail-closed — any violation ⇒ the WHOLE manifest is `Err`, never partial):
///   * The FIRST non-empty line must be exactly `schema <SCHEMA_ID>`. A different value / missing /
///     misplaced schema ⇒ [`ManifestError::BadSchema`]. A future schema id is REFUSED, not adapted.
///   * Keys are the closed set above; any other first-token ⇒ [`ManifestError::UnknownKey`].
///   * `name`/`title`/`purpose`/`feature`/`tier`/`deliver` are single-valued (a repeat ⇒
///     [`ManifestError::DuplicateKey`]); all are required (absence ⇒ [`ManifestError::MissingKey`]).
///   * `name` obeys [`valid_capability_token`]; `feature` obeys [`valid_feature`]. `title`/`purpose`
///     are free-ish text but must be non-empty and single-line (LF is the terminator; no control bytes).
///   * `host <name> <proto> <port>` reuses the sealed raw grammar VERBATIM (host via `valid_raw_host`,
///     proto `tcp|udp`, port `1..=65535`) by delegating to the same field checks the raw-triple parser
///     uses — NO reinvention (§4.3).
///   * `[R-MF2]`: if `tier one-click`, EVERY host rule must be `tcp` port `443`; else
///     [`ManifestError::OneClickNonTls`]. `tier ceremony` imposes no port restriction.
///   * At least one `host` line. `deliver hosts` with zero host lines (or the schema present but no
///     rules) ⇒ [`ManifestError::NoHostRules`].
///
/// Blank lines are permitted BETWEEN records only after the schema line, and are ignored; a blank/
/// malformed line elsewhere yields [`ManifestError::MalformedLine`]. Leading/trailing ASCII spaces
/// around a value are trimmed; interior structure is the record's own.
pub fn parse_manifest(text: &str) -> Result<Manifest, ManifestError> {
    // Reject control bytes up front (except the LF line terminator) — the same world-readable-`/run`
    // line-injection defense the raw grammar applies, hoisted to cover title/purpose free text too.
    if text
        .bytes()
        .any(|b| b != b'\n' && b != b'\t' && b.is_ascii_control())
    {
        return Err(ManifestError::MalformedLine("control byte in manifest".to_string()));
    }

    let mut lines = text.lines();

    // [R-MF1] schema MUST be the first non-empty line, exact value.
    let first = lines
        .find(|l| !l.trim().is_empty())
        .ok_or(ManifestError::BadSchema)?;
    match first.strip_prefix("schema ").map(str::trim) {
        Some(v) if v == SCHEMA_ID => {}
        _ => return Err(ManifestError::BadSchema),
    }

    let mut name: Option<String> = None;
    let mut title: Option<String> = None;
    let mut purpose: Option<String> = None;
    let mut feature: Option<String> = None;
    let mut tier: Option<Tier> = None;
    let mut deliver: Option<Deliver> = None;
    let mut rules: Vec<HostRule> = Vec::new();

    // A single-valued key: set once or fault DuplicateKey (fail-closed, no silent last-wins).
    fn set_once<T>(slot: &mut Option<T>, val: T, key: &'static str) -> Result<(), ManifestError> {
        if slot.is_some() {
            return Err(ManifestError::DuplicateKey(key));
        }
        *slot = Some(val);
        Ok(())
    }

    for raw in lines {
        let line = raw.trim_end_matches([' ', '\t']);
        if line.trim().is_empty() {
            continue; // blank separator lines are ignored (post-schema only, per doc)
        }
        // A second `schema` line is an unknown/duplicate key path — treat as malformed schema repeat.
        if line == first || line.strip_prefix("schema ").is_some() {
            return Err(ManifestError::DuplicateKey("schema"));
        }
        let (key, value) = match line.split_once(' ') {
            Some((k, v)) => (k, v.trim()),
            None => return Err(ManifestError::MalformedLine(format!("no value for `{line}`"))),
        };
        if value.is_empty() {
            return Err(ManifestError::MalformedLine(format!("empty value for `{key}`")));
        }
        match key {
            "name" => {
                if !valid_capability_token(value) {
                    return Err(ManifestError::BadToken(value.to_string()));
                }
                set_once(&mut name, value.to_string(), "name")?;
            }
            "title" => set_once(&mut title, value.to_string(), "title")?,
            "purpose" => set_once(&mut purpose, value.to_string(), "purpose")?,
            "feature" => {
                if !valid_feature(value) {
                    return Err(ManifestError::BadToken(value.to_string()));
                }
                set_once(&mut feature, value.to_string(), "feature")?;
            }
            "tier" => {
                let t = match value {
                    "one-click" => Tier::OneClick,
                    "ceremony" => Tier::Ceremony,
                    other => return Err(ManifestError::BadTier(other.to_string())),
                };
                set_once(&mut tier, t, "tier")?;
            }
            "deliver" => {
                let d = match value {
                    "hosts" => Deliver::Hosts,
                    "none" => Deliver::None,
                    other => return Err(ManifestError::BadDeliver(other.to_string())),
                };
                set_once(&mut deliver, d, "deliver")?;
            }
            "host" => rules.push(parse_host_rule(value)?),
            other => return Err(ManifestError::UnknownKey(other.to_string())),
        }
    }

    // [R-MF1] every required key must be present (fail-closed on absence).
    let name = name.ok_or(ManifestError::MissingKey("name"))?;
    let title = title.ok_or(ManifestError::MissingKey("title"))?;
    let purpose = purpose.ok_or(ManifestError::MissingKey("purpose"))?;
    let feature = feature.ok_or(ManifestError::MissingKey("feature"))?;
    let tier = tier.ok_or(ManifestError::MissingKey("tier"))?;
    let deliver = deliver.ok_or(ManifestError::MissingKey("deliver"))?;

    // A capability that reaches nothing is malformed (there is nothing to bless).
    if rules.is_empty() {
        return Err(ManifestError::NoHostRules);
    }

    // [R-MF2] THE TIER INVARIANT — checked here so an invalid one-click manifest never EXISTS. Every
    // one-click host rule must be tcp:443; this is what makes `ip-api.com tcp 80` structurally
    // unclickable and forbids any udp host from a one-click capability.
    if tier == Tier::OneClick {
        for r in &rules {
            if r.proto != Proto::Tcp || r.port != 443 {
                return Err(ManifestError::OneClickNonTls(format!(
                    "{} {} {}",
                    r.host,
                    r.proto.label(),
                    r.port
                )));
            }
        }
    }

    Ok(Manifest {
        name,
        title,
        purpose,
        feature,
        tier,
        deliver,
        rules,
    })
}

/// Parse one `host` line VALUE (`<name> <proto> <port>`) by reusing the sealed raw grammar verbatim
/// (§4.3): join the three space-separated fields as `name:proto:port` and delegate to
/// [`crate::desktop_egress::parse_raw_triple`], so the host (`valid_raw_host`), proto (`tcp|udp`), and
/// port (`1..=65535`) checks are the EXACT SAME argv/line-injection-hardened rules — no reinvention.
fn parse_host_rule(value: &str) -> Result<HostRule, ManifestError> {
    let fields: Vec<&str> = value.split(' ').filter(|f| !f.is_empty()).collect();
    if fields.len() != 3 {
        return Err(ManifestError::BadHostRule(format!(
            "expected `<name> <proto> <port>`, got `{value}`"
        )));
    }
    let (host, proto, port) = (fields[0], fields[1], fields[2]);
    // Guard host separately with the shared grammar first so a colon smuggled into the host can't
    // confuse the `name:proto:port` reconstruction we hand to the raw parser.
    if !valid_raw_host(host) {
        return Err(ManifestError::BadHostRule(format!("invalid host `{host}`")));
    }
    let wire = format!("{host}:{proto}:{port}");
    let t = crate::desktop_egress::parse_raw_triple(&wire)
        .map_err(|_| ManifestError::BadHostRule(format!("{host} {proto} {port}")))?;
    Ok(HostRule {
        host: t.host,
        proto: t.proto,
        port: t.port,
    })
}

// ---- the catalog --------------------------------------------------------------------------------

/// The authoring source of a catalog entry (ADR-009 §4.2). The `/etc/hosts` delivery restriction and
/// the collision rule are keyed on this — it is a DATA property of every entry, never inferred at the
/// call site: `Sealed` is dm-verity `/usr/lib/shrek/egress-capabilities`; `Owner` is the root:root 0700
/// `/home/.shrek-system/egress/manifests`, written only by egressd on a confirmed SAK ceremony.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Shipped-in-image, dm-verity sealed — identical integrity to a compiled Rust literal (§4.2).
    Sealed,
    /// Owner-installed via ceremony — same authority a raw triple has, but NEVER lifted to `/etc/hosts`.
    Owner,
}

/// A capability entry: a parsed [`Manifest`] tagged with its [`Source`] and its fault state. An OWNER
/// entry that collided with a sealed name is kept as `faulted` so the panel can render it disabled +
/// legible (ADR-009 §4.3) — the collision is not silently dropped, it is a visible refusal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityEntry {
    pub manifest: Manifest,
    pub source: Source,
    /// `Some(reason)` if this entry is present-but-disabled (an owner manifest shadowing a sealed name,
    /// `[R-MF3]`). `None` for an active entry.
    pub fault: Option<String>,
}

/// The merged catalog over sealed + owner manifests — the S1 pure view egressd (S2) loads at boot and
/// after a ceremony install. Enforces `[R-MF3]` sealed-always-wins at construction ([`build_catalog`]),
/// so no consumer can accidentally honor an owner entry that shadows a sealed name.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Catalog {
    /// Active (non-faulted) entries, sealed first then owner, de-collided per `[R-MF3]`.
    pub entries: Vec<CapabilityEntry>,
    /// Owner entries REFUSED because their name collided with a sealed capability. Kept for legible
    /// display (§4.3), never active. Not a silent drop.
    pub faulted: Vec<CapabilityEntry>,
}

impl Catalog {
    /// Look up an ACTIVE capability by name (faulted entries are never resolvable — fail-closed).
    pub fn get(&self, name: &str) -> Option<&CapabilityEntry> {
        self.entries.iter().find(|e| e.manifest.name == name)
    }

    /// All ACTIVE sealed-source entries (the only ones eligible for `/etc/hosts` delivery, `[R-MF4]`).
    pub fn sealed_entries(&self) -> impl Iterator<Item = &CapabilityEntry> {
        self.entries.iter().filter(|e| e.source == Source::Sealed)
    }
}

/// Build the merged catalog from the two authoring sources (§4.2/§4.3), enforcing `[R-MF3]`: on a name
/// collision the SEALED entry wins and the OWNER entry FAULTS (an owner manifest can NEVER shadow a
/// sealed one). Sealed manifests are all admitted (their integrity is dm-verity's); an owner manifest
/// whose name matches ANY sealed name is moved to `faulted` with a legible reason. This is the pure
/// logic — the actual two-dir filesystem read is S2/egressd; here both inputs are already parsed.
///
/// Note: duplicate names WITHIN one source are not the collision this rule governs (that is a loader/fs
/// concern — one file per capability, §4.3); this fn merges across sources and applies precedence. If a
/// name appears twice among sealed inputs, the first is kept (sealed integrity means this shouldn't
/// happen; keeping first is the fail-closed choice — no owner-influenced ordering).
pub fn build_catalog(sealed: Vec<Manifest>, owner: Vec<Manifest>) -> Catalog {
    let mut entries: Vec<CapabilityEntry> = Vec::new();
    let mut faulted: Vec<CapabilityEntry> = Vec::new();

    // Sealed first — they set the reserved-name floor everything else defers to.
    let mut sealed_names: Vec<String> = Vec::new();
    for m in sealed {
        if sealed_names.iter().any(|n| n == &m.name) {
            // Two sealed manifests with one name: keep the first, fault the rest (defensive; §4.3
            // is one-file-per-capability, so this is a loader-caught anomaly surfaced legibly).
            faulted.push(CapabilityEntry {
                fault: Some(format!("duplicate sealed name `{}`", m.name)),
                source: Source::Sealed,
                manifest: m,
            });
            continue;
        }
        sealed_names.push(m.name.clone());
        entries.push(CapabilityEntry {
            manifest: m,
            source: Source::Sealed,
            fault: None,
        });
    }

    // Owner entries: a collision with ANY sealed name faults (sealed wins, [R-MF3]); a collision with
    // an already-admitted owner name also faults (no owner-vs-owner silent shadow).
    let mut owner_names: Vec<String> = Vec::new();
    for m in owner {
        if sealed_names.iter().any(|n| n == &m.name) {
            faulted.push(CapabilityEntry {
                fault: Some(format!(
                    "owner capability `{}` shadows a sealed capability (sealed wins)",
                    m.name
                )),
                source: Source::Owner,
                manifest: m,
            });
            continue;
        }
        if owner_names.iter().any(|n| n == &m.name) {
            faulted.push(CapabilityEntry {
                fault: Some(format!("duplicate owner name `{}`", m.name)),
                source: Source::Owner,
                manifest: m,
            });
            continue;
        }
        owner_names.push(m.name.clone());
        entries.push(CapabilityEntry {
            manifest: m,
            source: Source::Owner,
            fault: None,
        });
    }

    Catalog { entries, faulted }
}

// ---- the §4.4 isolation predicates --------------------------------------------------------------

/// `[R-MF4]` — the STRUCTURAL owner-pin↔root-resolution isolation predicate (ADR-009 §4.4 layer 1).
/// True iff `host` belongs to a SEALED-source, NON-BASELINE, `deliver hosts` ACTIVE capability in
/// `catalog`. This is the ONLY set the S2 `/etc/hosts` composer (`hosts.rs`) may lift into host-wide
/// resolution — so an owner-manifest pin can NEVER enter the file root reads, by construction and
/// permanently, regardless of any future hostname collision.
///
/// Fail-closed on every other case: an owner-source host, a `deliver none` host, an off-catalog host,
/// or a faulted entry ⇒ `false`. Baseline exclusion is inherited from the compiled table's own tier
/// (a manifest is never a baseline; the baseline profiles stay in [`crate::desktop_egress`]) — and is
/// re-asserted here so a host that ALSO happens to be a compiled baseline host is never lifted via a
/// manifest path. Replaces the draft's `is_blessable_desktop_host` for the §4.4 restriction; egressd's
/// `hosts.rs` calls THIS in S2.
pub fn is_sealed_deliverable_host(catalog: &Catalog, host: &str) -> bool {
    // Never lift a host that is a compiled BASELINE destination via the manifest path (defense in
    // depth against a manifest that names one — belt for the §4.4 "non-baseline" clause).
    if is_compiled_baseline_host(host) {
        return false;
    }
    catalog
        .sealed_entries()
        .filter(|e| e.manifest.deliver == Deliver::Hosts)
        .any(|e| e.manifest.rules.iter().any(|r| r.host == host))
}

/// `[R-MF5]` — the install-refuse host set (ADR-009 §4.4 layer 2). True if `host` is already consumed
/// by SEALED or ROOT machinery, so an owner manifest naming it must be REFUSED at install (not warned).
/// Enumerated, per the ADR, from four sources of already-reserved names:
///
///   1. The AGENT egress tables ([`crate::egress::EGRESS_PROFILES`]) — every host any agent profile
///      reaches (github, debian, pypi, the model/swamp broker aliases, …).
///   2. The provider-bind ALIAS set ([`crate::provider_bind::is_sealed_alias_host`]) — the 4 model
///      broker names + the swamp broker (names root resolves through the root-owned `/etc/hosts`).
///   3. The SEALED DESKTOP capability hosts — weather's open-meteo hosts (and any non-baseline compiled
///      desktop profile host); a sealed manifest's hosts are equally reserved once shipped.
///   4. The BASELINE profile hosts — `desktop-updates`'s update front (and `desktop-ntp`'s literal IPs).
///
/// Pure/total, no fs. The ceremony-refusal WIRING is S3; S1 supplies the predicate. Note this is a
/// SUPERSET guard: it is deliberately broad (an owner naming ANY reserved host is refused), because the
/// §4.4 ruling is "not a warning — a refusal".
pub fn is_system_reserved_host(host: &str) -> bool {
    // 1. Agent egress tables — every destination of every agent profile.
    if crate::egress::EGRESS_PROFILES
        .iter()
        .flat_map(|p| p.rules.iter())
        .any(|r| r.host == host)
    {
        return true;
    }
    // 2. Provider-bind alias set (model brokers + swamp) — the names root resolves via /etc/hosts.
    if crate::provider_bind::is_sealed_alias_host(host) {
        return true;
    }
    // 3 + 4. Every compiled DESKTOP profile host — baseline AND sealed non-baseline (weather). A sealed
    // desktop capability host is reserved; a baseline host is reserved. (desktop-ntp's hosts are literal
    // IPs, still matched verbatim; an owner host that is a bare IP colliding with them is refused too.)
    DESKTOP_EGRESS_PROFILES
        .iter()
        .flat_map(|p| p.rules.iter())
        .any(|r| r.host == host)
}

/// Is `host` a destination of a COMPILED BASELINE desktop profile (`desktop-ntp`/`desktop-updates`)?
/// Used by [`is_sealed_deliverable_host`] to re-assert the §4.4 "non-baseline" clause on the manifest
/// path. (Baseline hosts are resolved by root services not caught by the uid-1000 DNS drop, or are
/// literal IPs — neither needs nor should acquire a host-wide `/etc/hosts` pin via a capability.)
fn is_compiled_baseline_host(host: &str) -> bool {
    DESKTOP_EGRESS_PROFILES
        .iter()
        .filter(|p: &&EgressProfile| is_baseline_profile(p.name))
        .flat_map(|p| p.rules.iter())
        .any(|r| r.host == host)
}

// ---- the first sealed manifest fixture (ADR-009 §4.3) -------------------------------------------

/// The `weather` capability as its FIRST manifest (ADR-009 §4.3) — the sealed-source fixture proving
/// the grammar accepts the canonical shape. In S1 this is an in-code const; the on-disk
/// `/usr/lib/shrek/egress-capabilities/weather.capability` is S5 content. Kept byte-identical to the
/// ADR §4.3 listing so the spec and the fixture cannot drift. `weather` also remains in the compiled
/// [`crate::desktop_egress`] table for S1 (removing it would destabilize the S2 supervisor's current
/// resolve path); this manifest is the data-driven mirror the S2 loader will consume.
pub const WEATHER_MANIFEST: &str = "\
schema shrek-egress-capability/1
name weather
title Weather
purpose Local forecast and location search
feature dms:weather
tier one-click
deliver hosts
host api.open-meteo.com tcp 443
host geocoding-api.open-meteo.com tcp 443
";

#[cfg(test)]
mod tests {
    use super::*;

    // A small builder for a valid one-click manifest with a caller-chosen host block, so the tests
    // vary exactly one axis at a time (the tier invariant, a bad key, etc.).
    fn manifest_with(tier: &str, deliver: &str, hosts: &str) -> String {
        format!(
            "schema shrek-egress-capability/1\n\
             name testcap\n\
             title Test Capability\n\
             purpose A test capability\n\
             feature dms:test\n\
             tier {tier}\n\
             deliver {deliver}\n\
             {hosts}"
        )
    }

    // ---- [R-MF1] fail-closed, whole-file --------------------------------------------------------

    #[test]
    fn weather_manifest_parses_and_is_one_click_tls_pair() {
        // The FIRST capability manifest parses, is one-click, delivers hosts, and reaches EXACTLY the
        // two open-meteo hosts on tcp/443 (ADR-009 §4.3). This is the canonical happy path.
        let m = parse_manifest(WEATHER_MANIFEST).expect("weather manifest must parse");
        assert_eq!(m.name, "weather");
        assert_eq!(m.title, "Weather");
        assert_eq!(m.purpose, "Local forecast and location search");
        assert_eq!(m.feature, "dms:weather");
        assert_eq!(m.tier, Tier::OneClick);
        assert_eq!(m.deliver, Deliver::Hosts);
        assert_eq!(m.rules.len(), 2);
        assert!(m.allows("api.open-meteo.com", Proto::Tcp, 443));
        assert!(m.allows("geocoding-api.open-meteo.com", Proto::Tcp, 443));
        // Deny-by-default around it: no plaintext :80, no other host.
        assert!(!m.allows("api.open-meteo.com", Proto::Tcp, 80));
        assert!(!m.allows("ip-api.com", Proto::Tcp, 80));
        // And every rule is tcp/443 (the [R-MF2] invariant held at parse time).
        for r in &m.rules {
            assert_eq!(r.proto, Proto::Tcp);
            assert_eq!(r.port, 443);
        }
    }

    #[test]
    fn malformed_manifest_is_rejected_whole() {
        // A line with a key but no value ⇒ MalformedLine, whole-file reject (never a partial parse).
        let bad = manifest_with("one-click", "hosts", "host\n");
        assert!(matches!(parse_manifest(&bad), Err(ManifestError::MalformedLine(_))));
        // A bare garbage line (no key/value shape).
        let bad2 = "schema shrek-egress-capability/1\nname x\ngarbagelinewithnovalue\n";
        assert!(matches!(parse_manifest(bad2), Err(ManifestError::MalformedLine(_))));
        // A control byte anywhere ⇒ reject (state-file line-injection defense).
        let bad3 = manifest_with("one-click", "hosts", "host api.open-meteo.com\x00 tcp 443\n");
        assert!(parse_manifest(&bad3).is_err());
    }

    #[test]
    fn unknown_key_is_rejected() {
        let bad = "schema shrek-egress-capability/1\n\
                   name weather\ntitle W\npurpose P\nfeature f\ntier one-click\n\
                   deliver hosts\nhost api.open-meteo.com tcp 443\n\
                   evilkey somevalue\n";
        assert_eq!(
            parse_manifest(bad),
            Err(ManifestError::UnknownKey("evilkey".to_string()))
        );
    }

    #[test]
    fn bad_schema_is_rejected() {
        // wrong schema value ⇒ refused (a future schema is NOT best-effort adapted).
        let bad = WEATHER_MANIFEST.replace("shrek-egress-capability/1", "shrek-egress-capability/2");
        assert_eq!(parse_manifest(&bad), Err(ManifestError::BadSchema));
        // missing schema entirely.
        assert_eq!(parse_manifest("name weather\n"), Err(ManifestError::BadSchema));
        // schema not first.
        let bad2 = "name weather\nschema shrek-egress-capability/1\n";
        assert_eq!(parse_manifest(bad2), Err(ManifestError::BadSchema));
        // empty input.
        assert_eq!(parse_manifest(""), Err(ManifestError::BadSchema));
    }

    #[test]
    fn missing_required_key_is_rejected() {
        // drop `tier` ⇒ MissingKey("tier").
        let bad = "schema shrek-egress-capability/1\n\
                   name weather\ntitle W\npurpose P\nfeature f\n\
                   deliver hosts\nhost api.open-meteo.com tcp 443\n";
        assert_eq!(parse_manifest(bad), Err(ManifestError::MissingKey("tier")));
    }

    #[test]
    fn duplicate_single_valued_key_is_rejected() {
        let bad = manifest_with("one-click", "hosts", "host api.open-meteo.com tcp 443\ntier ceremony\n");
        assert_eq!(parse_manifest(&bad), Err(ManifestError::DuplicateKey("tier")));
    }

    #[test]
    fn no_host_rules_is_rejected() {
        let bad = manifest_with("one-click", "hosts", "");
        assert_eq!(parse_manifest(&bad), Err(ManifestError::NoHostRules));
    }

    #[test]
    fn bad_tokens_and_values_rejected() {
        // name with an illegal byte / leading dash.
        let bad_name = manifest_with("one-click", "hosts", "host a.b tcp 443\n")
            .replace("name testcap", "name -evil");
        assert!(matches!(parse_manifest(&bad_name), Err(ManifestError::BadToken(_))));
        // feature with two colons (not the closed one-colon namespace shape).
        let bad_feat = manifest_with("one-click", "hosts", "host a.b tcp 443\n")
            .replace("feature dms:test", "feature a:b:c");
        assert!(matches!(parse_manifest(&bad_feat), Err(ManifestError::BadToken(_))));
        // tier not in the closed set.
        let bad_tier = manifest_with("weekly", "hosts", "host a.b tcp 443\n");
        assert!(matches!(parse_manifest(&bad_tier), Err(ManifestError::BadTier(_))));
        // deliver not in the closed set.
        let bad_deliver = manifest_with("one-click", "maybe", "host a.b tcp 443\n");
        assert!(matches!(parse_manifest(&bad_deliver), Err(ManifestError::BadDeliver(_))));
    }

    #[test]
    fn host_rule_reuses_the_raw_grammar() {
        // A single-label host (no dot) is rejected by the reused valid_raw_host, same as raw triples.
        let bad = manifest_with("ceremony", "none", "host singlelabel tcp 8080\n");
        assert!(matches!(parse_manifest(&bad), Err(ManifestError::BadHostRule(_))));
        // Wrong arity (missing port).
        let bad2 = manifest_with("ceremony", "none", "host a.b tcp\n");
        assert!(matches!(parse_manifest(&bad2), Err(ManifestError::BadHostRule(_))));
        // Bad proto.
        let bad3 = manifest_with("ceremony", "none", "host a.b sctp 443\n");
        assert!(matches!(parse_manifest(&bad3), Err(ManifestError::BadHostRule(_))));
        // A leading-dash host (argv-injection) is refused by the shared grammar.
        let bad4 = manifest_with("ceremony", "none", "host -evil.com tcp 443\n");
        assert!(matches!(parse_manifest(&bad4), Err(ManifestError::BadHostRule(_))));
    }

    // ---- [R-MF2] THE TIER INVARIANT -------------------------------------------------------------

    #[test]
    fn one_click_all_tcp_443_is_ok() {
        let ok = manifest_with(
            "one-click",
            "hosts",
            "host api.open-meteo.com tcp 443\nhost geocoding-api.open-meteo.com tcp 443\n",
        );
        let m = parse_manifest(&ok).expect("all-tcp-443 one-click must parse");
        assert_eq!(m.tier, Tier::OneClick);
        assert_eq!(m.rules.len(), 2);
    }

    #[test]
    fn one_click_with_port_80_host_is_rejected() {
        // THE invariant that makes ip-api.com:80 structurally unclickable (ADR-009 §3/§4.3).
        let bad = manifest_with("one-click", "none", "host ip-api.com tcp 80\n");
        let err = parse_manifest(&bad).unwrap_err();
        assert!(matches!(err, ManifestError::OneClickNonTls(_)));
        assert!(err.reason().contains("ip-api.com"));
    }

    #[test]
    fn one_click_with_udp_host_is_rejected() {
        // A one-click capability may not carry a udp host either (only tcp:443 is one-clickable).
        let bad = manifest_with("one-click", "none", "host a.b udp 443\n");
        assert!(matches!(parse_manifest(&bad), Err(ManifestError::OneClickNonTls(_))));
        // A non-443 tcp port is also refused into one-click.
        let bad2 = manifest_with("one-click", "none", "host a.b tcp 8443\n");
        assert!(matches!(parse_manifest(&bad2), Err(ManifestError::OneClickNonTls(_))));
    }

    #[test]
    fn ceremony_tier_allows_other_ports_and_protos() {
        // ceremony carries the higher-consequence authority, so it has NO port restriction: :80, an
        // odd port, and udp are all admissible under ceremony (they just are not one-clickable).
        let m1 = parse_manifest(&manifest_with("ceremony", "none", "host ip-api.com tcp 80\n"))
            .expect("ceremony :80 must parse");
        assert_eq!(m1.tier, Tier::Ceremony);
        assert!(m1.allows("ip-api.com", Proto::Tcp, 80));
        let m2 = parse_manifest(&manifest_with("ceremony", "none", "host a.b udp 5353\n"))
            .expect("ceremony udp must parse");
        assert!(m2.allows("a.b", Proto::Udp, 5353));
    }

    // ---- [R-MF3] catalog: sealed shadows owner --------------------------------------------------

    fn weather() -> Manifest {
        parse_manifest(WEATHER_MANIFEST).unwrap()
    }

    fn owner_manifest(name: &str, host: &str) -> Manifest {
        let text = format!(
            "schema shrek-egress-capability/1\n\
             name {name}\ntitle T\npurpose P\nfeature owner:{name}\n\
             tier ceremony\ndeliver none\nhost {host} tcp 443\n"
        );
        parse_manifest(&text).unwrap()
    }

    #[test]
    fn catalog_sealed_shadows_owner_collision_faults() {
        // An owner manifest reusing the sealed `weather` name faults (sealed wins, [R-MF3]); it is
        // present-but-disabled in `faulted`, never active. A distinct owner name is admitted active.
        let sealed = vec![weather()];
        let owner = vec![
            owner_manifest("weather", "evil.example.com"), // collides → faults
            owner_manifest("owner-thing", "owner.example.com"), // distinct → active
        ];
        let cat = build_catalog(sealed, owner);
        // weather resolves to the SEALED entry (not the owner shadow).
        let w = cat.get("weather").expect("weather is active");
        assert_eq!(w.source, Source::Sealed);
        assert!(w.fault.is_none());
        assert!(!w.manifest.allows("evil.example.com", Proto::Tcp, 443));
        // the owner shadow is faulted, not active.
        assert_eq!(cat.faulted.len(), 1);
        assert_eq!(cat.faulted[0].manifest.name, "weather");
        assert_eq!(cat.faulted[0].source, Source::Owner);
        assert!(cat.faulted[0].fault.is_some());
        // the distinct owner capability IS active.
        let o = cat.get("owner-thing").expect("distinct owner cap active");
        assert_eq!(o.source, Source::Owner);
    }

    #[test]
    fn catalog_duplicate_owner_names_fault() {
        let cat = build_catalog(
            vec![],
            vec![
                owner_manifest("dup", "a.example.com"),
                owner_manifest("dup", "b.example.com"),
            ],
        );
        assert_eq!(cat.entries.len(), 1); // first kept
        assert_eq!(cat.faulted.len(), 1); // second faulted
    }

    // ---- [R-MF4] is_sealed_deliverable_host -----------------------------------------------------

    #[test]
    fn is_sealed_deliverable_host_true_for_sealed_weather_false_otherwise() {
        let sealed = vec![weather()];
        // an owner `deliver none` cap + an owner `deliver hosts`-attempting cap (still owner-source).
        let owner = vec![owner_manifest("owner-cap", "owner-host.example.com")];
        let cat = build_catalog(sealed, owner);

        // TRUE only for the sealed weather hosts (sealed-source, non-baseline, deliver hosts).
        assert!(is_sealed_deliverable_host(&cat, "api.open-meteo.com"));
        assert!(is_sealed_deliverable_host(&cat, "geocoding-api.open-meteo.com"));
        // FALSE for an owner-source host (owner pins NEVER lifted into host-wide resolution).
        assert!(!is_sealed_deliverable_host(&cat, "owner-host.example.com"));
        // FALSE for a compiled baseline host (desktop-updates) — the §4.4 non-baseline clause.
        assert!(!is_sealed_deliverable_host(&cat, "shrekos-updates.iambu.dev"));
        // FALSE for an off-catalog host.
        assert!(!is_sealed_deliverable_host(&cat, "random.example.com"));
    }

    #[test]
    fn sealed_deliver_none_host_is_not_deliverable() {
        // A sealed-source capability with `deliver none` is NOT lifted into /etc/hosts (only
        // `deliver hosts` is). Build a sealed manifest with deliver none and confirm.
        let sealed_none = parse_manifest(
            "schema shrek-egress-capability/1\n\
             name sealednone\ntitle T\npurpose P\nfeature s:none\n\
             tier one-click\ndeliver none\nhost pinned.example.com tcp 443\n",
        )
        .unwrap();
        let cat = build_catalog(vec![sealed_none], vec![]);
        assert!(!is_sealed_deliverable_host(&cat, "pinned.example.com"));
    }

    // ---- [R-MF5] is_system_reserved_host --------------------------------------------------------

    #[test]
    fn is_system_reserved_host_covers_all_reserved_sources() {
        // 1. an agent-egress host.
        assert!(is_system_reserved_host("github.com"));
        assert!(is_system_reserved_host("deb.debian.org"));
        // 2. a provider-bind alias (model broker) + the swamp broker.
        assert!(is_system_reserved_host("shrek-model-proxy"));
        assert!(is_system_reserved_host("shrek-swamp-broker"));
        // 3. a sealed desktop capability host (weather / open-meteo).
        assert!(is_system_reserved_host("api.open-meteo.com"));
        assert!(is_system_reserved_host("geocoding-api.open-meteo.com"));
        // 4. a baseline profile host (desktop-updates front; desktop-ntp literal IP).
        assert!(is_system_reserved_host("shrekos-updates.iambu.dev"));
        assert!(is_system_reserved_host("162.159.200.1"));
        // FALSE for a host no sealed/root machinery consumes (an owner manifest may name THIS one).
        assert!(!is_system_reserved_host("weather.example.org"));
        assert!(!is_system_reserved_host("owner-chosen.example.com"));
        assert!(!is_system_reserved_host(""));
    }

    // ---- token grammar --------------------------------------------------------------------------

    #[test]
    fn token_and_feature_grammar() {
        assert!(valid_capability_token("weather"));
        assert!(valid_capability_token("desktop-ntp"));
        assert!(valid_capability_token("cap0"));
        assert!(!valid_capability_token(""));
        assert!(!valid_capability_token("-lead"));
        assert!(!valid_capability_token("Upper"));
        assert!(!valid_capability_token("has space"));
        assert!(!valid_capability_token("under_score"));
        // feature: one or two closed segments joined by a single colon.
        assert!(valid_feature("dms:weather"));
        assert!(valid_feature("weather"));
        assert!(!valid_feature("a:b:c")); // two colons
        assert!(!valid_feature("dms:")); // empty segment
        assert!(!valid_feature(":weather")); // empty segment
        assert!(!valid_feature("dms:-x")); // leading dash in a segment
    }

    #[test]
    fn tier_and_source_tokens_are_closed() {
        assert_eq!(Tier::OneClick.as_str(), "one-click");
        assert_eq!(Tier::Ceremony.as_str(), "ceremony");
    }
}
