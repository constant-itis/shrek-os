//! ingest_admit — the sealed, integrity-bound untrusted-ingest admission (Phase-6 slice-1a).
//!
//! This is the first minimal slice of the deferred §4 provenance store that
//! [`shrek_policy::Origin::UntrustedIngest`] documents ("an affirmative, integrity-checked record …
//! needs the §4 mutable provenance log; never produced in the MVP"). It produces exactly ONE origin
//! fact — `UntrustedIngest` — and only under an INTEGRITY proof, never from a caller assertion:
//!
//!   A session may be classified `Origin::UntrustedIngest` — earning `T-untrust` instead of the
//!   `T-hostile` floor — ONLY when the sealed T2 containment harness that will execute it is
//!   integrity-authentic: the `runsc` harness binary's fs-verity digest is present in the sealed
//!   admit-list. The band and the wall are coupled through integrity — you may treat code as merely
//!   untrusted (a weaker wall than hostile) precisely because an authenticated gVisor harness exists
//!   to contain it.
//!
//! Fail-high is total and mirrors [`crate::provenance_plane`]: a missing/empty/malformed admit-list, a
//! harness with no fs-verity, a measurement error, or a digest miss ⇒ `Origin::None` ⇒ the FROZEN
//! [`shrek_policy::derive_band`] returns `T-hostile`. This module supplies the integrity-checked origin
//! fact; it does NOT re-implement the lattice and does NOT touch `derive_band`.
//!
//! Override discipline: the admit-list PATH follows the same ORACLE-ONLY convention as the T2 harness
//! substrate (`t2_plane::sealed_{runsc,rootfs}_path`, `SHREK_T2_*`) — a sealed image sets no env, so
//! production reads only the compiled-in, dm-verity-`/usr` default. The decision itself is a kernel
//! fs-verity measurement, never an env value; the override only relocates the sealed file for the
//! oracle, exactly as `SHREK_T2_RUNSC` relocates the harness binary it authenticates.

use crate::linux_uapi::measure_verity;
use crate::pin_manifest::DigestAlgo;
use shrek_policy::{derive_band, Evidence, Origin, TrustBand};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

/// The sealed admit-list: the fs-verity identities of the authorised T2 untrusted-ingest harness
/// binaries. Baked under the dm-verity `/usr` at image build (`seal-t2-artifacts.sh`), so changing it
/// requires a signed image update — §4-static custody, exactly like the pin-manifest.
const INGEST_ADMIT_PATH: &str = "/usr/lib/shrek/t2-ingest-admit";
const HEADER_V1: &str = "shrek-t2-ingest-admit v1";

/// Resolve the admit-list path. `SHREK_INGEST_ADMIT` is honoured ONLY as the oracle counterpart to the
/// `SHREK_T2_*` substrate overrides — production (the sealed image) sets no env and reads the
/// compiled-in default under the read-only, roothash-authenticated `/usr`.
pub fn admit_path() -> PathBuf {
    std::env::var_os("SHREK_INGEST_ADMIT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(INGEST_ADMIT_PATH))
}

/// A parsed admit-list: authorised `(algo, digest)` harness identities. An empty list (header only) is
/// the shipped default and admits nothing.
struct AdmitList {
    entries: Vec<(DigestAlgo, Vec<u8>)>,
}

impl AdmitList {
    /// Parse `shrek-t2-ingest-admit v1` + one `<algo> <hexdigest>` per line (`#` comments / blank lines
    /// ignored). FAIL-HIGH: any malformed line rejects the WHOLE list (returns `Err`) rather than
    /// silently admitting a subset — a corrupt sealed policy must deny, never optimistically allow.
    fn parse(text: &str) -> Result<AdmitList, String> {
        let mut lines = text
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty() && !l.starts_with('#'));
        match lines.next() {
            Some(HEADER_V1) => {}
            Some(other) => return Err(format!("bad or missing version header: {other:?}")),
            None => return Err("empty admit-list (no version header)".to_string()),
        }
        let mut entries = Vec::new();
        for line in lines {
            let mut it = line.split_whitespace();
            let algo_tok = it.next().unwrap_or("");
            let hex_tok = it.next().unwrap_or("");
            if it.next().is_some() {
                return Err(format!("trailing tokens in admit line: {line:?}"));
            }
            let algo = match algo_tok {
                "sha256" => DigestAlgo::Sha256,
                "sha512" => DigestAlgo::Sha512,
                other => return Err(format!("unknown digest algorithm: {other:?}")),
            };
            let digest = decode_hex(hex_tok).ok_or_else(|| format!("bad hex digest: {hex_tok:?}"))?;
            if digest.len() != algo.size() {
                return Err(format!(
                    "digest length {} != {} bytes for {algo_tok}",
                    digest.len(),
                    algo.size()
                ));
            }
            entries.push((algo, digest));
        }
        Ok(AdmitList { entries })
    }

    /// Exact `(algo, digest)` membership. A recognised measured algorithm whose digest is byte-equal to
    /// an authorised entry ⇒ authentic. Anything else ⇒ not authentic (fail-high upstream).
    fn contains(&self, algo: DigestAlgo, digest: &[u8]) -> bool {
        self.entries.iter().any(|(a, d)| *a == algo && d.as_slice() == digest)
    }
}

/// Decode an even-length lowercase/uppercase hex string to bytes; `None` on any non-hex nibble or odd
/// length (fail-high: an unparseable digest can never match).
fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if s.is_empty() || s.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let b = s.as_bytes();
    let nib = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    let mut i = 0;
    while i < b.len() {
        out.push((nib(b[i])? << 4) | nib(b[i + 1])?);
        i += 2;
    }
    Some(out)
}

/// The outcome of an ingest admission, for the derivation + the audit line.
pub struct IngestDerivation {
    /// The band from the FROZEN [`derive_band`] over the origin fact this module established.
    pub band: TrustBand,
    /// Whether the harness measured authentic against the sealed admit-list (the sole gate on
    /// `UntrustedIngest`). `false` ⇒ origin `None` ⇒ `T-hostile`.
    pub harness_authentic: bool,
    /// The harness's measured fs-verity digest (hex), for the audit line — `None` if it did not
    /// measure (no verity / error).
    pub measured_hex: Option<String>,
}

/// Load the sealed admit-list. `None` on absence (shipped default ⇒ admit nothing) OR on ANY parse
/// error (fail-high — a malformed sealed policy denies), logged so it is diagnosable.
fn load_admit() -> Option<AdmitList> {
    let path = admit_path();
    let text = std::fs::read_to_string(&path).ok()?;
    match AdmitList::parse(&text) {
        Ok(l) => Some(l),
        Err(e) => {
            eprintln!("gatekeeperd/ingest_admit: ADMIT-LIST REJECTED (fail-high, admit nothing): {e}");
            None
        }
    }
}

/// Measure the harness binary's fs-verity `(algo, digest)`. Opens `O_RDONLY` (the ioctl is rejected on
/// `O_PATH`); any open/measure failure ⇒ `None` (fail-high). Not TOCTOU-relevant to identity: fs-verity
/// binds the digest to the file content the kernel will actually serve on every read.
fn measure_harness(runsc: &Path) -> Option<(u16, Vec<u8>)> {
    let fd = std::fs::File::open(runsc).ok()?;
    measure_verity(fd.as_raw_fd()).ok()
}

/// Derive the trust band for a T2 untrusted-ingest harness session from INTEGRITY EVIDENCE only.
///
/// `runsc` is the harness binary gatekeeperd is about to drive (its sealed substrate path). If its
/// fs-verity digest is in the sealed admit-list, the session's origin is the affirmative
/// `UntrustedIngest`; otherwise `None`. The band is then the frozen [`derive_band`] over an `Evidence`
/// whose only positive fact is that origin — so a bad/missing/mismatched harness yields `T-hostile`,
/// and an authentic one yields exactly `T-untrust` (never `T-first`/`T-pinned`: neither sealed-domain
/// nor pin fact is asserted here).
pub fn derive_session(runsc: &Path) -> IngestDerivation {
    let measured = measure_harness(runsc);
    let measured_hex = measured.as_ref().map(|(_, d)| hex(d));
    let harness_authentic = match (load_admit(), &measured) {
        (Some(list), Some((algo_id, digest))) => match DigestAlgo::from_verity_id(*algo_id) {
            Some(algo) => list.contains(algo, digest),
            None => false,
        },
        _ => false,
    };
    let origin = if harness_authentic { Origin::UntrustedIngest } else { Origin::None };
    let evidence = Evidence {
        entrypoint_sealed: false,
        domain_execution_sealed: false,
        pinned_digest_match: false,
        origin,
    };
    IngestDerivation { band: derive_band(&evidence), harness_authentic, measured_hex }
}

/// Lowercase hex, for the audit line.
fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const D0: &str = "0000000000000000000000000000000000000000000000000000000000000000";
    const D1: &str = "1111111111111111111111111111111111111111111111111111111111111111";

    #[test]
    fn parse_admits_listed_digests_and_rejects_others() {
        let l = AdmitList::parse(&format!("{HEADER_V1}\n# harness\nsha256 {D0}\n")).unwrap();
        let d0 = decode_hex(D0).unwrap();
        let d1 = decode_hex(D1).unwrap();
        assert!(l.contains(DigestAlgo::Sha256, &d0));
        assert!(!l.contains(DigestAlgo::Sha256, &d1));
        // Right digest, wrong algorithm ⇒ never a match.
        assert!(!l.contains(DigestAlgo::Sha512, &d0));
    }

    #[test]
    fn empty_list_admits_nothing() {
        let l = AdmitList::parse(&format!("{HEADER_V1}\n")).unwrap();
        assert!(!l.contains(DigestAlgo::Sha256, &decode_hex(D0).unwrap()));
    }

    #[test]
    fn missing_header_and_bad_lines_fail_high() {
        assert!(AdmitList::parse("sha256 whatever\n").is_err());
        assert!(AdmitList::parse("").is_err());
        // Wrong-length digest for the named algorithm.
        assert!(AdmitList::parse(&format!("{HEADER_V1}\nsha256 abcd\n")).is_err());
        // Unknown algorithm.
        assert!(AdmitList::parse(&format!("{HEADER_V1}\nmd5 {D0}\n")).is_err());
        // Trailing garbage.
        assert!(AdmitList::parse(&format!("{HEADER_V1}\nsha256 {D0} extra\n")).is_err());
    }

    #[test]
    fn sha512_entries_parse_at_correct_length() {
        let d = "ab".repeat(64); // 64 bytes = sha512
        let l = AdmitList::parse(&format!("{HEADER_V1}\nsha512 {d}\n")).unwrap();
        assert!(l.contains(DigestAlgo::Sha512, &decode_hex(&d).unwrap()));
    }

    #[test]
    fn decode_hex_rejects_odd_and_nonhex() {
        assert!(decode_hex("abc").is_none());
        assert!(decode_hex("zz").is_none());
        assert!(decode_hex("").is_none());
        assert_eq!(decode_hex("00ff").unwrap(), vec![0x00, 0xff]);
    }

    #[test]
    fn origin_fact_maps_through_frozen_derive_band() {
        // The admission's contract with the lattice: UntrustedIngest⇒Untrust, None⇒Hostile. Guards
        // against a future edit that lets this module assert a stronger band than the origin fact earns.
        let admit = Evidence { entrypoint_sealed: false, domain_execution_sealed: false, pinned_digest_match: false, origin: Origin::UntrustedIngest };
        assert_eq!(derive_band(&admit), TrustBand::Untrust);
        let deny = Evidence { origin: Origin::None, ..admit };
        assert_eq!(derive_band(&deny), TrustBand::Hostile);
    }
}
