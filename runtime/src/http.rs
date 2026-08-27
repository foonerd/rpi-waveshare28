//! Minimal HTTP/1.1 GET over a plain TCP socket.
//!
//! Deliberately not a crate. The only request this binary makes is against
//! `http://localhost:3000`, so a general client buys nothing: `ureq` and
//! `reqwest` both pull `url` -> `idna` -> the `icu_*` normalisation stack, and
//! carrying a Unicode library to parse a loopback URL is not a trade worth
//! making on a 512 MB board.
//!
//! Scope is exactly what is needed and no more: no TLS, no redirects, no
//! keep-alive, no chunked decoding.
//!
//! The body is delimited by `Content-Length` where the server sends one, and
//! only falls back to reading until EOF where it does not. An earlier version
//! sent `Connection: close` and always read to EOF, which made every request
//! depend on the server actually closing the socket. Volumio does not always
//! close promptly, and the result was intermittent EAGAIN as the read timeout
//! expired on a response that had already arrived in full.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// Errors a request can produce.
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    /// The URL was not a plain `http://host[:port]/path`.
    #[error("malformed url: {0}")]
    Url(String),
    /// The host did not resolve.
    #[error("host did not resolve: {0}")]
    Resolve(String),
    /// Socket-level failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Response was not valid HTTP, or the status line was missing.
    #[error("malformed response")]
    Response,
    /// Server returned a non-2xx status.
    #[error("http status {0}")]
    Status(u16),
}

/// A parsed `http://host:port/path`.
struct Target {
    authority: String,
    path: String,
}

fn parse(url: &str) -> Result<Target, HttpError> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| HttpError::Url(url.into()))?;

    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };

    if authority.is_empty() {
        return Err(HttpError::Url(url.into()));
    }

    // Default the port so `to_socket_addrs` has something to work with.
    let authority = if authority.contains(':') {
        authority.to_string()
    } else {
        format!("{authority}:80")
    };

    Ok(Target {
        authority,
        path: path.to_string(),
    })
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

    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    stream.set_nodelay(true)?;

    // Host header carries the authority as given, including any explicit port.
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: waveshare28-panel\r\nAccept: application/json\r\nConnection: close\r\n\r\n",
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
            stream.read_to_end(&mut raw)?;
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
        assert_eq!(t.authority, "localhost:3000");
        assert_eq!(t.path, "/api/v1/getState");
    }

    #[test]
    fn defaults_port_and_path() {
        let t = parse("http://example").unwrap();
        assert_eq!(t.authority, "example:80");
        assert_eq!(t.path, "/");
    }

    #[test]
    fn rejects_non_http() {
        assert!(parse("https://localhost/x").is_err());
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
