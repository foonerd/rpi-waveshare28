//! Minimal HTTP/1.1 GET over a plain TCP socket.
//!
//! Deliberately not a crate. The only request this binary makes is against
//! `http://localhost:3000`, so a general client buys nothing: `ureq` and
//! `reqwest` both pull `url` -> `idna` -> the `icu_*` normalisation stack, and
//! carrying a Unicode library to parse a loopback URL is not a trade worth
//! making on a 512 MB board.
//!
//! Scope is exactly what is needed and no more: no TLS, no redirects, no
//! keep-alive, no chunked decoding. `Connection: close` is sent so the body is
//! delimited by EOF, which sidesteps chunked transfer entirely.

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
///
/// `timeout` applies separately to connect, read and write, so a stalled
/// server cannot hold the caller for longer than roughly three times it.
pub fn get(url: &str, timeout: Duration) -> Result<String, HttpError> {
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

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;

    split_response(&raw)
}

/// Split a raw response into status and body, and check the status.
fn split_response(raw: &[u8]) -> Result<String, HttpError> {
    let sep = find_header_end(raw).ok_or(HttpError::Response)?;
    let head = std::str::from_utf8(&raw[..sep]).map_err(|_| HttpError::Response)?;

    let status_line = head.lines().next().ok_or(HttpError::Response)?;
    let code: u16 = status_line
        .split_whitespace()
        .nth(1)
        .ok_or(HttpError::Response)?
        .parse()
        .map_err(|_| HttpError::Response)?;

    if !(200..300).contains(&code) {
        return Err(HttpError::Status(code));
    }

    // +4 to step over the blank line terminator.
    let body = &raw[sep + 4..];
    String::from_utf8(body.to_vec()).map_err(|_| HttpError::Response)
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
    fn splits_body() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{\"a\":1}";
        assert_eq!(split_response(raw).unwrap(), "{\"a\":1}");
    }

    #[test]
    fn rejects_error_status() {
        let raw = b"HTTP/1.1 500 Internal Server Error\r\n\r\nboom";
        assert!(matches!(split_response(raw), Err(HttpError::Status(500))));
    }
}
