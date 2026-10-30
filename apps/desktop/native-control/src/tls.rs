//! Exact private numeric endpoint + provisioned leaf pin, then normal WebPKI verification.
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    net::{IpAddr, SocketAddr, TcpStream},
    sync::Arc,
    time::{Duration, Instant},
};

pub const MAX_BODY: usize = 1024 * 1024;
const MAX_REPLY: usize = 2 * MAX_BODY;
const MAX_HEADERS: usize = 16 * 1024;

pub fn endpoint(value: &str) -> Result<SocketAddr, String> {
    let address = value
        .strip_prefix("https://")
        .ok_or("Use https://PRIVATE_NUMERIC_IP:PORT without a path")?
        .parse::<SocketAddr>()
        .map_err(|_| "Use an exact numeric private IP and explicit port")?;
    let allowed = match address.ip() {
        IpAddr::V4(ip) => {
            ip.is_loopback()
                || ip.is_private()
                || (ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]))
        }
        IpAddr::V6(ip) => ip.is_loopback() || (ip.segments()[0] & 0xfe00 == 0xfc00),
    };
    if !allowed || address.port() == 0 {
        return Err("Public, wildcard and link-local endpoints are unsupported".into());
    }
    Ok(address)
}

pub fn configuration(pem: &str, pin: &str) -> Result<Arc<ClientConfig>, String> {
    let certificates = rustls_pemfile::certs(&mut pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Invalid certificate PEM")?;
    if certificates.len() != 1 {
        return Err("Supply exactly one provisioned device certificate".into());
    }
    let cert = certificates[0].clone();
    let digest = Sha256::digest(cert.as_ref())
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect::<String>();
    if pin != digest {
        return Err("Certificate SHA-256 does not match the out-of-band pin".into());
    }
    let mut roots = RootCertStore::empty();
    roots
        .add(cert.clone())
        .map_err(|_| "Invalid trust certificate")?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(
        Arc::new(roots),
        provider.clone(),
    )
    .build()
    .map_err(|_| "Invalid certificate verifier")?;
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|_| "TLS configuration failed")?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned { cert, verifier }))
        .with_no_client_auth();
    config.enable_early_data = false;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

#[derive(Debug)]
struct Pinned {
    cert: CertificateDer<'static>,
    verifier: Arc<rustls::client::WebPkiServerVerifier>,
}
impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if end.as_ref() != self.cert.as_ref() {
            return Err(rustls::Error::General(
                "Device certificate pin mismatch".into(),
            ));
        }
        self.verifier
            .verify_server_cert(end, intermediates, name, ocsp, now)
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.verifier.verify_tls12_signature(message, cert, dss)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.verifier.verify_tls13_signature(message, cert, dss)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.verifier.supported_verify_schemes()
    }
}

struct Deadline {
    stream: TcpStream,
    end: Instant,
}
impl Deadline {
    fn remaining(&self) -> std::io::Result<Duration> {
        self.end
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::TimedOut, "Device deadline"))
    }
}
impl Read for Deadline {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.stream.set_read_timeout(Some(self.remaining()?))?;
        self.stream.read(bytes)
    }
}
impl Write for Deadline {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        self.stream.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}
#[derive(serde::Serialize)]
pub struct Reply {
    pub status: u16,
    pub body: String,
}
fn network(_: impl std::fmt::Display) -> String {
    "Paired TLS request failed or timed out. An admitted write may have completed; refresh and review. Verify endpoint, certificate and service availability.".into()
}

pub fn post(
    address: SocketAddr,
    config: Arc<ClientConfig>,
    route: &str,
    body: &str,
    bearer: Option<&str>,
) -> Result<Reply, String> {
    if body.len() > MAX_BODY {
        return Err("Control request exceeds 1 MiB".into());
    }
    let end = Instant::now() + Duration::from_secs(10);
    let socket = TcpStream::connect_timeout(&address, Duration::from_secs(2)).map_err(network)?;
    let connection =
        rustls::ClientConnection::new(config, ServerName::IpAddress(address.ip().into()))
            .map_err(network)?;
    let mut tls = rustls::StreamOwned::new(
        connection,
        Deadline {
            stream: socket,
            end,
        },
    );
    // Explicit handshake: no invitation or credential bytes before pin/name/time/signature checks.
    while tls.conn.is_handshaking() {
        tls.conn.complete_io(&mut tls.sock).map_err(network)?;
    }
    let authorization = match bearer {
        Some(value) => {
            super::hex_id(value)?;
            format!("Authorization: Bearer {value}\r\n")
        }
        None => String::new(),
    };
    let head = format!("POST {route} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nContent-Type: application/json\r\n{authorization}Content-Length: {}\r\n\r\n", body.len());
    tls.write_all(head.as_bytes()).map_err(network)?;
    tls.write_all(body.as_bytes()).map_err(network)?;
    tls.flush().map_err(network)?;
    let mut bytes = Vec::new();
    let mut framing = None;
    loop {
        let mut chunk = [0; 8192];
        let n = tls.read(&mut chunk).map_err(network)?;
        if n == 0 {
            return Err("Truncated control response; refresh before another write".into());
        }
        bytes.extend_from_slice(&chunk[..n]);
        if bytes.len() > MAX_HEADERS + MAX_REPLY {
            return Err("Control response exceeds 2 MiB".into());
        }
        if framing.is_none() {
            if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                if end + 4 > MAX_HEADERS {
                    return Err("Control headers exceed limit".into());
                }
                framing = Some(headers(&bytes[..end])?);
                let (_, length) = framing.unwrap();
                framing = Some((end + 4, length));
            } else if bytes.len() > MAX_HEADERS {
                return Err("Control headers exceed limit".into());
            }
        }
        if let Some((offset, length)) = framing {
            if bytes.len() > offset + length {
                return Err("Invalid control HTTP framing".into());
            }
            if bytes.len() == offset + length {
                let (status, _) = headers(&bytes[..offset - 4])?;
                return Ok(Reply {
                    status: status as u16,
                    body: String::from_utf8(bytes[offset..].to_vec())
                        .map_err(|_| "Control response is not UTF-8")?,
                });
            }
        }
    }
}
fn headers(bytes: &[u8]) -> Result<(usize, usize), String> {
    let invalid = || "Invalid control HTTP framing; redirects are denied".to_string();
    let header = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let mut lines = header.split("\r\n");
    let mut status = lines.next().ok_or_else(invalid)?.split(' ');
    if status.next() != Some("HTTP/1.1") {
        return Err(invalid());
    }
    let status: usize = status
        .next()
        .ok_or_else(invalid)?
        .parse()
        .map_err(|_| invalid())?;
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
    let length = length.filter(|n| *n <= MAX_REPLY).ok_or_else(invalid)?;
    if !json {
        return Err(invalid());
    }
    Ok((status, length))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_numeric_endpoint_cannot_become_public_dns_url_or_proxy_path() {
        for value in [
            "https://0.0.0.0:443",
            "https://8.8.8.8:443",
            "https://example.com:443",
            "http://127.0.0.1:443",
            "https://127.0.0.1:0",
            "https://127.0.0.1:443/path",
            "https://user@127.0.0.1:443",
            "https://[::]:443",
            "https://[fe80::1]:443",
        ] {
            assert!(endpoint(value).is_err(), "{value}");
        }
        for value in [
            "https://127.0.0.1:8385",
            "https://192.168.1.20:443",
            "https://100.64.1.2:443",
            "https://[::1]:443",
            "https://[fd00::1]:443",
        ] {
            assert!(endpoint(value).is_ok(), "{value}");
        }
    }
    #[test]
    fn rejects_redirect_duplicate_length_and_chunked_framing_before_body() {
        for header in [b"HTTP/1.1 307 Redirect\r\nContent-Type: application/json\r\nContent-Length: 2".as_slice(), b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nContent-Length: 2", b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked", b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 999999999"] {
            assert!(headers(header).is_err());
        }
        assert_eq!(
            headers(b"HTTP/1.1 403 Denied\r\nContent-Type: application/json\r\nContent-Length: 2")
                .unwrap(),
            (403, 2)
        );
    }
}
