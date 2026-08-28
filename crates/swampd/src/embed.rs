//! embed — the pluggable embedding-provider abstraction (slice-4, `docs/phase6-swamp-slice4-semantic-
//! provider.md`). This is the IN-BASE surface of the semantic tier: the versioned backend interface, a
//! deterministic chunker, and a socket-framed backend that speaks to an OFF-IMAGE broker-side proxy.
//!
//! What is deliberately NOT here (the sealed-base surface bound, slice-4 §1.3):
//!   - no HTTP client, no TLS, no DNS — swampd never dials the network (`confine.rs` keeps
//!     `handled_access_net = 0`). [`SocketBackend`] speaks a compact length-prefixed binary framing to a
//!     LOCAL unix socket named by a sealed provider-profile; the `swamp-embed-proxy` (off-image,
//!     excluded from workspace `default-members`) is the only party that reaches the LAN provider.
//!   - no model weights, no inference deps — the embedding runtime is the PROVIDER (the LAN service),
//!     never in the sealed image.
//! The base therefore links ZERO new crates for this tier: the framing is hand-rolled over `std`, the
//! hash is inline FNV-1a, similarity/storage live in `index.rs` over the already-bundled sqlite.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

/// A backend's versioned identity — the four components of the `semantic_version` rebuild key
/// (slice-4 §2/§5): a change in any of them forces a wipe+re-embed (`index.rs::reconcile_semantic_
/// version`), because vectors from a different provider/model/dim/interface are not comparable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendIdentity {
    pub provider_id: String,
    pub model_id: String,
    pub dim: u32,
    /// The semantic-tier interface/schema version (bumped when the chunking or wire semantics change).
    pub version: u32,
}

impl BackendIdentity {
    /// The `provider|model|dim|version` string persisted as `meta.semantic_version` and stamped onto
    /// every stored vector. `|` cannot appear in the numeric fields; provider/model ids are sealed
    /// config, so the join is unambiguous.
    pub fn semantic_version(&self) -> String {
        format!("{}|{}|{}|{}", self.provider_id, self.model_id, self.dim, self.version)
    }
}

/// An embedding backend produces one `dim`-length vector per input chunk. It is UNTRUSTED FOR AUTHORITY
/// (slice-4 §1.2 T6): it only scores objects already inside the caller's authorized candidate set; it
/// can never widen scope. A failure degrades the query to lexical FTS (§1.2 T7) — it is never fatal.
pub trait EmbeddingBackend {
    fn identity(&self) -> BackendIdentity;
    /// Embed a batch of chunk texts, order-preserving. On ANY error the caller drops to FTS for the
    /// affected work and reports `semantic unavailable` — never a hard failure.
    fn embed(&self, chunks: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError>;
}

/// The runtime semantic context threaded through enrichment and query handling: a live backend plus the
/// current `semantic_version` stamped onto stored vectors. `None` of this (no `SemanticCtx`) is exactly
/// the "no provider" state — enrichment writes no vectors and queries report `semantic unavailable`,
/// serving the FTS floor. Its presence never widens authority; it only enables scoring.
pub struct SemanticCtx<'a> {
    pub backend: &'a dyn EmbeddingBackend,
    pub semantic_version: String,
}

impl SemanticCtx<'_> {
    /// Embed one query string into a single vector, or `None` if the provider is unreachable/erroring —
    /// the caller then degrades that query to lexical FTS and reports `semantic unavailable` (§1.2 T7).
    pub fn embed_query(&self, terms: &str) -> Option<Vec<f32>> {
        self.backend.embed(&[terms]).ok()?.into_iter().next()
    }
}

#[derive(Debug)]
pub enum EmbedError {
    /// The proxy socket could not be reached / spoke a malformed frame / timed out. Degrade to FTS.
    Unavailable(String),
    /// The provider returned a vector whose width is not the backend's declared `dim`. Reject (T6).
    DimMismatch { got: usize, want: u32 },
}

impl std::fmt::Display for EmbedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmbedError::Unavailable(s) => write!(f, "embedding provider unavailable: {s}"),
            EmbedError::DimMismatch { got, want } => write!(f, "embedding dim {got} != declared {want}"),
        }
    }
}

// ─── SocketBackend — the in-base client to the off-image proxy ──────────────────────────────────────
//
// Wire framing (swampd ⇄ swamp-embed-proxy), plaintext over a LOCAL unix socket. Deliberately a tiny
// hand-rolled binary protocol so the base carries no JSON/HTTP dependency (§1.3):
//
//   REQUEST:   u32 n                         (chunk count, LE)
//              for each chunk: u32 len, len bytes (UTF-8)
//   RESPONSE:  u8  status                    (1 = ok, 0 = provider error → EmbedError::Unavailable)
//              u32 dim                        (floats per vector, LE)
//              for each chunk: dim × f32      (LE) — n vectors, order-matching the request
//
// The proxy owns all provider variance (OpenAI /v1/embeddings translation, the gated LAN egress). It is
// off-image; swampd only knows the socket path (a sealed provider-profile) + this framing.

pub struct SocketBackend {
    socket: PathBuf,
    identity: BackendIdentity,
    timeout: Duration,
}

impl SocketBackend {
    pub fn new(socket: PathBuf, identity: BackendIdentity) -> SocketBackend {
        SocketBackend { socket, identity, timeout: Duration::from_secs(30) }
    }
}

impl EmbeddingBackend for SocketBackend {
    fn identity(&self) -> BackendIdentity {
        self.identity.clone()
    }

    fn embed(&self, chunks: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if chunks.is_empty() {
            return Ok(Vec::new());
        }
        let mut stream = UnixStream::connect(&self.socket)
            .map_err(|e| EmbedError::Unavailable(format!("connect {}: {e}", self.socket.display())))?;
        let _ = stream.set_read_timeout(Some(self.timeout));
        let _ = stream.set_write_timeout(Some(self.timeout));
        let req = encode_request(chunks);
        stream.write_all(&req).map_err(|e| EmbedError::Unavailable(format!("write: {e}")))?;
        stream.flush().ok();
        let mut resp = Vec::new();
        stream.read_to_end(&mut resp).map_err(|e| EmbedError::Unavailable(format!("read: {e}")))?;
        decode_response(&resp, chunks.len(), self.identity.dim)
    }
}

/// Encode the request frame (see the wire framing above). Pure — unit-tested without a socket.
fn encode_request(chunks: &[&str]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
    for c in chunks {
        let b = c.as_bytes();
        buf.extend_from_slice(&(b.len() as u32).to_le_bytes());
        buf.extend_from_slice(b);
    }
    buf
}

/// Decode the response frame into `n` vectors of the declared `dim`. Any short/oversized/status-0 frame
/// or a dim mismatch is an error → the caller degrades to FTS (never a panic, never a wrong vector).
fn decode_response(resp: &[u8], n: usize, dim: u32) -> Result<Vec<Vec<f32>>, EmbedError> {
    if resp.len() < 5 {
        return Err(EmbedError::Unavailable(format!("short frame ({} bytes)", resp.len())));
    }
    if resp[0] != 1 {
        return Err(EmbedError::Unavailable("provider reported error (status 0)".into()));
    }
    let got_dim = u32::from_le_bytes([resp[1], resp[2], resp[3], resp[4]]);
    if got_dim != dim {
        return Err(EmbedError::DimMismatch { got: got_dim as usize, want: dim });
    }
    let want = 5 + n * dim as usize * 4;
    if resp.len() != want {
        return Err(EmbedError::Unavailable(format!("frame len {} != expected {want}", resp.len())));
    }
    let mut out = Vec::with_capacity(n);
    let mut off = 5;
    for _ in 0..n {
        let mut v = Vec::with_capacity(dim as usize);
        for _ in 0..dim {
            v.push(f32::from_le_bytes([resp[off], resp[off + 1], resp[off + 2], resp[off + 3]]));
            off += 4;
        }
        out.push(v);
    }
    Ok(out)
}

// ─── Deterministic chunking (slice-4 §5, Fork F5) ────────────────────────────────────────────────────
//
// Stable, reproducible boundaries that are a pure function of the object's extracted text, so re-chunking
// after an edit is idempotent and chunk identity (object_id, ordinal) is stable. Windows are sized in
// CHARS (never splitting a UTF-8 boundary) to stay safely under the provider's token cap — a prior
// retrieval bakeoff saw ~11% of DENSE docs exceed EmbeddingGemma's 2048-token cap at ~1.28 tok/char, so a ~1200-
// char window (~1500 tokens worst case) leaves margin. A fixed overlap preserves cross-boundary context.

const CHUNK_CHARS: usize = 1200;
const OVERLAP_CHARS: usize = 150;

/// One deterministic chunk of an object's text: its order, byte span into the extracted text, the text
/// itself, and a stable content hash for incremental-skip.
pub struct Chunk {
    pub ordinal: i64,
    pub byte_start: usize,
    pub byte_end: usize,
    pub text: String,
    pub text_hash: String,
}

/// Split `text` into deterministic overlapping windows. Empty/whitespace-only text yields no chunks.
pub fn chunk_text(text: &str) -> Vec<Chunk> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    // Byte offset of every char boundary, plus the end — lets us window on CHARS but record BYTE spans.
    let mut bounds: Vec<usize> = text.char_indices().map(|(i, _)| i).collect();
    bounds.push(text.len());
    let nchars = bounds.len() - 1;
    let step = CHUNK_CHARS - OVERLAP_CHARS; // > 0 by construction
    let mut chunks = Vec::new();
    let mut start_char = 0;
    let mut ordinal = 0i64;
    while start_char < nchars {
        let end_char = (start_char + CHUNK_CHARS).min(nchars);
        let bs = bounds[start_char];
        let be = bounds[end_char];
        let slice = &text[bs..be];
        chunks.push(Chunk {
            ordinal,
            byte_start: bs,
            byte_end: be,
            text: slice.to_string(),
            text_hash: fnv1a_hex(slice),
        });
        ordinal += 1;
        if end_char == nchars {
            break;
        }
        start_char += step;
    }
    chunks
}

/// FNV-1a 64-bit, hex-encoded — a small, dependency-free, run-stable content hash (unlike the randomized
/// `DefaultHasher`). Used only to skip re-embedding an unchanged chunk; collisions merely miss a skip.
fn fnv1a_hex(s: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_version_string_is_stable_and_joined() {
        let id = BackendIdentity {
            provider_id: "local-lan".into(),
            model_id: "embeddinggemma-300m".into(),
            dim: 768,
            version: 1,
        };
        assert_eq!(id.semantic_version(), "local-lan|embeddinggemma-300m|768|1");
    }

    #[test]
    fn request_frame_roundtrips_shape() {
        let req = encode_request(&["ab", "cde"]);
        // n=2, then (len=2,"ab"), (len=3,"cde")
        assert_eq!(&req[0..4], &2u32.to_le_bytes());
        assert_eq!(&req[4..8], &2u32.to_le_bytes());
        assert_eq!(&req[8..10], b"ab");
        assert_eq!(&req[10..14], &3u32.to_le_bytes());
        assert_eq!(&req[14..17], b"cde");
        assert_eq!(req.len(), 4 + (4 + 2) + (4 + 3));
    }

    fn build_ok_response(vecs: &[Vec<f32>], dim: u32) -> Vec<u8> {
        let mut b = vec![1u8];
        b.extend_from_slice(&dim.to_le_bytes());
        for v in vecs {
            for x in v {
                b.extend_from_slice(&x.to_le_bytes());
            }
        }
        b
    }

    #[test]
    fn response_frame_decodes_vectors_in_order() {
        let vecs = vec![vec![1.0f32, 2.0], vec![3.0, 4.0]];
        let resp = build_ok_response(&vecs, 2);
        let out = decode_response(&resp, 2, 2).unwrap();
        assert_eq!(out, vecs);
    }

    #[test]
    fn response_frame_rejects_status_error_short_and_dim_mismatch() {
        // status 0 → Unavailable (degrade to FTS)
        assert!(matches!(decode_response(&[0, 2, 0, 0, 0], 1, 2), Err(EmbedError::Unavailable(_))));
        // too short → Unavailable
        assert!(matches!(decode_response(&[1, 2], 1, 2), Err(EmbedError::Unavailable(_))));
        // dim mismatch → DimMismatch (T6 reject)
        let resp = build_ok_response(&[vec![1.0, 2.0, 3.0]], 3);
        assert!(matches!(decode_response(&resp, 1, 768), Err(EmbedError::DimMismatch { got: 3, want: 768 })));
        // right dim but wrong length (missing a vector) → Unavailable
        let resp = build_ok_response(&[vec![1.0, 2.0]], 2);
        assert!(matches!(decode_response(&resp, 2, 2), Err(EmbedError::Unavailable(_))));
    }

    #[test]
    fn chunking_is_deterministic_and_reproducible() {
        let text = "lorem ipsum ".repeat(400); // ~4800 chars → multiple windows
        let a = chunk_text(&text);
        let b = chunk_text(&text);
        assert!(a.len() > 1, "long text should produce multiple chunks");
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.ordinal, y.ordinal);
            assert_eq!((x.byte_start, x.byte_end), (y.byte_start, y.byte_end));
            assert_eq!(x.text_hash, y.text_hash);
        }
    }

    #[test]
    fn chunk_boundaries_overlap_and_cover_and_are_byte_exact() {
        let text = "lorem ipsum ".repeat(400);
        let chunks = chunk_text(&text);
        // First window starts at 0; each chunk's recorded span slices exactly to its stored text.
        assert_eq!(chunks[0].byte_start, 0);
        for c in &chunks {
            assert_eq!(&text[c.byte_start..c.byte_end], c.text);
        }
        // Last chunk reaches the end of the text.
        assert_eq!(chunks.last().unwrap().byte_end, text.len());
        // Consecutive windows overlap by OVERLAP_CHARS worth (start of chunk n+1 < end of chunk n).
        if chunks.len() >= 2 {
            assert!(chunks[1].byte_start < chunks[0].byte_end);
        }
    }

    #[test]
    fn short_text_is_one_chunk_empty_text_is_none() {
        let one = chunk_text("just a little note");
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].ordinal, 0);
        assert_eq!(one[0].byte_start, 0);
        assert_eq!(one[0].byte_end, "just a little note".len());
        assert!(chunk_text("").is_empty());
        assert!(chunk_text("   \n\t  ").is_empty());
    }

    #[test]
    fn chunking_never_splits_a_utf8_boundary() {
        // Multibyte chars (é = 2 bytes, 😀 = 4). Windowing on chars must keep every slice valid UTF-8.
        let text = "café 😀 ".repeat(500);
        let chunks = chunk_text(&text);
        for c in &chunks {
            // Indexing text[bs..be] already panics on a non-boundary; assert the slice equals stored text.
            assert_eq!(&text[c.byte_start..c.byte_end], c.text);
        }
    }

    #[test]
    fn text_hash_changes_with_content() {
        assert_ne!(fnv1a_hex("alpha"), fnv1a_hex("beta"));
        assert_eq!(fnv1a_hex("alpha"), fnv1a_hex("alpha"));
    }
}
