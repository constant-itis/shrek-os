//! swamp-embed-proxy — the OFF-IMAGE embedding-provider proxy (docs/phase6-swamp-slice4-semantic-
//! provider.md §1.2/§1.3). It is the ONLY party that reaches the LAN embedding provider; swampd stays
//! network-free and speaks a tiny plaintext binary framing to this proxy over a LOCAL unix socket.
//!
//! ```text
//!  swampd  --unix socket, binary framing-->  swamp-embed-proxy  --plaintext HTTP-->  evo-x2:8102
//!  (no network, no http, no model)           (this process: HTTP + JSON)            (EmbeddingGemma)
//! ```
//!
//! Wire framing (MUST match crates/swampd/src/embed.rs exactly):
//!   REQUEST  (proxy reads):  u32 n; for each: u32 len, len UTF-8 bytes.
//!   RESPONSE (proxy writes): u8 status (1 ok / 0 err); u32 dim; for each of n: dim × f32 (LE).
//! On ANY provider/JSON failure the proxy writes a status-0 frame — swampd then degrades that work to
//! the lexical FTS floor (§1.2 T7). The proxy NEVER holds authority: it only turns text into numbers.
//!
//! Env (broker-side operational config; NEVER caller-influenced):
//!   SWAMP_EMBED_SOCKET    unix socket to listen on          (default /run/swamp-embed.sock)
//!   SWAMP_EMBED_UPSTREAM  provider host:port                (default 192.168.1.152:8102)
//!   SWAMP_EMBED_MODEL     model id sent to /v1/embeddings   (default embeddinggemma-300m)
//!   SWAMP_EMBED_PATH      request path                      (default /v1/embeddings)

use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};

use tinyjson::JsonValue;

const DEFAULT_SOCKET: &str = "/run/swamp-embed.sock";
const DEFAULT_UPSTREAM: &str = "192.168.1.152:8102";
const DEFAULT_MODEL: &str = "embeddinggemma-300m";
const DEFAULT_PATH: &str = "/v1/embeddings";

/// Defensive bounds against a malformed sender (swampd is trusted, but frame parsing fails closed).
const MAX_CHUNKS: usize = 4096;
const MAX_CHUNK_BYTES: usize = 4 * 1024 * 1024;
const MAX_HTTP_BODY: usize = 64 * 1024 * 1024;

fn main() {
    std::process::exit(real_main());
}

fn real_main() -> i32 {
    let socket = env_or("SWAMP_EMBED_SOCKET", DEFAULT_SOCKET);
    let upstream = env_or("SWAMP_EMBED_UPSTREAM", DEFAULT_UPSTREAM);
    let model = env_or("SWAMP_EMBED_MODEL", DEFAULT_MODEL);
    let path = env_or("SWAMP_EMBED_PATH", DEFAULT_PATH);
    let Some((up_host, up_port)) = split_hostport(&upstream) else {
        eprintln!("SWAMP-EMBED-PROXY-ERROR bad SWAMP_EMBED_UPSTREAM {upstream:?}");
        return 2;
    };

    let _ = std::fs::remove_file(&socket);
    let listener = match UnixListener::bind(&socket) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("SWAMP-EMBED-PROXY-ERROR bind {socket}: {e}");
            return 2;
        }
    };
    // swampd (user `swamp`) connects; local trust boundary. 0666 like swampd's own query socket.
    let _ = std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o666));
    println!("SWAMP-EMBED-PROXY-LISTEN {socket} upstream={up_host}:{up_port} model={model} path={path}");

    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                if let Err(e) = handle(stream, &up_host, up_port, &model, &path) {
                    eprintln!("SWAMP-EMBED-PROXY conn error: {e}");
                }
            }
            Err(e) => eprintln!("SWAMP-EMBED-PROXY accept error: {e}"),
        }
    }
    0
}

/// One request: decode the chunk batch, embed it via the provider, write the response frame. Any
/// provider/JSON error is turned into a status-0 frame (swampd degrades to FTS) — never a hard failure.
fn handle(mut stream: UnixStream, host: &str, port: u16, model: &str, path: &str) -> std::io::Result<()> {
    let chunks = match decode_request(&mut stream) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("SWAMP-EMBED-PROXY bad request frame: {e}");
            stream.write_all(&encode_err())?;
            return Ok(());
        }
    };
    if chunks.is_empty() {
        stream.write_all(&encode_ok(&[], 0))?;
        return Ok(());
    }
    let refs: Vec<&str> = chunks.iter().map(|s| s.as_str()).collect();
    match embed_batch(host, port, path, model, &refs) {
        Ok(vecs) if vecs.len() == chunks.len() && vecs.iter().all(|v| v.len() == vecs[0].len()) => {
            let dim = vecs[0].len() as u32;
            stream.write_all(&encode_ok(&vecs, dim))?;
        }
        Ok(_) => {
            eprintln!("SWAMP-EMBED-PROXY provider returned wrong count/ragged dims — failing closed");
            stream.write_all(&encode_err())?;
        }
        Err(e) => {
            eprintln!("SWAMP-EMBED-PROXY provider error: {e} — failing closed (swampd → FTS)");
            stream.write_all(&encode_err())?;
        }
    }
    Ok(())
}

// ─── framing (matches swampd/src/embed.rs) ──────────────────────────────────────────────────────────

fn decode_request(stream: &mut impl Read) -> Result<Vec<String>, String> {
    let n = read_u32(stream)? as usize;
    if n > MAX_CHUNKS {
        return Err(format!("chunk count {n} exceeds cap {MAX_CHUNKS}"));
    }
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let len = read_u32(stream)? as usize;
        if len > MAX_CHUNK_BYTES {
            return Err(format!("chunk len {len} exceeds cap {MAX_CHUNK_BYTES}"));
        }
        let mut buf = vec![0u8; len];
        stream.read_exact(&mut buf).map_err(|e| format!("short chunk body: {e}"))?;
        out.push(String::from_utf8_lossy(&buf).into_owned());
    }
    Ok(out)
}

fn read_u32(stream: &mut impl Read) -> Result<u32, String> {
    let mut b = [0u8; 4];
    stream.read_exact(&mut b).map_err(|e| format!("short u32: {e}"))?;
    Ok(u32::from_le_bytes(b))
}

fn encode_ok(vecs: &[Vec<f32>], dim: u32) -> Vec<u8> {
    let mut b = Vec::with_capacity(5 + vecs.len() * dim as usize * 4);
    b.push(1u8);
    b.extend_from_slice(&dim.to_le_bytes());
    for v in vecs {
        for x in v {
            b.extend_from_slice(&x.to_le_bytes());
        }
    }
    b
}

/// A status-0 frame: swampd's `decode_response` reads status 0 → `EmbedError::Unavailable` → FTS floor.
fn encode_err() -> Vec<u8> {
    let mut b = vec![0u8];
    b.extend_from_slice(&0u32.to_le_bytes());
    b
}

// ─── provider call (OpenAI /v1/embeddings over plaintext HTTP) ──────────────────────────────────────

fn embed_batch(host: &str, port: u16, path: &str, model: &str, chunks: &[&str]) -> Result<Vec<Vec<f32>>, String> {
    let body = build_request_json(model, chunks);
    let resp = http_post(host, port, path, body.as_bytes())?;
    parse_embeddings(&resp, chunks.len())
}

/// `{"model":"<model>","input":["chunk", ...]}` — built via tinyjson so chunk text is JSON-escaped.
fn build_request_json(model: &str, chunks: &[&str]) -> String {
    let input: Vec<JsonValue> = chunks.iter().map(|c| JsonValue::String((*c).to_string())).collect();
    let mut obj = std::collections::HashMap::new();
    obj.insert("model".to_string(), JsonValue::String(model.to_string()));
    obj.insert("input".to_string(), JsonValue::Array(input));
    JsonValue::Object(obj).stringify().unwrap_or_else(|_| "{}".to_string())
}

/// Parse `{"data":[{"embedding":[f,...]}, ...]}` (OpenAI shape) into `expect` f32 vectors, in order.
fn parse_embeddings(body: &[u8], expect: usize) -> Result<Vec<Vec<f32>>, String> {
    let text = std::str::from_utf8(body).map_err(|_| "non-UTF-8 response body".to_string())?;
    let json: JsonValue = text.parse().map_err(|e| format!("bad JSON: {e}"))?;
    let obj = json.get::<std::collections::HashMap<String, JsonValue>>().ok_or("response not an object")?;
    let data = obj
        .get("data")
        .and_then(|d| d.get::<Vec<JsonValue>>())
        .ok_or("response missing `data` array")?;
    let mut out = Vec::with_capacity(data.len());
    for item in data {
        let emb = item
            .get::<std::collections::HashMap<String, JsonValue>>()
            .and_then(|o| o.get("embedding"))
            .and_then(|e| e.get::<Vec<JsonValue>>())
            .ok_or("data item missing `embedding` array")?;
        let mut v = Vec::with_capacity(emb.len());
        for x in emb {
            let f = x.get::<f64>().ok_or("embedding element not a number")?;
            v.push(*f as f32);
        }
        out.push(v);
    }
    if out.len() != expect {
        return Err(format!("provider returned {} vectors, expected {expect}", out.len()));
    }
    Ok(out)
}

/// Minimal plaintext HTTP/1.1 POST. Sends `Connection: close` so the body is read to EOF (no chunked/
/// Content-Length parsing). Returns the response BODY bytes; a non-2xx status is an error.
fn http_post(host: &str, port: u16, path: &str, body: &[u8]) -> Result<Vec<u8>, String> {
    let mut stream = TcpStream::connect((host, port)).map_err(|e| format!("connect {host}:{port}: {e}"))?;
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).map_err(|e| format!("write head: {e}"))?;
    stream.write_all(body).map_err(|e| format!("write body: {e}"))?;
    stream.flush().ok();
    let mut raw = Vec::new();
    stream.take(MAX_HTTP_BODY as u64).read_to_end(&mut raw).map_err(|e| format!("read: {e}"))?;
    let code = status_code(&raw).ok_or("no HTTP status line")?;
    if !(200..300).contains(&code) {
        return Err(format!("provider HTTP {code}"));
    }
    let sep = find_subslice(&raw, b"\r\n\r\n").ok_or("no header/body separator")?;
    Ok(raw[sep + 4..].to_vec())
}

// ─── small helpers (mirror model-proxy) ─────────────────────────────────────────────────────────────

fn status_code(resp: &[u8]) -> Option<u16> {
    let line_end = find_subslice(resp, b"\r\n").unwrap_or(resp.len());
    let line = std::str::from_utf8(&resp[..line_end]).ok()?;
    line.split(' ').nth(1)?.parse().ok()
}

fn split_hostport(s: &str) -> Option<(String, u16)> {
    let (h, p) = s.rsplit_once(':')?;
    if h.is_empty() {
        return None;
    }
    Some((h.to_string(), p.parse().ok()?))
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_request(chunks: &[&str]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
        for c in chunks {
            b.extend_from_slice(&(c.len() as u32).to_le_bytes());
            b.extend_from_slice(c.as_bytes());
        }
        b
    }

    #[test]
    fn decode_request_reads_the_swampd_frame() {
        let raw = frame_request(&["hello", "world!!"]);
        let mut cur = std::io::Cursor::new(raw);
        let got = decode_request(&mut cur).unwrap();
        assert_eq!(got, vec!["hello".to_string(), "world!!".to_string()]);
    }

    #[test]
    fn decode_request_rejects_absurd_counts() {
        let mut b = (MAX_CHUNKS as u32 + 1).to_le_bytes().to_vec();
        b.extend_from_slice(&[0, 0, 0, 0]);
        assert!(decode_request(&mut std::io::Cursor::new(b)).is_err());
    }

    #[test]
    fn encode_ok_matches_swampd_expectations() {
        let vecs = vec![vec![1.0f32, 2.0], vec![3.0, 4.0]];
        let f = encode_ok(&vecs, 2);
        assert_eq!(f[0], 1); // status ok
        assert_eq!(&f[1..5], &2u32.to_le_bytes()); // dim
        assert_eq!(f.len(), 5 + 2 * 2 * 4);
        // The err frame is status 0 + dim 0.
        assert_eq!(encode_err(), vec![0, 0, 0, 0, 0]);
    }

    #[test]
    fn build_request_json_escapes_and_shapes() {
        let j = build_request_json("m", &["a \"quote\"", "b"]);
        let parsed: JsonValue = j.parse().unwrap();
        let obj = parsed.get::<std::collections::HashMap<String, JsonValue>>().unwrap();
        assert_eq!(obj.get("model").unwrap().get::<String>().unwrap(), "m");
        let input = obj.get("input").unwrap().get::<Vec<JsonValue>>().unwrap();
        assert_eq!(input.len(), 2);
        assert_eq!(input[0].get::<String>().unwrap(), "a \"quote\"");
    }

    #[test]
    fn parse_embeddings_reads_openai_shape() {
        let body = br#"{"data":[{"embedding":[0.1,0.2,0.3]},{"embedding":[0.4,0.5,0.6]}]}"#;
        let vecs = parse_embeddings(body, 2).unwrap();
        assert_eq!(vecs.len(), 2);
        assert_eq!(vecs[0].len(), 3);
        assert!((vecs[0][0] - 0.1).abs() < 1e-6);
        // Wrong count → error (fail closed).
        assert!(parse_embeddings(body, 3).is_err());
        // Malformed → error.
        assert!(parse_embeddings(b"not json", 1).is_err());
        assert!(parse_embeddings(br#"{"nope":1}"#, 1).is_err());
    }

    #[test]
    fn hostport_and_status_helpers() {
        assert_eq!(split_hostport("192.168.1.152:8102"), Some(("192.168.1.152".to_string(), 8102)));
        assert_eq!(split_hostport("noport"), None);
        assert_eq!(status_code(b"HTTP/1.1 200 OK\r\n\r\n"), Some(200));
        assert_eq!(status_code(b"HTTP/1.1 503 x\r\n"), Some(503));
    }
}
