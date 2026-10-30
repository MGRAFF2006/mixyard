//! Narrow local-console transport. This is not a remote pairing/authentication edge.
//! Kept std-only so the actual native boundary can be exercised without a GUI SDK.
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAX_REQUEST: usize = 1024 * 1024;
const MAX_RESPONSE: usize = 2 * 1024 * 1024;
const MAX_HEADERS: usize = 16 * 1024;
const DEADLINE: Duration = Duration::from_secs(10);

pub struct LocalConsole {
    port: u16,
    active: Arc<AtomicUsize>,
}

pub struct Request {
    pub path: String,
    pub method: String,
    pub token: Option<String>,
    pub body: Option<String>,
}

#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub body: String,
}

pub struct Admission(Arc<AtomicUsize>);
impl Drop for Admission {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl LocalConsole {
    pub fn new(port: u16) -> Result<Self, String> {
        if port == 0 {
            return Err("MIXYARD_WEB_PORT must be a nonzero numeric port".into());
        }
        Ok(Self {
            port,
            active: Arc::new(AtomicUsize::new(0)),
        })
    }

    pub fn acquire(&self) -> Result<Admission, String> {
        self.active
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < 4).then_some(n + 1)
            })
            .map_err(|_| {
                "Four console operations are in progress; refresh before retrying a write"
                    .to_string()
            })?;
        Ok(Admission(self.active.clone()))
    }

    pub fn request(&self, request: Request) -> Result<Reply, String> {
        validate(&request)?;
        let deadline = Instant::now() + DEADLINE;
        // Numeric loopback only: no DNS, proxy environment, redirects or caller-supplied host.
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, self.port));
        let mut stream =
            TcpStream::connect_timeout(&address, Duration::from_secs(2)).map_err(|_| {
                "Local console unavailable. Start mixyard-web separately, then Refresh.".to_string()
            })?;
        stream
            .set_write_timeout(Some(remaining(deadline)?))
            .map_err(io_error)?;
        let body = request.body.unwrap_or_default();
        let token = request
            .token
            .map(|v| format!("X-Mixyard-Token: {v}\r\n"))
            .unwrap_or_default();
        let head = format!("{} {} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nContent-Type: application/json\r\n{token}Content-Length: {}\r\n\r\n", request.method, request.path, body.len());
        stream.write_all(head.as_bytes()).map_err(io_error)?;
        stream
            .set_write_timeout(Some(remaining(deadline)?))
            .map_err(io_error)?;
        stream.write_all(body.as_bytes()).map_err(io_error)?;
        let mut bytes = Vec::new();
        let mut headers_end = None;
        loop {
            stream
                .set_read_timeout(Some(remaining(deadline)?))
                .map_err(io_error)?;
            let mut chunk = [0; 8192];
            let count = stream.read(&mut chunk).map_err(io_error)?;
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..count]);
            if bytes.len() > MAX_HEADERS + MAX_RESPONSE {
                return Err("Local console response exceeds the supported size".into());
            }
            if headers_end.is_none() {
                headers_end = bytes
                    .windows(4)
                    .position(|v| v == b"\r\n\r\n")
                    .map(|n| n + 4);
                if headers_end.unwrap_or(bytes.len()) > MAX_HEADERS {
                    return Err("Local console headers exceed the supported size".into());
                }
            }
        }
        parse_reply(&bytes)
    }
}

fn io_error(_: std::io::Error) -> String {
    "Local console transport failed or timed out. An admitted write may have completed; Refresh and review before another write.".into()
}
fn remaining(deadline: Instant) -> Result<Duration, String> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|v| !v.is_zero())
        .ok_or_else(|| "Local console deadline exceeded; Refresh before another write".into())
}

fn validate(request: &Request) -> Result<(), String> {
    if request.path.starts_with("/control/") {
        return Err("Remote control requires an explicitly paired TLS transport; this desktop connection supports the local console only".into());
    }
    let session = request.path == "/api/session" && request.method == "GET";
    let allowed = matches!(
        request.path.as_str(),
        "/api/status"
            | "/api/validate"
            | "/api/inspect"
            | "/api/profiles/read"
            | "/api/profiles/edit"
            | "/api/dsp/preview"
            | "/api/dsp/validate"
            | "/api/dsp/delay"
            | "/api/automation/validate"
            | "/api/automation/evaluate"
            | "/api/automation/context"
            | "/api/automation/settings/read"
            | "/api/automation/settings/write"
            | "/api/automation/arm"
            | "/api/automation/manual"
            | "/api/automation/status"
            | "/api/automation/disarm"
            | "/api/automation/live"
            | "/api/automation/review"
    );
    if !session && !(allowed && request.method == "POST") {
        return Err("Desktop console route or method is not permitted".into());
    }
    if session {
        if request.token.is_some() || request.body.is_some() {
            return Err("Session discovery cannot carry credentials or a body".into());
        }
    } else if !request
        .token
        .as_ref()
        .is_some_and(|v| v.len() == 64 && v.bytes().all(|c| c.is_ascii_hexdigit()))
    {
        return Err("Local console session token unavailable; Refresh to reconnect".into());
    }
    if request.body.as_ref().is_some_and(|v| v.len() > MAX_REQUEST) {
        return Err("Desktop console request exceeds 1 MiB".into());
    }
    Ok(())
}

fn parse_reply(bytes: &[u8]) -> Result<Reply, String> {
    let invalid =
        || "Invalid local console HTTP response; Refresh before another write".to_string();
    let end = bytes
        .windows(4)
        .position(|v| v == b"\r\n\r\n")
        .ok_or_else(invalid)?;
    if end + 4 > MAX_HEADERS || bytes.len() - end - 4 > MAX_RESPONSE {
        return Err(invalid());
    }
    let header = std::str::from_utf8(&bytes[..end]).map_err(|_| invalid())?;
    let mut lines = header.split("\r\n");
    let mut status_line = lines.next().ok_or_else(invalid)?.split(' ');
    if status_line.next() != Some("HTTP/1.1") {
        return Err(invalid());
    }
    let status: u16 = status_line
        .next()
        .ok_or_else(invalid)?
        .parse()
        .map_err(|_| invalid())?;
    // Redirects are never followed, even to loopback. The service returns JSON bodies.
    if !(200..=599).contains(&status) || (300..400).contains(&status) {
        return Err(invalid());
    }
    let mut length = None;
    let mut json = false;
    for line in lines {
        let (name, value) = line.split_once(':').ok_or_else(invalid)?;
        match name.to_ascii_lowercase().as_str() {
            "content-length" => {
                if length.is_some() {
                    return Err(invalid());
                }
                length = Some(value.trim().parse::<usize>().map_err(|_| invalid())?);
            }
            "content-type" => {
                if json || value.trim() != "application/json" {
                    return Err(invalid());
                }
                json = true;
            }
            "transfer-encoding" => return Err(invalid()),
            _ => {}
        }
    }
    let body = &bytes[end + 4..];
    if !json || length != Some(body.len()) {
        return Err(invalid());
    }
    Ok(Reply {
        status,
        body: String::from_utf8(body.to_vec()).map_err(|_| invalid())?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    fn request(path: &str) -> Request {
        Request {
            path: path.into(),
            method: "POST".into(),
            token: Some("a".repeat(64)),
            body: Some("{}".into()),
        }
    }

    #[test]
    fn rejects_remote_shell_files_encoded_paths_and_header_injection_before_network() {
        let client = LocalConsole::new(1).unwrap();
        for path in [
            "https://example.com/api/status",
            "//example.com/api/status",
            "/api/../secret",
            "/api/status?host=x",
            "/api/%73tatus",
            "/api/status\r\nX: y",
            "/shell",
            "/control/request",
        ] {
            assert!(client.request(request(path)).is_err(), "{path}");
        }
        let mut injected = request("/api/status");
        injected.token = Some("a\r\nAuthorization: Bearer secret".into());
        assert!(client.request(injected).unwrap_err().contains("token"));
    }

    #[test]
    fn forwards_exact_local_request_preserving_precision_and_http_errors_without_replay() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut wire = Vec::new();
            let body = "{\"nanoseconds\":18446744073709551615}";
            while !wire.ends_with(body.as_bytes()) {
                let mut bytes = [0; 1024];
                let n = stream.read(&mut bytes).unwrap();
                assert!(n > 0);
                wire.extend_from_slice(&bytes[..n]);
            }
            let wire = String::from_utf8(wire).unwrap();
            assert!(wire.starts_with("POST /api/dsp/delay HTTP/1.1\r\n"));
            assert!(wire.contains(&format!("Host: 127.0.0.1:{port}\r\n")));
            assert!(!wire.contains("Authorization:"));
            stream.write_all(b"HTTP/1.1 409 Conflict\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
        });
        let mut input = request("/api/dsp/delay");
        input.body = Some("{\"nanoseconds\":18446744073709551615}".into());
        let reply = LocalConsole::new(port).unwrap().request(input).unwrap();
        assert_eq!(reply.status, 409);
        assert_eq!(reply.body, "{}");
        worker.join().unwrap();
    }

    #[test]
    fn refuses_redirect_ambiguous_framing_incomplete_or_oversized_response() {
        for reply in [
            "HTTP/1.1 302 Found\r\nContent-Type: application/json\r\nContent-Length: 0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 3\r\n\r\n{}",
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 2\r\n\r\n{}",
        ] { assert!(parse_reply(reply.as_bytes()).is_err()); }
        let mut bytes =
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2097153\r\n\r\n"
                .to_vec();
        bytes.extend(vec![b'x'; MAX_RESPONSE + 1]);
        assert!(parse_reply(&bytes).is_err());
    }

    #[test]
    fn bounds_admission_until_the_actual_worker_releases_it() {
        let client = LocalConsole::new(8384).unwrap();
        let permits: Vec<_> = (0..4).map(|_| client.acquire().unwrap()).collect();
        assert!(client.acquire().is_err());
        drop(permits);
        assert!(client.acquire().is_ok());
    }

    #[test]
    fn rejects_oversized_request_before_connecting_and_headers_before_parsing_json() {
        let mut input = request("/api/profiles/edit");
        input.body = Some("x".repeat(MAX_REQUEST + 1));
        assert!(LocalConsole::new(1)
            .unwrap()
            .request(input)
            .unwrap_err()
            .contains("1 MiB"));
        let header = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nX-Excess: {}\r\n\r\n{{}}", "x".repeat(MAX_HEADERS));
        assert!(parse_reply(header.as_bytes()).is_err());
    }
}
