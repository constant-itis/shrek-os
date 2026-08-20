//! model-proxy — the broker-side authenticated egress proxy for the coder's hosted-model (Anthropic)
//! provider (docs/phase6-slice3-provider-abstraction.md, security-model.md §7).
//!
//! WHY IT EXISTS. A hosted model needs an API key — a secret. Putting that key inside the T2 sandbox
//! (which has untrusted-read + egress) would complete the lethal trifecta. So the key lives HERE,
//! broker-side, outside the box. The sandboxed coder speaks PLAINTEXT to this proxy over the sealed
//! one-destination `model-anthropic` egress (`shrek-model-proxy`); this proxy injects the auth header
//! and terminates TLS to Anthropic. The box never holds the secret and never reaches Anthropic
//! directly. Authority is unchanged: this adds no capability to the sandbox — it is the sealed egress
//! DESTINATION, nothing more.
//!
//! WHAT IT IS NOT. Not a control plane, not sealed into the appliance image, not a translator (the
//! coder builds the messages-API wire; the proxy forwards the body verbatim and only adds the key +
//! version headers + TLS). It reads the key from a BROKER-SIDE file — never an env var the sandbox
//! could inherit, never a value in the repo.
//!
//! Config (all env; broker-side):
//!   SHREK_PROXY_LISTEN        plaintext listen addr for the box    (default 127.0.0.1:8200)
//!   SHREK_PROXY_UPSTREAM      host:port to TLS-forward to           (default api.anthropic.com:443)
//!   SHREK_ANTHROPIC_KEY_FILE  path to the API key (REQUIRED)        — broker-side only
//!   SHREK_ANTHROPIC_VERSION   anthropic-version header              (default 2023-06-01)
//!   SHREK_PROXY_EXTRA_CA      extra PEM CA to trust for upstream    (oracle self-signed cert; optional)

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

const DEFAULT_LISTEN: &str = "127.0.0.1:8200";
const DEFAULT_UPSTREAM: &str = "api.anthropic.com:443";
const DEFAULT_VERSION: &str = "2023-06-01";
/// Guard: refuse to read an absurdly large request/response so a broken peer cannot OOM the broker.
const MAX_BODY: usize = 16 * 1024 * 1024;

fn main() {
    std::process::exit(real_main());
}

fn real_main() -> i32 {
    let listen = env_or("SHREK_PROXY_LISTEN", DEFAULT_LISTEN);
    let upstream = env_or("SHREK_PROXY_UPSTREAM", DEFAULT_UPSTREAM);
    let version = env_or("SHREK_ANTHROPIC_VERSION", DEFAULT_VERSION);
    let key_file = match std::env::var("SHREK_ANTHROPIC_KEY_FILE") {
        Ok(p) if !p.is_empty() => p,
        _ => {
            eprintln!("MODEL-PROXY-ERROR SHREK_ANTHROPIC_KEY_FILE is required (broker-side key path)");
            return 2;
        }
    };
    let (up_host, up_port) = match split_hostport(&upstream) {
        Some(hp) => hp,
        None => { eprintln!("MODEL-PROXY-ERROR bad SHREK_PROXY_UPSTREAM {upstream:?}"); return 2; }
    };

    // Build the TLS client config ONCE (webpki-roots + optional oracle CA), shared across connections.
    let tls_config = match build_tls_config() {
        Ok(c) => Arc::new(c),
        Err(e) => { eprintln!("MODEL-PROXY-ERROR tls config: {e}"); return 2; }
    };

    let listener = match TcpListener::bind(&listen) {
        Ok(l) => l,
        Err(e) => { eprintln!("MODEL-PROXY-ERROR bind {listen}: {e}"); return 2; }
    };
    println!("MODEL-PROXY-LISTEN {listen} upstream={up_host}:{up_port} key_file={key_file}");

    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                let cfg = Arc::clone(&tls_config);
                let (h, ver, kf) = (up_host.clone(), version.clone(), key_file.clone());
                let up_host2 = up_host.clone();
                std::thread::spawn(move || {
                    if let Err(e) = handle(stream, cfg, &h, up_port, &up_host2, &ver, &kf) {
                        eprintln!("MODEL-PROXY-ERROR conn: {e}");
                    }
                });
            }
            Err(e) => eprintln!("MODEL-PROXY-ERROR accept: {e}"),
        }
    }
    0
}

/// Handle one box→proxy→Anthropic exchange. Reads the box's plaintext request, injects the key +
/// version headers, TLS-forwards the body VERBATIM to the upstream, and writes the upstream response
/// back to the box plaintext. The key is read fresh from the broker-side file per request and never
/// logged; on any auth-read failure we fail CLOSED (502 to the box) rather than forward unauthenticated.
fn handle(
    mut box_stream: TcpStream,
    tls_config: Arc<ClientConfig>,
    sni_host: &str,
    up_port: u16,
    connect_host: &str,
    version: &str,
    key_file: &str,
) -> std::io::Result<()> {
    let (method, path, body) = match read_http_request(&mut box_stream) {
        Ok(v) => v,
        Err(e) => {
            write_plain(&mut box_stream, 400, "bad request from box")?;
            return Err(e);
        }
    };

    // The secret lives ONLY here, read from the broker-side file. Never an env the box can inherit.
    let key = match std::fs::read_to_string(key_file) {
        Ok(k) => k.trim().to_string(),
        Err(e) => {
            eprintln!("MODEL-PROXY-ERROR key file {key_file}: {e}");
            write_plain(&mut box_stream, 502, "proxy has no upstream credential")?;
            return Ok(());
        }
    };
    println!("MODEL-PROXY-FWD {method} {path} -> {sni_host}:{up_port} body={}", body.len());
    // Anchored marker proving the injection point exists — the KEY VALUE is never printed.
    println!("MODEL-PROXY-INJECTED-AUTH x-api-key(len={}) anthropic-version={version}", key.len());

    // Compose the upstream request: box's method+path, our Host + auth headers, the body verbatim.
    let mut req = Vec::new();
    req.extend_from_slice(
        format!(
            "{method} {path} HTTP/1.1\r\n\
             Host: {sni_host}\r\n\
             x-api-key: {key}\r\n\
             anthropic-version: {version}\r\n\
             content-type: application/json\r\n\
             content-length: {}\r\n\
             connection: close\r\n\r\n",
            body.len()
        )
        .as_bytes(),
    );
    req.extend_from_slice(&body);

    // TLS to Anthropic (or the oracle's canned upstream). SNI = the upstream host; the connect target
    // resolves through the system resolver (the oracle pins it via /etc/hosts to the canned responder).
    let response = match tls_forward(tls_config, sni_host, connect_host, up_port, &req) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("MODEL-PROXY-ERROR upstream TLS: {e}");
            write_plain(&mut box_stream, 502, "upstream TLS failed")?;
            return Ok(());
        }
    };
    let status = status_code(&response).unwrap_or(0);
    println!("MODEL-PROXY-UPSTREAM-STATUS {status} bytes={}", response.len());

    // Return the upstream response VERBATIM to the box (plaintext).
    box_stream.write_all(&response)?;
    box_stream.flush().ok();
    Ok(())
}

/// Open a TLS connection to `connect_host:port` with SNI `sni_host`, send `req`, return the raw
/// response bytes (headers + body). A server that closes without `close_notify` is treated as a clean
/// EOF (common), not an error.
fn tls_forward(
    tls_config: Arc<ClientConfig>,
    sni_host: &str,
    connect_host: &str,
    port: u16,
    req: &[u8],
) -> Result<Vec<u8>, String> {
    let server_name = ServerName::try_from(sni_host.to_string())
        .map_err(|_| format!("bad SNI host {sni_host:?}"))?;
    let conn = ClientConnection::new(tls_config, server_name).map_err(|e| e.to_string())?;
    let sock = TcpStream::connect((connect_host, port))
        .map_err(|e| format!("connect {connect_host}:{port}: {e}"))?;
    let mut tls = StreamOwned::new(conn, sock);

    tls.write_all(req).map_err(|e| format!("tls write: {e}"))?;
    tls.flush().ok();

    let mut resp = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        match tls.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                resp.extend_from_slice(&buf[..n]);
                if resp.len() > MAX_BODY {
                    return Err("upstream response exceeds cap".into());
                }
            }
            // A missing close_notify surfaces as UnexpectedEof — the response is already complete.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(format!("tls read: {e}")),
        }
    }
    if resp.is_empty() {
        return Err("empty upstream response".into());
    }
    Ok(resp)
}

/// rustls client config: Mozilla webpki-roots + (optional) an extra PEM CA for the oracle's
/// self-signed canned upstream. Explicit `ring` provider so the crypto backend is unambiguous.
fn build_tls_config() -> Result<ClientConfig, String> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    if let Ok(ca_path) = std::env::var("SHREK_PROXY_EXTRA_CA") {
        if !ca_path.is_empty() {
            let pem = std::fs::read(&ca_path).map_err(|e| format!("extra CA {ca_path}: {e}"))?;
            let mut rd = std::io::BufReader::new(&pem[..]);
            let mut added = 0;
            for cert in rustls_pemfile::certs(&mut rd) {
                let cert: CertificateDer = cert.map_err(|e| format!("extra CA parse: {e}"))?;
                roots.add(cert).map_err(|e| format!("extra CA add: {e}"))?;
                added += 1;
            }
            eprintln!("MODEL-PROXY-INFO trusting {added} extra CA cert(s) from {ca_path}");
        }
    }

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(config)
}

// ---- minimal HTTP/1.1 request reader (box→proxy is a controlled, single-request path) -------------

/// Read one HTTP request from the box: the request line (method, path), then headers, then exactly
/// `Content-Length` body bytes. Fails closed on a malformed head or an oversized/short body.
fn read_http_request(stream: &mut TcpStream) -> std::io::Result<(String, String, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    // Read until we have the full header block (blank line).
    let head_end = loop {
        if let Some(p) = find_subslice(&buf, b"\r\n\r\n") {
            break p + 4;
        }
        if buf.len() > 64 * 1024 {
            return Err(ioerr("request headers too large"));
        }
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            return Err(ioerr("connection closed before headers complete"));
        }
        buf.extend_from_slice(&tmp[..n]);
    };

    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let req_line = lines.next().unwrap_or("");
    let mut parts = req_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    if method.is_empty() || path.is_empty() {
        return Err(ioerr("malformed request line"));
    }
    let mut content_length = 0usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                content_length = v.trim().parse().map_err(|_| ioerr("bad content-length"))?;
            }
        }
    }
    if content_length > MAX_BODY {
        return Err(ioerr("request body exceeds cap"));
    }

    // Body = whatever already trailed the header block, plus the rest up to content_length.
    let mut body = buf[head_end..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            return Err(ioerr("connection closed before body complete"));
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);
    Ok((method, path, body))
}

fn write_plain(stream: &mut TcpStream, code: u16, msg: &str) -> std::io::Result<()> {
    let body = format!("{{\"error\":{{\"type\":\"proxy_error\",\"message\":{msg:?}}}}}");
    let resp = format!(
        "HTTP/1.1 {code} PROXY\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(resp.as_bytes())?;
    stream.flush().ok();
    Ok(())
}

/// Pull the numeric status out of an HTTP response's first line (`HTTP/1.1 200 OK`).
fn status_code(resp: &[u8]) -> Option<u16> {
    let first = resp.split(|&b| b == b'\n').next()?;
    let s = String::from_utf8_lossy(first);
    s.split_whitespace().nth(1)?.parse().ok()
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
    std::env::var(key).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| default.to_string())
}

fn ioerr(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_hostport_parses() {
        assert_eq!(split_hostport("api.anthropic.com:443"), Some(("api.anthropic.com".into(), 443)));
        assert_eq!(split_hostport("127.0.0.1:8200"), Some(("127.0.0.1".into(), 8200)));
        assert_eq!(split_hostport("noport"), None);
        assert_eq!(split_hostport(":443"), None);
    }

    #[test]
    fn status_code_reads_first_line() {
        assert_eq!(status_code(b"HTTP/1.1 200 OK\r\n\r\nbody"), Some(200));
        assert_eq!(status_code(b"HTTP/1.1 429 Too Many Requests\r\n"), Some(429));
        assert_eq!(status_code(b"garbage"), None);
    }

    #[test]
    fn find_subslice_locates_blank_line() {
        assert_eq!(find_subslice(b"a\r\n\r\nb", b"\r\n\r\n"), Some(1));
        assert_eq!(find_subslice(b"noblank", b"\r\n\r\n"), None);
    }
}
