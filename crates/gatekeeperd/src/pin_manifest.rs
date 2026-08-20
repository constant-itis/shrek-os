//! pin_manifest — the sealed static pin store (Phase-5 slice-8, B1 evidence store for `T-pinned`).
//!
//! A digest-keyed allow-list, sealed under the dm-verity `/usr` root (security-model.md §4 STATIC
//! custody: change it ⇒ signed image update). It lets a *specific, content-vetted* third-party
//! artifact earn `T-pinned` (floor T0) instead of failing high to `T-hostile`. gatekeeperd measures
//! the object's fs-verity digest (see [`crate::provenance_plane`]) and looks the tuple
//! `(digest_algorithm, digest)` up here; a hit with a `closed-world` class sets both facts the
//! `shrek_policy::derive_band` lattice needs for `T-pinned`.
//!
//! Design of record: [`docs/phase5-slice8-pin-manifest.md`](../../../docs/phase5-slice8-pin-manifest.md)
//! (amendment B: a versioned sealed FILE, tiny dependency-free parser, **fail-high on malformed /
//! unknown / conflicting input**). This module is the parser + the pure lookup; the file read and the
//! kernel measurement live in `provenance_plane`.
//!
//! Fail-high is total: [`PinManifest::parse`] returns `Err` for ANY doubt, and the caller treats an
//! `Err` (or an absent file) as "no pins" — so nothing can earn `T-pinned`, the correct fail-safe.
//! There is deliberately no partial/best-effort parse: one bad line poisons the whole manifest rather
//! than silently shipping an optimistic subset.

/// fs-verity digest algorithms we recognise. The `u16` is the on-ABI `fsverity_digest.digest_algorithm`
/// (`FS_VERITY_HASH_ALG_*`, linux/fsverity.h); the size is the digest length the kernel emits and the
/// manifest must encode. An algorithm outside this set — from the kernel OR the manifest — never
/// matches (amendment A: unknown algorithm/size fails high).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DigestAlgo {
    Sha256,
    Sha512,
}

impl DigestAlgo {
    /// fs-verity `FS_VERITY_HASH_ALG_SHA256 = 1`, `SHA512 = 2` (linux/fsverity.h).
    pub fn from_verity_id(id: u16) -> Option<DigestAlgo> {
        match id {
            1 => Some(DigestAlgo::Sha256),
            2 => Some(DigestAlgo::Sha512),
            _ => None,
        }
    }

    fn from_name(s: &str) -> Option<DigestAlgo> {
        match s {
            "sha256" => Some(DigestAlgo::Sha256),
            "sha512" => Some(DigestAlgo::Sha512),
            _ => None,
        }
    }

    /// The on-ABI `fsverity_digest.digest_algorithm` id for this algorithm — the inverse of
    /// [`DigestAlgo::from_verity_id`]. Lets a closure member's expected digest be compared directly to
    /// a runtime `measure_verity` result `(algo_id, digest)` without re-deriving the enum.
    pub fn to_verity_id(self) -> u16 {
        match self {
            DigestAlgo::Sha256 => 1,
            DigestAlgo::Sha512 => 2,
        }
    }

    /// Digest length in bytes — the manifest hex must be exactly twice this.
    pub fn size(self) -> usize {
        match self {
            DigestAlgo::Sha256 => 32,
            DigestAlgo::Sha512 => 64,
        }
    }
}

/// The behavioural execution class of a pinned artifact (amendment C). `OpenWorld` is defined by
/// behaviour, not enumeration: any profile that lets **mutable / unmeasured bytes become
/// instructions** — an interpreter, a JIT, a plugin/extension loader — is open-world and can never
/// earn a positive band (the no-laundering rule, slice-7 §5.1). The sealed manifest is the authority
/// for this class; a writable-mount binary is never in the compiled-in `CLOSED_WORLD` path list, so
/// without a per-entry class a legitimate pin match would still fail the domain gate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PinClass {
    ClosedWorld,
    OpenWorld,
}

impl PinClass {
    fn from_token(s: &str) -> Option<PinClass> {
        match s {
            "closed-world" => Some(PinClass::ClosedWorld),
            "open-world" => Some(PinClass::OpenWorld),
            _ => None,
        }
    }

    /// True only for a closed-world pin — the sole class eligible to set `domain_execution_sealed`.
    pub fn is_closed_world(self) -> bool {
        self == PinClass::ClosedWorld
    }
}

/// One pinned artifact: the identity tuple `(algo, digest)` plus its behavioural class.
#[derive(Clone, PartialEq, Eq, Debug)]
struct Pin {
    algo: DigestAlgo,
    digest: Vec<u8>,
    class: PinClass,
}

/// One non-entrypoint member of a sealed-dynamic closure (slice-10): the interpreter (`ld.so`) or a
/// transitive `DT_NEEDED` library, identified by fs-verity `(algo, digest)` and by its loader-visible
/// name — an **absolute path** for the interpreter (its `PT_INTERP` pathname, shadow-bound), or a
/// **bare SONAME** for a library (the `DT_NEEDED` name, resolved by the pinned loader under the island
/// lib dir). The constructor binds the pinned inode at that location inside the workload mount ns and
/// re-measures the digest against this record — the manifest digest, not any build-time enumeration, is
/// the authority (I10).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ClosureMember {
    pub algo: DigestAlgo,
    pub digest: Vec<u8>,
    /// Interpreter absolute `PT_INTERP` path, or a library's bare SONAME.
    pub path: String,
}

/// A sealed-dynamic closure (slice-10, F2=B): a dynamically-linked pinned entrypoint plus the
/// **complete** set of objects the loader may map — exactly one pinned interpreter and every pinned
/// transitive `DT_NEEDED` library. This is the object that must remain sealed/pinned; authenticating
/// the entrypoint alone is insufficient (docs/phase5-slice10-sealed-dynamic.md §1). The entry tuple
/// `(entry_algo, entry_digest)` is the lookup key (measured at derivation); `class` must be
/// closed-world to seal.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Closure {
    pub entry_algo: DigestAlgo,
    pub entry_digest: Vec<u8>,
    pub class: PinClass,
    /// Exactly one — the pinned `ld.so` bound at the entrypoint's `PT_INTERP` pathname.
    pub interp: ClosureMember,
    /// Every transitive `DT_NEEDED`, each identity-pinned. May be empty (an interp-only closure).
    pub libs: Vec<ClosureMember>,
}

/// The outcome of looking an entrypoint digest up in the manifest: a slice-8/9 single-inode static
/// pin, or a slice-10 sealed-dynamic closure. Both carry the entry's `PinClass`.
pub enum PinMatch<'a> {
    Static(PinClass),
    Closure(&'a Closure),
}

/// A parsed, validated pin manifest. Constructed only via [`PinManifest::parse`], which fails high on
/// anything malformed — so a `PinManifest` value is always internally consistent (no duplicate keys).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PinManifest {
    pins: Vec<Pin>,
    closures: Vec<Closure>,
}

/// The required first line — a version pin so an older gatekeeper never silently mis-reads a newer
/// (differently-shaped) manifest. A version mismatch fails high. v1 = standalone static pins only
/// (slice-8/9); v2 = v1 plus sealed-dynamic closure records (slice-10). A v1-header manifest that
/// contains a closure record fails high (the older grammar has no `entry`/`interp`/`lib` keyword).
const HEADER_V1: &str = "shrek-pin-manifest v1";
const HEADER_V2: &str = "shrek-pin-manifest v2";

impl PinManifest {
    /// Parse the manifest text. `Ok` only if EVERY line is well-formed; otherwise `Err(reason)` and
    /// the caller must treat it as "no pins". Grammar:
    ///
    /// ```text
    /// shrek-pin-manifest v2                         # v1 = standalone pins only
    /// # comments and blank lines are ignored
    /// <algo> <digest_hex> <exec_class>              # standalone static pin (slice-8/9)
    /// entry  <algo> <digest_hex> <exec_class>       # sealed-dynamic closure entrypoint (opens a block)
    /// interp <algo> <digest_hex> <abs_path>         #   exactly one per block — the pinned ld.so
    /// lib    <algo> <digest_hex> <abs_path>         #   zero+ per block — a transitive DT_NEEDED
    /// ```
    ///
    /// A closure block is opened by an `entry` line and closed by the next `entry`, the next standalone
    /// pin, or EOF. `interp`/`lib` lines attach to the currently-open block. Fail-high triggers:
    /// absent/unknown header, unknown algorithm, hex length ≠ algorithm size, non-lowercase-hex digest,
    /// unknown class, wrong field count, a closure record (`entry`/`interp`/`lib`) under the v1 header,
    /// an `interp`/`lib` with no open `entry`, a block with ≠ 1 `interp`, a non-absolute member path, a
    /// duplicate lookup key (`(algo,digest)` across all standalone pins + closure entries), or a
    /// duplicate member key within one closure.
    pub fn parse(text: &str) -> Result<PinManifest, String> {
        let mut lines = text
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && !l.starts_with('#'));

        let v2 = match lines.next() {
            Some(HEADER_V2) => true,
            Some(HEADER_V1) => false,
            Some(other) => return Err(format!("bad or missing version header: {other:?}")),
            None => return Err("empty manifest (no version header)".to_string()),
        };

        let mut pins: Vec<Pin> = Vec::new();
        let mut closures: Vec<Closure> = Vec::new();
        // The closure block currently being accumulated (opened by `entry`, not yet flushed).
        let mut cur: Option<ClosureBuilder> = None;

        for line in lines {
            let kind = line.split_whitespace().next().unwrap_or("");
            match kind {
                "entry" | "interp" | "lib" if !v2 => {
                    return Err(format!("closure record {kind:?} requires manifest header v2"));
                }
                "entry" => {
                    flush_closure(cur.take(), &mut closures)?;
                    let (algo, digest, class) = parse_pin_fields(line)?;
                    cur = Some(ClosureBuilder { entry_algo: algo, entry_digest: digest, class, interp: None, libs: Vec::new() });
                }
                "interp" | "lib" => {
                    let Some(b) = cur.as_mut() else {
                        return Err(format!("{kind:?} record with no open `entry` block: {line:?}"));
                    };
                    let (algo, digest, path) = parse_member_fields(line, kind == "interp")?;
                    let member = ClosureMember { algo, digest, path };
                    if kind == "interp" {
                        if b.interp.is_some() {
                            return Err(format!("duplicate `interp` in closure for entry {}", hex(&b.entry_digest)));
                        }
                        b.interp = Some(member);
                    } else {
                        b.libs.push(member);
                    }
                }
                _ => {
                    // A standalone static pin closes any open closure block.
                    flush_closure(cur.take(), &mut closures)?;
                    let (algo, digest, class) = parse_pin_fields(line)?;
                    pins.push(Pin { algo, digest, class });
                }
            }
        }
        flush_closure(cur.take(), &mut closures)?;

        // Lookup-key uniqueness across all standalone pins + closure entries (no ambiguous digest).
        let mut keys: Vec<(DigestAlgo, &[u8])> = Vec::new();
        for p in &pins {
            keys.push((p.algo, &p.digest));
        }
        for c in &closures {
            keys.push((c.entry_algo, &c.entry_digest));
        }
        for i in 0..keys.len() {
            for j in (i + 1)..keys.len() {
                if keys[i] == keys[j] {
                    return Err(format!("duplicate pin key for {} {}", algo_name(keys[i].0), hex(keys[i].1)));
                }
            }
        }
        Ok(PinManifest { pins, closures })
    }

    /// Look up a measured `(verity_algo_id, digest)`; returns the entry's class on an exact tuple match
    /// against a standalone pin OR a closure entry. An algorithm id we don't recognise, or a digest of
    /// the wrong length, can never match (fail high). A miss is `None` — the caller derives `T-hostile`.
    pub fn lookup(&self, verity_algo_id: u16, digest: &[u8]) -> Option<PinClass> {
        match self.lookup_match(verity_algo_id, digest)? {
            PinMatch::Static(class) => Some(class),
            PinMatch::Closure(c) => Some(c.class),
        }
    }

    /// Look up a measured entrypoint digest and report WHICH kind of pin it is: a single-inode static
    /// pin (slice-8/9) or a sealed-dynamic closure (slice-10). A standalone pin takes precedence over a
    /// closure at the same key — but lookup-key uniqueness (enforced at parse) guarantees at most one.
    pub fn lookup_match(&self, verity_algo_id: u16, digest: &[u8]) -> Option<PinMatch<'_>> {
        let algo = DigestAlgo::from_verity_id(verity_algo_id)?;
        if digest.len() != algo.size() {
            return None;
        }
        if let Some(p) = self.pins.iter().find(|p| p.algo == algo && p.digest == digest) {
            return Some(PinMatch::Static(p.class));
        }
        self.closures
            .iter()
            .find(|c| c.entry_algo == algo && c.entry_digest == digest)
            .map(PinMatch::Closure)
    }

    /// Count of standalone pins + closure entries — for the audit line and tests only.
    pub fn len(&self) -> usize {
        self.pins.len() + self.closures.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pins.is_empty() && self.closures.is_empty()
    }
}

/// A closure block under construction while parsing (before the `interp`-present check flushes it).
struct ClosureBuilder {
    entry_algo: DigestAlgo,
    entry_digest: Vec<u8>,
    class: PinClass,
    interp: Option<ClosureMember>,
    libs: Vec<ClosureMember>,
}

/// Finalize an accumulated closure block: require exactly one `interp`, member-key uniqueness within
/// the closure, and push it. `None` (no open block) is a no-op.
fn flush_closure(b: Option<ClosureBuilder>, out: &mut Vec<Closure>) -> Result<(), String> {
    let Some(b) = b else { return Ok(()) };
    let interp = b
        .interp
        .ok_or_else(|| format!("closure for entry {} has no `interp`", hex(&b.entry_digest)))?;
    // Member-key uniqueness within this closure (interp + every lib distinct).
    let mut mkeys: Vec<(DigestAlgo, &[u8])> = vec![(interp.algo, &interp.digest)];
    for l in &b.libs {
        mkeys.push((l.algo, &l.digest));
    }
    for i in 0..mkeys.len() {
        for j in (i + 1)..mkeys.len() {
            if mkeys[i] == mkeys[j] {
                return Err(format!("duplicate closure member {} {}", algo_name(mkeys[i].0), hex(mkeys[i].1)));
            }
        }
    }
    out.push(Closure { entry_algo: b.entry_algo, entry_digest: b.entry_digest, class: b.class, interp, libs: b.libs });
    Ok(())
}

/// Parse `<algo> <hex> <class>` (the `entry` keyword, if present, is stripped by the caller position).
/// Accepts an optional leading keyword token (`entry`) and requires exactly the three fields after it.
fn parse_pin_fields(line: &str) -> Result<(DigestAlgo, Vec<u8>, PinClass), String> {
    let mut it = line.split_whitespace();
    // Drop a leading `entry` keyword if present; standalone pins have no keyword.
    let first = it.next();
    let (algo_s, hex_s, class_s) = if first == Some("entry") {
        (it.next(), it.next(), it.next())
    } else {
        (first, it.next(), it.next())
    };
    let extra = it.next();
    let (Some(algo_s), Some(hex_s), Some(class_s)) = (algo_s, hex_s, class_s) else {
        return Err(format!("expected `<algo> <hex> <class>`, got {line:?}"));
    };
    if extra.is_some() {
        return Err(format!("trailing tokens on line {line:?}"));
    }
    let algo = DigestAlgo::from_name(algo_s).ok_or_else(|| format!("unknown digest algorithm {algo_s:?}"))?;
    let digest = decode_hex(hex_s, algo.size())
        .ok_or_else(|| format!("bad {algo_s} digest {hex_s:?} (need {} lowercase hex chars)", algo.size() * 2))?;
    let class = PinClass::from_token(class_s).ok_or_else(|| format!("unknown exec class {class_s:?}"))?;
    Ok((algo, digest, class))
}

/// Parse an `interp`/`lib` line: `<keyword> <algo> <hex> <name>`. `name` is the member's loader-visible
/// identity: an **absolute path** for the interpreter (the `PT_INTERP` pathname it is shadow-bound at),
/// or a **bare SONAME** for a library (no slash — the `DT_NEEDED` name the pinned loader resolves under
/// the island lib dir). The wrong shape for the kind is rejected (fail high).
fn parse_member_fields(line: &str, is_interp: bool) -> Result<(DigestAlgo, Vec<u8>, String), String> {
    let mut it = line.split_whitespace();
    let _keyword = it.next();
    let (Some(algo_s), Some(hex_s), Some(name_s)) = (it.next(), it.next(), it.next()) else {
        return Err(format!("expected `<keyword> <algo> <hex> <name>`, got {line:?}"));
    };
    if it.next().is_some() {
        return Err(format!("trailing tokens on line {line:?}"));
    }
    let algo = DigestAlgo::from_name(algo_s).ok_or_else(|| format!("unknown digest algorithm {algo_s:?}"))?;
    let digest = decode_hex(hex_s, algo.size())
        .ok_or_else(|| format!("bad {algo_s} digest {hex_s:?} (need {} lowercase hex chars)", algo.size() * 2))?;
    if is_interp {
        if !name_s.starts_with('/') {
            return Err(format!("interp path must be absolute, got {name_s:?}"));
        }
    } else {
        // A library SONAME is a bare filename — no path separator (it resolves under the island lib dir).
        if name_s.contains('/') || name_s.is_empty() {
            return Err(format!("lib name must be a bare SONAME (no slash), got {name_s:?}"));
        }
    }
    Ok((algo, digest, name_s.to_string()))
}

fn algo_name(a: DigestAlgo) -> &'static str {
    match a {
        DigestAlgo::Sha256 => "sha256",
        DigestAlgo::Sha512 => "sha512",
    }
}

/// Lowercase-hex a digest for diagnostics only.
fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// Decode exactly `size` bytes of LOWERCASE hex (fail on odd length, wrong length, or any non-hex /
/// uppercase nibble). Lowercase-only keeps the on-disk digest canonical, so two encodings of the same
/// bytes can't split a key.
fn decode_hex(s: &str, size: usize) -> Option<Vec<u8>> {
    if s.len() != size * 2 {
        return None;
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(size);
    let mut i = 0;
    while i < bytes.len() {
        let hi = nibble(bytes[i])?;
        let lo = nibble(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Some(out)
}

/// One lowercase-hex nibble, or `None`. Uppercase is rejected on purpose (canonical form only).
fn nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const D0: &str = "0000000000000000000000000000000000000000000000000000000000000000";
    const D1: &str = "1111111111111111111111111111111111111111111111111111111111111111";

    fn digest(byte: u8) -> Vec<u8> {
        vec![byte; 32]
    }

    #[test]
    fn parses_header_and_entries() {
        let m = PinManifest::parse(&format!(
            "shrek-pin-manifest v1\n# a comment\n\nsha256 {D0} closed-world\nsha256 {D1} open-world\n"
        ))
        .unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m.lookup(1, &digest(0x00)), Some(PinClass::ClosedWorld));
        assert_eq!(m.lookup(1, &digest(0x11)), Some(PinClass::OpenWorld));
    }

    #[test]
    fn empty_but_valid_manifest_has_no_pins() {
        let m = PinManifest::parse("shrek-pin-manifest v1\n# nothing pinned\n").unwrap();
        assert!(m.is_empty());
        assert_eq!(m.lookup(1, &digest(0x00)), None);
    }

    #[test]
    fn missing_or_wrong_header_fails_high() {
        assert!(PinManifest::parse("").is_err());
        assert!(PinManifest::parse("sha256 {D0} closed-world").is_err());
        assert!(PinManifest::parse("shrek-pin-manifest v3\n").is_err()); // unknown FUTURE version
        assert!(PinManifest::parse("# only comments\n").is_err());
        // A header-only v2 manifest is VALID (no pins) — the shipped-empty default.
        assert!(PinManifest::parse("shrek-pin-manifest v2\n").unwrap().is_empty());
    }

    #[test]
    fn unknown_algorithm_fails_high() {
        assert!(PinManifest::parse(&format!("shrek-pin-manifest v1\nmd5 {D0} closed-world\n")).is_err());
    }

    #[test]
    fn wrong_digest_length_fails_high() {
        // sha256 needs 64 hex chars; give it 63 and a sha512 digest under sha256.
        assert!(PinManifest::parse("shrek-pin-manifest v1\nsha256 abc closed-world\n").is_err());
        let long = "a".repeat(128);
        assert!(PinManifest::parse(&format!("shrek-pin-manifest v1\nsha256 {long} closed-world\n")).is_err());
    }

    #[test]
    fn uppercase_hex_is_rejected() {
        let up = "A".repeat(64);
        assert!(PinManifest::parse(&format!("shrek-pin-manifest v1\nsha256 {up} closed-world\n")).is_err());
    }

    #[test]
    fn unknown_class_fails_high() {
        assert!(PinManifest::parse(&format!("shrek-pin-manifest v1\nsha256 {D0} sometimes\n")).is_err());
    }

    #[test]
    fn wrong_field_count_fails_high() {
        assert!(PinManifest::parse(&format!("shrek-pin-manifest v1\nsha256 {D0}\n")).is_err());
        assert!(PinManifest::parse(&format!("shrek-pin-manifest v1\nsha256 {D0} closed-world extra\n")).is_err());
    }

    #[test]
    fn conflicting_duplicate_key_fails_high() {
        // Same (algo,digest), different class — a policy contradiction, not a silent pick.
        assert!(PinManifest::parse(&format!(
            "shrek-pin-manifest v1\nsha256 {D0} closed-world\nsha256 {D0} open-world\n"
        ))
        .is_err());
        // Even an identical duplicate is rejected — a well-formed manifest lists a key once.
        assert!(PinManifest::parse(&format!(
            "shrek-pin-manifest v1\nsha256 {D0} closed-world\nsha256 {D0} closed-world\n"
        ))
        .is_err());
    }

    #[test]
    fn lookup_rejects_unknown_algo_id_and_bad_size() {
        let m = PinManifest::parse(&format!("shrek-pin-manifest v1\nsha256 {D0} closed-world\n")).unwrap();
        assert_eq!(m.lookup(99, &digest(0x00)), None); // unknown verity algo id
        assert_eq!(m.lookup(1, &[0u8; 16]), None); // wrong digest length
        assert_eq!(m.lookup(2, &vec![0u8; 64]), None); // right shape, wrong algo — no such pin
    }

    #[test]
    fn sha512_entries_supported() {
        let d = "2".repeat(128);
        let m = PinManifest::parse(&format!("shrek-pin-manifest v1\nsha512 {d} closed-world\n")).unwrap();
        assert_eq!(m.lookup(2, &vec![0x22u8; 64]), Some(PinClass::ClosedWorld));
    }

    // ---- slice-10: sealed-dynamic closures ----------------------------------------------------

    const DE: &str = "3333333333333333333333333333333333333333333333333333333333333333"; // entry
    const DI: &str = "4444444444444444444444444444444444444444444444444444444444444444"; // interp
    const DL: &str = "5555555555555555555555555555555555555555555555555555555555555555"; // lib

    #[test]
    fn parses_a_closure_and_looks_it_up() {
        let m = PinManifest::parse(&format!(
            "shrek-pin-manifest v2\n\
             entry sha256 {DE} closed-world\n\
             interp sha256 {DI} /lib64/ld-linux-x86-64.so.2\n\
             lib sha256 {DL} libc.so.6\n"
        ))
        .unwrap();
        assert_eq!(m.len(), 1);
        // The entry digest classifies (closed-world) via lookup...
        assert_eq!(m.lookup(1, &digest(0x33)), Some(PinClass::ClosedWorld));
        // ...and lookup_match reveals it is a closure with the right members.
        match m.lookup_match(1, &digest(0x33)) {
            Some(PinMatch::Closure(c)) => {
                assert_eq!(c.interp.path, "/lib64/ld-linux-x86-64.so.2");
                assert_eq!(c.interp.digest, digest(0x44));
                assert_eq!(c.libs.len(), 1);
                assert_eq!(c.libs[0].path, "libc.so.6"); // bare SONAME
                assert_eq!(c.libs[0].digest, digest(0x55));
            }
            _ => panic!("expected a closure match"),
        }
        // A miss is still a miss.
        assert!(m.lookup_match(1, &digest(0x00)).is_none());
    }

    #[test]
    fn standalone_pin_and_closure_coexist_under_v2() {
        let m = PinManifest::parse(&format!(
            "shrek-pin-manifest v2\n\
             sha256 {D0} closed-world\n\
             entry sha256 {DE} closed-world\n\
             interp sha256 {DI} /lib/ld.so\n"
        ))
        .unwrap();
        assert_eq!(m.len(), 2);
        assert!(matches!(m.lookup_match(1, &digest(0x00)), Some(PinMatch::Static(PinClass::ClosedWorld))));
        assert!(matches!(m.lookup_match(1, &digest(0x33)), Some(PinMatch::Closure(_))));
    }

    #[test]
    fn interp_only_closure_is_valid() {
        // A closure with no DT_NEEDED (interp only) is well-formed.
        let m = PinManifest::parse(&format!(
            "shrek-pin-manifest v2\nentry sha256 {DE} closed-world\ninterp sha256 {DI} /lib/ld.so\n"
        ))
        .unwrap();
        match m.lookup_match(1, &digest(0x33)).unwrap() {
            PinMatch::Closure(c) => assert!(c.libs.is_empty()),
            _ => panic!("expected closure"),
        }
    }

    #[test]
    fn closure_records_under_v1_header_fail_high() {
        assert!(PinManifest::parse(&format!("shrek-pin-manifest v1\nentry sha256 {DE} closed-world\n")).is_err());
        assert!(PinManifest::parse(&format!("shrek-pin-manifest v1\ninterp sha256 {DI} /lib/ld.so\n")).is_err());
    }

    #[test]
    fn interp_without_entry_fails_high() {
        assert!(PinManifest::parse(&format!("shrek-pin-manifest v2\ninterp sha256 {DI} /lib/ld.so\n")).is_err());
        assert!(PinManifest::parse(&format!("shrek-pin-manifest v2\nlib sha256 {DL} /lib/x.so\n")).is_err());
    }

    #[test]
    fn closure_without_interp_fails_high() {
        // entry with a following lib but no interp — the loader has no pinned ld.so.
        assert!(PinManifest::parse(&format!(
            "shrek-pin-manifest v2\nentry sha256 {DE} closed-world\nlib sha256 {DL} /lib/x.so\n"
        ))
        .is_err());
        // entry immediately followed by EOF (no interp).
        assert!(PinManifest::parse(&format!("shrek-pin-manifest v2\nentry sha256 {DE} closed-world\n")).is_err());
    }

    #[test]
    fn two_interps_in_one_closure_fail_high() {
        assert!(PinManifest::parse(&format!(
            "shrek-pin-manifest v2\n\
             entry sha256 {DE} closed-world\n\
             interp sha256 {DI} /lib/ld.so\n\
             interp sha256 {DL} /lib/ld2.so\n"
        ))
        .is_err());
    }

    #[test]
    fn relative_member_path_fails_high() {
        assert!(PinManifest::parse(&format!(
            "shrek-pin-manifest v2\nentry sha256 {DE} closed-world\ninterp sha256 {DI} lib/ld.so\n"
        ))
        .is_err());
    }

    #[test]
    fn duplicate_entry_key_fails_high() {
        assert!(PinManifest::parse(&format!(
            "shrek-pin-manifest v2\n\
             entry sha256 {DE} closed-world\ninterp sha256 {DI} /lib/ld.so\n\
             entry sha256 {DE} closed-world\ninterp sha256 {DL} /lib/ld2.so\n"
        ))
        .is_err());
        // An entry digest that also appears as a standalone pin ⇒ ambiguous lookup key ⇒ fail high.
        assert!(PinManifest::parse(&format!(
            "shrek-pin-manifest v2\nsha256 {DE} closed-world\nentry sha256 {DE} closed-world\ninterp sha256 {DI} /lib/ld.so\n"
        ))
        .is_err());
    }

    #[test]
    fn duplicate_member_within_closure_fails_high() {
        // interp digest == a lib digest in the same closure ⇒ fail high.
        assert!(PinManifest::parse(&format!(
            "shrek-pin-manifest v2\n\
             entry sha256 {DE} closed-world\n\
             interp sha256 {DI} /lib/ld.so\n\
             lib sha256 {DI} x.so\n"
        ))
        .is_err());
    }

    #[test]
    fn closure_member_wrong_field_count_fails_high() {
        assert!(PinManifest::parse(&format!("shrek-pin-manifest v2\nentry sha256 {DE} closed-world\ninterp sha256 {DI}\n")).is_err());
        assert!(PinManifest::parse(&format!(
            "shrek-pin-manifest v2\nentry sha256 {DE} closed-world\ninterp sha256 {DI} /lib/ld.so extra\n"
        ))
        .is_err());
    }
}
