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

/// A parsed, validated pin manifest. Constructed only via [`PinManifest::parse`], which fails high on
/// anything malformed — so a `PinManifest` value is always internally consistent (no duplicate keys).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PinManifest {
    pins: Vec<Pin>,
}

/// The required first line — a version pin so an older gatekeeper never silently mis-reads a newer
/// (differently-shaped) manifest. A version mismatch fails high.
const HEADER_V1: &str = "shrek-pin-manifest v1";

impl PinManifest {
    /// Parse the manifest text. `Ok` only if EVERY line is well-formed; otherwise `Err(reason)` and
    /// the caller must treat it as "no pins". Grammar (v1):
    ///
    /// ```text
    /// shrek-pin-manifest v1
    /// # comments and blank lines are ignored
    /// <algo> <digest_hex> <exec_class>
    /// ```
    ///
    /// Fail-high triggers: absent/unknown header, unknown algorithm, hex length ≠ algorithm size,
    /// non-lowercase-hex digest, unknown class, wrong field count, or a duplicate `(algo,digest)` key
    /// (a conflict — no last-writer-wins).
    pub fn parse(text: &str) -> Result<PinManifest, String> {
        let mut lines = text
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && !l.starts_with('#'));

        match lines.next() {
            Some(HEADER_V1) => {}
            Some(other) => return Err(format!("bad or missing version header: {other:?}")),
            None => return Err("empty manifest (no version header)".to_string()),
        }

        let mut pins: Vec<Pin> = Vec::new();
        for line in lines {
            let mut it = line.split_whitespace();
            let (algo_s, hex_s, class_s, extra) = (it.next(), it.next(), it.next(), it.next());
            let (Some(algo_s), Some(hex_s), Some(class_s)) = (algo_s, hex_s, class_s) else {
                return Err(format!("expected `<algo> <hex> <class>`, got {line:?}"));
            };
            if extra.is_some() {
                return Err(format!("trailing tokens on line {line:?}"));
            }
            let algo = DigestAlgo::from_name(algo_s)
                .ok_or_else(|| format!("unknown digest algorithm {algo_s:?}"))?;
            let digest = decode_hex(hex_s, algo.size())
                .ok_or_else(|| format!("bad {algo_s} digest {hex_s:?} (need {} lowercase hex chars)", algo.size() * 2))?;
            let class = PinClass::from_token(class_s)
                .ok_or_else(|| format!("unknown exec class {class_s:?}"))?;

            // Conflict / duplicate key ⇒ fail high (amendment B). Same (algo,digest) must appear once.
            if pins.iter().any(|p| p.algo == algo && p.digest == digest) {
                return Err(format!("duplicate pin for {algo_s} {hex_s}"));
            }
            pins.push(Pin { algo, digest, class });
        }
        Ok(PinManifest { pins })
    }

    /// Look up a measured `(verity_algo_id, digest)`; returns the pin's class on an exact tuple match.
    /// An algorithm id we don't recognise, or a digest of the wrong length, can never match (fail
    /// high). A miss is `None` — the caller derives `T-hostile`.
    pub fn lookup(&self, verity_algo_id: u16, digest: &[u8]) -> Option<PinClass> {
        let algo = DigestAlgo::from_verity_id(verity_algo_id)?;
        if digest.len() != algo.size() {
            return None;
        }
        self.pins
            .iter()
            .find(|p| p.algo == algo && p.digest == digest)
            .map(|p| p.class)
    }

    /// Count of pins — for the audit line and tests only.
    pub fn len(&self) -> usize {
        self.pins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pins.is_empty()
    }
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
        assert!(PinManifest::parse("shrek-pin-manifest v2\n").is_err());
        assert!(PinManifest::parse("# only comments\n").is_err());
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
}
