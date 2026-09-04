//! dot — the sealed DNS-over-TLS re-pin client (ADR-007 S2c).
//!
//! The supervisor must turn a blessed profile's sealed NAME (e.g. `api.open-meteo.com`) into IPs to pin,
//! but it must NOT ask `resolved`, NetworkManager, `resolv.conf`, `/etc/hosts`, or `getaddrinfo` — all
//! of which uid 1000 can steer (`[R1-MF1]`/`[R2-MF-C]`; the standing #3121 defect). So it resolves over
//! its OWN transport-authenticated channel:
//!
//!   * connect to a SEALED resolver IP on 853 (literal IP — NO name resolution to bootstrap, so nothing
//!     to poison), and
//!   * complete a rustls handshake whose trust base is ONLY the sealed [`ca_path`] bundle (two roots,
//!     not the host store / not webpki) AND whose server name must EXACTLY match the resolver's sealed
//!     hostname ([`RESOLVERS`]). A cert that doesn't chain to a sealed root or doesn't match the sealed
//!     name aborts the handshake.
//!
//! Then a minimal DNS query rides that TLS stream (RFC 7858 framing: a 2-byte length prefix + message).
//! FAIL-CLOSED everywhere: a bad name, a TLS failure, a non-NOERROR/mismatched-id response, or zero A
//! records ⇒ `Err` and NO pin is written (the baked deny skeleton stands). uid 1000 holds authority over
//! none of the three gates (the roots, the IPs, the expected hostname), so it cannot steer the pin even
//! though it owns `/etc/hosts`.
//!
//! The clock bootstrap that makes this possible: `desktop-ntp` uses sealed literal IPs and does NO
//! resolution, so timesyncd sets a sane clock BEFORE this runs — without it the TLS `notBefore`/`After`
//! check here could not pass (`[R2-MF-C]`, the DoT↔clock cycle break).

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rustls::{ClientConfig, ClientConnection, RootCertStore, Stream};
use rustls_pki_types::ServerName;

use crate::store::Pin;

/// A sealed DoT resolver: a literal IP the client dials on 853 and the EXACT TLS hostname its cert must
/// present (verified against the sealed root bundle). Baked in code — sealed policy, like the egress
/// profile table; uid 1000 can neither add a resolver nor change a hostname.
pub struct Resolver {
    pub ip: Ipv4Addr,
    pub hostname: &'static str,
}

/// The sealed resolver set (owner decision: Cloudflare + Quad9, tried in order for redundancy). Each
/// pair's cert chains to a root in `dot-ca-roots.pem` and presents exactly `hostname` — verified live
/// 2026-09-04. Two independent operators ⇒ no single point of resolution failure.
pub const RESOLVERS: &[Resolver] = &[
    Resolver { ip: Ipv4Addr::new(1, 1, 1, 1), hostname: "cloudflare-dns.com" },
    Resolver { ip: Ipv4Addr::new(1, 0, 0, 1), hostname: "cloudflare-dns.com" },
    Resolver { ip: Ipv4Addr::new(9, 9, 9, 9), hostname: "dns.quad9.net" },
    Resolver { ip: Ipv4Addr::new(149, 112, 112, 112), hostname: "dns.quad9.net" },
];

/// The sealed private trust base (two roots). NOT the host CA store. Overridable ONLY in the oracle
/// build via `SHREK_EGRESS_DOT_CA` (mirrors the store's `oracle-env` discipline); the sealed image
/// compiles the override out and always uses the baked path.
pub fn ca_path() -> PathBuf {
    #[cfg(feature = "oracle-env")]
    if let Ok(p) = std::env::var("SHREK_EGRESS_DOT_CA") {
        if !p.is_empty() {
            return p.into();
        }
    }
    "/usr/lib/shrek/dot-ca-roots.pem".into()
}

/// Why a DoT resolution did not yield a pin. Every variant is fail-closed at the call site (no element
/// written); the supervisor records a `resolve-fail` fault.
#[derive(Debug)]
pub enum DotError {
    /// The name is not a valid DNS name (label/length rules) — refused before any network.
    BadName(String),
    /// The sealed CA bundle could not be read/parsed (a build/seal fault, not a runtime steer).
    Ca(String),
    /// Every sealed resolver failed (TLS, I/O, or an empty/rejecting answer). Carries per-resolver
    /// detail for the journal.
    AllFailed(Vec<String>),
}

impl std::fmt::Display for DotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DotError::BadName(n) => write!(f, "invalid DNS name: {n}"),
            DotError::Ca(e) => write!(f, "sealed CA bundle: {e}"),
            DotError::AllFailed(errs) => write!(f, "all sealed resolvers failed: {}", errs.join("; ")),
        }
    }
}

// ---- DNS wire codec (dep-free) ------------------------------------------------------------------

/// A DNS name is a dotted sequence of labels, each 1..=63 bytes, total ≤ 253. The sealed profile hosts
/// satisfy this; we validate anyway (defense in depth) so a malformed sealed entry fails before touching
/// the network rather than emitting a corrupt query.
fn valid_dns_name(name: &str) -> bool {
    let name = name.strip_suffix('.').unwrap_or(name);
    if name.is_empty() || name.len() > 253 {
        return false;
    }
    name.split('.').all(|l| {
        !l.is_empty()
            && l.len() <= 63
            && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
}

/// Build a DNS query for the A records of `name` (RD set). `id` is echoed back and checked.
fn build_a_query(id: u16, name: &str) -> Vec<u8> {
    let mut m = Vec::with_capacity(name.len() + 18);
    m.extend_from_slice(&id.to_be_bytes());
    m.extend_from_slice(&0x0100u16.to_be_bytes()); // flags: RD=1
    m.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    m.extend_from_slice(&0u16.to_be_bytes()); // ANCOUNT
    m.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    m.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    for label in name.strip_suffix('.').unwrap_or(name).split('.') {
        m.push(label.len() as u8);
        m.extend_from_slice(label.as_bytes());
    }
    m.push(0); // root label
    m.extend_from_slice(&1u16.to_be_bytes()); // QTYPE = A
    m.extend_from_slice(&1u16.to_be_bytes()); // QCLASS = IN
    m
}

/// Advance past a DNS name at `pos` (labels, or a compression pointer which terminates the name).
fn skip_name(msg: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        let len = *msg.get(pos)?;
        match len & 0xC0 {
            0x00 => {
                if len == 0 {
                    return Some(pos + 1);
                }
                pos = pos.checked_add(1 + len as usize)?;
                if pos > msg.len() {
                    return None;
                }
            }
            0xC0 => return Some(pos + 2), // pointer: 2 bytes, name ends
            _ => return None,             // reserved bits set ⇒ malformed
        }
    }
}

/// Parse the A records out of a DNS response, fail-closed. Rejects (returns `None`) on: short message,
/// id mismatch, not-a-response, any RCODE ≠ NOERROR, or truncation. Collects every type-A/class-IN RR
/// in the answer section (following whatever CNAME chain the resolver already flattened).
fn parse_a_records(msg: &[u8], expect_id: u16) -> Option<Vec<Ipv4Addr>> {
    if msg.len() < 12 {
        return None;
    }
    if u16::from_be_bytes([msg[0], msg[1]]) != expect_id {
        return None;
    }
    let flags = u16::from_be_bytes([msg[2], msg[3]]);
    if flags & 0x8000 == 0 {
        return None; // not a response
    }
    if flags & 0x000F != 0 {
        return None; // RCODE ≠ NOERROR ⇒ fail-closed
    }
    let qd = u16::from_be_bytes([msg[4], msg[5]]);
    let an = u16::from_be_bytes([msg[6], msg[7]]);
    let mut pos = 12usize;
    for _ in 0..qd {
        pos = skip_name(msg, pos)?;
        pos = pos.checked_add(4)?; // QTYPE + QCLASS
        if pos > msg.len() {
            return None;
        }
    }
    let mut out = Vec::new();
    for _ in 0..an {
        pos = skip_name(msg, pos)?;
        if pos + 10 > msg.len() {
            return None;
        }
        let rtype = u16::from_be_bytes([msg[pos], msg[pos + 1]]);
        let rclass = u16::from_be_bytes([msg[pos + 2], msg[pos + 3]]);
        let rdlen = u16::from_be_bytes([msg[pos + 8], msg[pos + 9]]) as usize;
        pos += 10;
        if pos + rdlen > msg.len() {
            return None;
        }
        if rtype == 1 && rclass == 1 && rdlen == 4 {
            out.push(Ipv4Addr::new(msg[pos], msg[pos + 1], msg[pos + 2], msg[pos + 3]));
        }
        pos += rdlen;
    }
    Some(out)
}

// ---- rustls transport ---------------------------------------------------------------------------

/// Build the rustls client config from ONLY the sealed CA bundle. `default-features=false` + no
/// webpki-roots means the empty store starts with NOTHING; the two sealed roots are the entire trust
/// base. ring provider, TLS 1.2 + 1.3.
fn build_config(ca_pem: &Path) -> Result<Arc<ClientConfig>, DotError> {
    let pem = std::fs::read(ca_pem).map_err(|e| DotError::Ca(format!("read {}: {e}", ca_pem.display())))?;
    let mut rd = &pem[..];
    let mut roots = RootCertStore::empty();
    let mut n = 0usize;
    for c in rustls_pemfile::certs(&mut rd) {
        let cert = c.map_err(|e| DotError::Ca(format!("parse: {e}")))?;
        roots.add(cert).map_err(|e| DotError::Ca(format!("add root: {e}")))?;
        n += 1;
    }
    if n == 0 {
        return Err(DotError::Ca("no roots in sealed bundle".into()));
    }
    let config = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| DotError::Ca(format!("tls versions: {e}")))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// Query one sealed resolver for `name`'s A records over DoT. `Err(String)` on any failure (TLS,
/// I/O, empty/rejecting answer) — the caller tries the next resolver.
fn query_one(
    config: &Arc<ClientConfig>,
    resolver: &Resolver,
    name: &str,
    id: u16,
    timeout: Duration,
) -> Result<Vec<Ipv4Addr>, String> {
    let addr = SocketAddr::from((resolver.ip, 853));
    let mut sock = TcpStream::connect_timeout(&addr, timeout)
        .map_err(|e| format!("{}: connect {e}", resolver.ip))?;
    sock.set_read_timeout(Some(timeout)).ok();
    sock.set_write_timeout(Some(timeout)).ok();

    // EXACT sealed hostname — the cert must present this or the handshake aborts.
    let server_name = ServerName::try_from(resolver.hostname.to_string())
        .map_err(|e| format!("{}: bad server name {}: {e}", resolver.ip, resolver.hostname))?;
    let mut conn = ClientConnection::new(config.clone(), server_name)
        .map_err(|e| format!("{}: tls init {e}", resolver.ip))?;
    let mut tls = Stream::new(&mut conn, &mut sock);

    let query = build_a_query(id, name);
    let mut framed = Vec::with_capacity(query.len() + 2);
    framed.extend_from_slice(&(query.len() as u16).to_be_bytes());
    framed.extend_from_slice(&query);
    tls.write_all(&framed).map_err(|e| format!("{}: write {e}", resolver.ip))?;
    tls.flush().map_err(|e| format!("{}: flush {e}", resolver.ip))?;

    let mut lenbuf = [0u8; 2];
    tls.read_exact(&mut lenbuf).map_err(|e| format!("{}: read len {e}", resolver.ip))?;
    let rlen = u16::from_be_bytes(lenbuf) as usize;
    if rlen == 0 || rlen > 65535 {
        return Err(format!("{}: bad response length {rlen}", resolver.ip));
    }
    let mut resp = vec![0u8; rlen];
    tls.read_exact(&mut resp).map_err(|e| format!("{}: read body {e}", resolver.ip))?;

    match parse_a_records(&resp, id) {
        Some(a) if !a.is_empty() => Ok(a),
        Some(_) => Err(format!("{}: zero A records", resolver.ip)),
        None => Err(format!("{}: malformed/rejecting response", resolver.ip)),
    }
}

/// Resolve `name` to IPv4s over sealed DoT, trying each sealed resolver in order until one answers with
/// ≥1 A record. Fail-closed: bad name / CA fault / all resolvers failed ⇒ `Err`. Result is sorted +
/// de-duplicated. `id` is caller-provided (the sealed daemons avoid rng; over an authenticated single-
/// query TLS stream a fixed id is safe — the transport, not the id, is the security).
pub fn resolve_over_dot(name: &str, id: u16, timeout: Duration) -> Result<Vec<Ipv4Addr>, DotError> {
    if !valid_dns_name(name) {
        return Err(DotError::BadName(name.to_string()));
    }
    let config = build_config(&ca_path())?;
    let mut errs = Vec::new();
    for r in RESOLVERS {
        match query_one(&config, r, name, id, timeout) {
            Ok(mut a) => {
                a.sort();
                a.dedup();
                return Ok(a);
            }
            Err(e) => errs.push(e),
        }
    }
    Err(DotError::AllFailed(errs))
}

/// Resolve every sealed rule host of a PINNABLE profile into [`Pin`]s ready for the store/applier.
/// Refuses a non-pinnable profile (broad / pre-pinned / baseline-empty / unknown) — those are never
/// DoT-resolved. A single host that fails to resolve fails the whole profile (fail-closed: a partial
/// pin set is not applied).
pub fn resolve_profile_pins(profile: &str, id: u16, timeout: Duration) -> Result<Vec<Pin>, DotError> {
    use shrek_policy::desktop_egress::{is_broad_profile, is_prepinned_profile, resolve_desktop};
    let prof = resolve_desktop(profile).ok_or_else(|| DotError::BadName(profile.to_string()))?;
    if is_broad_profile(profile) || is_prepinned_profile(profile) || prof.is_empty() {
        return Err(DotError::BadName(format!("{profile} is not DoT-resolvable")));
    }
    let mut pins = Vec::new();
    for rule in prof.rules {
        let addrs = resolve_over_dot(rule.host, id, timeout)?;
        for addr in addrs {
            pins.push(Pin { name: rule.host.to_string(), addr });
        }
    }
    Ok(pins)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_dns_name_rules() {
        assert!(valid_dns_name("api.open-meteo.com"));
        assert!(valid_dns_name("cloudflare-dns.com"));
        assert!(!valid_dns_name(""));
        assert!(!valid_dns_name("a..b"));
        assert!(!valid_dns_name(&"x".repeat(64))); // label too long
        assert!(!valid_dns_name("evil.example/../etc")); // slash not allowed
    }

    #[test]
    fn build_query_shape() {
        let q = build_a_query(0x1234, "a.bc");
        // header id
        assert_eq!(&q[0..2], &[0x12, 0x34]);
        assert_eq!(&q[2..4], &[0x01, 0x00]); // RD
        assert_eq!(&q[4..6], &[0x00, 0x01]); // QDCOUNT=1
        // qname: 1 'a' 2 'b' 'c' 0
        assert_eq!(&q[12..], &[1, b'a', 2, b'b', b'c', 0, 0, 1, 0, 1]);
    }

    /// Assemble a minimal response: echo the question + two A answers (with a compression pointer to
    /// the question name, the common resolver encoding), and prove we extract both IPs.
    #[test]
    fn parse_two_a_records_with_compression() {
        let id: u16 = 0xABCD;
        let mut m = Vec::new();
        m.extend_from_slice(&id.to_be_bytes());
        m.extend_from_slice(&0x8180u16.to_be_bytes()); // QR=1, RD=1, RA=1, RCODE=0
        m.extend_from_slice(&1u16.to_be_bytes()); // QD
        m.extend_from_slice(&2u16.to_be_bytes()); // AN=2
        m.extend_from_slice(&0u16.to_be_bytes());
        m.extend_from_slice(&0u16.to_be_bytes());
        // question: "a.b" A IN  (name starts at offset 12)
        m.extend_from_slice(&[1, b'a', 1, b'b', 0]);
        m.extend_from_slice(&1u16.to_be_bytes());
        m.extend_from_slice(&1u16.to_be_bytes());
        // answer 1: pointer to 12, A IN ttl=60 rdlen=4 1.2.3.4
        for (t, c, ttl, rd, ip) in [
            (1u16, 1u16, 60u32, 4u16, [1u8, 2, 3, 4]),
            (1, 1, 60, 4, [5, 6, 7, 8]),
        ] {
            m.extend_from_slice(&[0xC0, 12]); // compression pointer to question name
            m.extend_from_slice(&t.to_be_bytes());
            m.extend_from_slice(&c.to_be_bytes());
            m.extend_from_slice(&ttl.to_be_bytes());
            m.extend_from_slice(&rd.to_be_bytes());
            m.extend_from_slice(&ip);
        }
        let got = parse_a_records(&m, id).unwrap();
        assert_eq!(got, vec![Ipv4Addr::new(1, 2, 3, 4), Ipv4Addr::new(5, 6, 7, 8)]);
    }

    #[test]
    fn parse_fails_closed() {
        // id mismatch
        let mut m = vec![0, 1];
        m.extend_from_slice(&[0x81, 0x80, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(parse_a_records(&m, 0xFFFF), None);
        // NXDOMAIN (rcode=3)
        let mut nx = 0xABCDu16.to_be_bytes().to_vec();
        nx.extend_from_slice(&0x8183u16.to_be_bytes());
        nx.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(parse_a_records(&nx, 0xABCD), None);
        // too short
        assert_eq!(parse_a_records(&[0, 0], 0), None);
        // not a response (QR=0)
        let mut q = 0x1u16.to_be_bytes().to_vec();
        q.extend_from_slice(&0x0100u16.to_be_bytes());
        q.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(parse_a_records(&q, 1), None);
    }

    #[test]
    fn resolvers_are_the_sealed_four() {
        assert_eq!(RESOLVERS.len(), 4);
        assert!(RESOLVERS.iter().any(|r| r.ip == Ipv4Addr::new(1, 1, 1, 1) && r.hostname == "cloudflare-dns.com"));
        assert!(RESOLVERS.iter().any(|r| r.ip == Ipv4Addr::new(9, 9, 9, 9) && r.hostname == "dns.quad9.net"));
    }

    #[test]
    fn resolve_profile_refuses_non_pinnable() {
        // broad / pre-pinned / unknown are never DoT-resolved (no network hit — errors before any I/O).
        for p in ["web-browsing", "desktop-ntp", "desktop-updates", "evil"] {
            assert!(resolve_profile_pins(p, 0, Duration::from_millis(1)).is_err(), "{p} must be refused");
        }
    }
}
