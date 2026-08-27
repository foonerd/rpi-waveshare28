//! Minimal HTTP/1.1 GET, over TCP or TLS.
//!
//! Deliberately not an HTTP client crate. `ureq` and `reqwest` both pull
//! `url` -> `idna` -> the `icu_*` normalisation stack, which is a Unicode
//! library carried solely to parse a URL, and it raises the toolchain floor
//! as a side effect. rustls sits under this instead, so there is exactly one
//! HTTP implementation and TLS is the only thing added.
//!
//! Scope is exactly what is needed and no more: no redirects, no keep-alive,
//! no chunked decoding.
//!
//! The body is delimited by `Content-Length` where the server sends one, and
//! only falls back to reading until EOF where it does not. An earlier version
//! sent `Connection: close` and always read to EOF, which made every request
//! depend on the server actually closing the socket. Volumio does not always
//! close promptly, and the result was intermittent EAGAIN as the read timeout
//! expired on a response that had already arrived in full.
//!
//! Trust anchors come from `webpki-roots`, compiled in. A statically linked
//! binary cannot rely on a system certificate store being present, and
//! Volumio images do not necessarily ship `ca-certificates`.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// Errors a request can produce.
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    /// The URL was not `http://host[:port]/path` or the `https` equivalent.
    #[error("malformed url: {0}")]
    Url(String),
    /// The host did not resolve.
    #[error("host did not resolve: {0}")]
    Resolve(String),
    /// Socket-level failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// TLS handshake or configuration failure.
    #[error("tls: {0}")]
    Tls(String),
    /// Response was not valid HTTP, or the status line was missing.
    #[error("malformed response")]
    Response,
    /// Server returned a non-2xx status.
    #[error("http status {0}")]
    Status(u16),
}

/// A parsed URL.
struct Target {
    /// Host without port, needed separately for TLS server name checking.
    host: String,
    /// Host and port, as sent in the Host header and used for resolution.
    authority: String,
    path: String,
    tls: bool,
}

fn parse(url: &str) -> Result<Target, HttpError> {
    let (rest, tls, default_port) = if let Some(r) = url.strip_prefix("https://") {
        (r, true, 443)
    } else if let Some(r) = url.strip_prefix("http://") {
        (r, false, 80)
    } else {
        return Err(HttpError::Url(url.into()));
    };

    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };

    if authority.is_empty() {
        return Err(HttpError::Url(url.into()));
    }

    let (host, authority) = match authority.split_once(':') {
        Some((h, _)) => (h.to_string(), authority.to_string()),
        None => (authority.to_string(), format!("{authority}:{default_port}")),
    };

    Ok(Target {
        host,
        authority,
        path: path.to_string(),
        tls,
    })
}

/// Shared client configuration.
///
/// Built once: assembling the root store parses every trust anchor, which is
/// wasted work on a 3A+ if it happens per request.
fn tls_config() -> Result<Arc<rustls::ClientConfig>, HttpError> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();

    if let Some(c) = CONFIG.get() {
        return Ok(c.clone());
    }

    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let cfg = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();

    Ok(CONFIG.get_or_init(|| Arc::new(cfg)).clone())
}

/// Perform a GET and return the response body as a string.
pub fn get(url: &str, timeout: Duration) -> Result<String, HttpError> {
    let body = get_bytes(url, timeout)?;
    String::from_utf8(body).map_err(|_| HttpError::Response)
}

/// Perform a GET and return the raw response body.
///
/// `timeout` applies separately to connect, read and write, so a stalled
/// server cannot hold the caller for longer than roughly three times it.
pub fn get_bytes(url: &str, timeout: Duration) -> Result<Vec<u8>, HttpError> {
    let target = parse(url)?;

    let addr = target
        .authority
        .to_socket_addrs()
        .map_err(|_| HttpError::Resolve(target.authority.clone()))?
        .next()
        .ok_or_else(|| HttpError::Resolve(target.authority.clone()))?;

    let mut sock = TcpStream::connect_timeout(&addr, timeout)?;
    sock.set_read_timeout(Some(timeout))?;
    sock.set_write_timeout(Some(timeout))?;
    sock.set_nodelay(true)?;

    if target.tls {
        let name = rustls_pki_types::ServerName::try_from(target.host.clone())
            .map_err(|e| HttpError::Tls(format!("server name {}: {e}", target.host)))?;
        let conn = rustls::ClientConnection::new(tls_config()?, name)
            .map_err(|e| HttpError::Tls(e.to_string()))?;
        let mut tls = rustls::StreamOwned::new(conn, sock);
        exchange(&mut tls, &target)
    } else {
        exchange(&mut sock, &target)
    }
}

/// Send the request and read the response, over whichever stream.
///
/// Split out so the plain and TLS paths share one implementation. The
/// framing rules are identical; only the transport differs.
fn exchange<S: Read + Write>(stream: &mut S, target: &Target) -> Result<Vec<u8>, HttpError> {
    // Host header carries the authority as given, including any explicit port.
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: waveshare28-panel\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        target.path, target.authority
    );
    stream.write_all(req.as_bytes())?;
    stream.flush()?;

    // Read until the header block is complete, then use Content-Length to
    // decide how much body to expect. Reading blindly to EOF would block until
    // the read timeout whenever the server keeps the socket open, regardless
    // of the response having already arrived.
    let mut raw = Vec::with_capacity(4096);
    let mut chunk = [0u8; 2048];

    let header_end = loop {
        if let Some(i) = find_header_end(&raw) {
            break i;
        }
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(HttpError::Response);
        }
        raw.extend_from_slice(&chunk[..n]);
    };

    let head = std::str::from_utf8(&raw[..header_end]).map_err(|_| HttpError::Response)?;
    let code = status_code(head)?;
    let want = content_length(head);

    // +4 steps over the blank line terminator.
    let body_start = header_end + 4;

    match want {
        Some(len) => {
            while raw.len() < body_start + len {
                let n = stream.read(&mut chunk)?;
                if n == 0 {
                    break;
                }
                raw.extend_from_slice(&chunk[..n]);
            }
        }
        // No Content-Length leaves EOF as the only delimiter available.
        None => {
            // A TLS stream reports close_notify as an error rather than EOF on
            // some servers; a truncated read here is not worth failing over
            // when the body is already complete.
            let _ = stream.read_to_end(&mut raw);
        }
    }

    if !(200..300).contains(&code) {
        return Err(HttpError::Status(code));
    }

    let end = match want {
        Some(len) => (body_start + len).min(raw.len()),
        None => raw.len(),
    };

    Ok(raw[body_start..end].to_vec())
}

/// Parse the status code out of a header block.
fn status_code(head: &str) -> Result<u16, HttpError> {
    head.lines()
        .next()
        .ok_or(HttpError::Response)?
        .split_whitespace()
        .nth(1)
        .ok_or(HttpError::Response)?
        .parse()
        .map_err(|_| HttpError::Response)
}

/// Content-Length from a header block, if present and parseable.
fn content_length(head: &str) -> Option<usize> {
    head.lines()
        .skip(1)
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim())
        })
        .and_then(|v| v.parse().ok())
}

/// Index of the CRLFCRLF that ends the header block.
fn find_header_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|w| w == b"\r\n\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_host_port_path() {
        let t = parse("http://localhost:3000/api/v1/getState").unwrap();
        assert_eq!(t.host, "localhost");
        assert_eq!(t.authority, "localhost:3000");
        assert_eq!(t.path, "/api/v1/getState");
        assert!(!t.tls);
    }

    #[test]
    fn defaults_port_and_path() {
        let t = parse("http://example").unwrap();
        assert_eq!(t.authority, "example:80");
        assert_eq!(t.path, "/");
    }

    #[test]
    fn https_defaults_to_443() {
        let t = parse("https://cdn.example/logo.png").unwrap();
        assert_eq!(t.host, "cdn.example");
        assert_eq!(t.authority, "cdn.example:443");
        assert_eq!(t.path, "/logo.png");
        assert!(t.tls);
    }

    #[test]
    fn rejects_unknown_scheme() {
        assert!(parse("ftp://localhost/x").is_err());
        assert!(parse("localhost/x").is_err());
    }

    #[test]
    fn reads_status_line() {
        assert_eq!(status_code("HTTP/1.1 200 OK\r\nX: y").unwrap(), 200);
        assert_eq!(
            status_code("HTTP/1.1 500 Internal Server Error").unwrap(),
            500
        );
    }

    #[test]
    fn reads_content_length_case_insensitively() {
        let head = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\ncontent-length: 42";
        assert_eq!(content_length(head), Some(42));
    }

    #[test]
    fn absent_content_length_is_none() {
        assert_eq!(content_length("HTTP/1.1 200 OK\r\nX: y"), None);
    }

    #[test]
    fn finds_header_terminator() {
        assert_eq!(find_header_end(b"AB\r\n\r\nbody"), Some(2));
        assert_eq!(find_header_end(b"no terminator"), None);
    }
}
