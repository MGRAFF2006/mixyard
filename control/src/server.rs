use crate::{
    fail,
    identity::{allowed_ip, random, Credential},
    web::remote::Canonical,
    Failure, IdentityStore, Permission, Result,
};
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    body::{Bytes, Incoming},
    header::HeaderMap,
    server::conn::http1,
    service::service_fn,
    Method, Request, Response,
};
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    convert::Infallible,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::TcpListener, sync::Semaphore, time::timeout};
use tokio_rustls::TlsAcceptor;

pub struct Options {
    pub enabled: bool,
    pub bind: SocketAddr,
    pub store: PathBuf,
    pub assets: PathBuf,
    pub instance: String,
}

struct Inner {
    identity: IdentityStore,
    canonical: Canonical,
    authority_id: String,
}
pub struct Control {
    listener: TcpListener,
    tls: TlsAcceptor,
    inner: Arc<Mutex<Inner>>,
    address: SocketAddr,
    operations: Arc<Semaphore>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Pair {
    device_id: String,
    invitation: String,
    name: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Envelope {
    device_id: String,
    authority_id: String,
    revision: u64,
    path: Option<String>,
    body: Value,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Revoke {
    credential_id: String,
}
fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|_| {
        fail(
            400,
            "invalid-json",
            "JSON fields or types do not match this operation",
        )
    })
}
fn permission(credential: &Credential, role: Permission) -> Result<()> {
    if credential.permissions.contains(&role) {
        Ok(())
    } else {
        Err(fail(
            403,
            "permission-denied",
            format!("This operation requires {role:?} permission"),
        ))
    }
}
fn route_permission(path: &str) -> Result<Permission> {
    match path {
        "/api/status" | "/api/validate" | "/api/inspect" | "/api/profiles/read" |
        "/api/dsp/preview" | "/api/dsp/validate" | "/api/dsp/delay" |
        "/api/automation/status" | "/api/automation/settings/read" |
        "/api/automation/validate" | "/api/automation/evaluate" | "/api/automation/context" => Ok(Permission::View),
        "/api/profiles/edit" | "/api/automation/disarm" | "/api/automation/manual" => Ok(Permission::Control),
        "/api/automation/settings/write" => Ok(Permission::Settings),
        _ => Err(fail(422,"unsupported-route","Route is not remotely supported; remote arm/review/live and native audio are unavailable")),
    }
}
impl Inner {
    fn revision(&self) -> Result<u64> {
        self.canonical
            .revision()
            .map_err(|e| fail(503, "store-unavailable", e))
    }
    fn scope(&self, e: &Envelope) -> Result<()> {
        if e.device_id != self.identity.device_id() {
            return Err(fail(
                409,
                "target-mismatch",
                "Request belongs to another device",
            ));
        }
        if e.authority_id != self.authority_id {
            return Err(fail(
                409,
                "session-changed",
                "Service restarted; discard pending writes and review again",
            ));
        }
        if e.revision != self.revision()? {
            return Err(fail(
                409,
                "store-conflict",
                "Profile revision changed; reread session and review; never replay writes",
            ));
        }
        Ok(())
    }
    fn dispatch(&mut self, path: &str, bytes: &[u8], bearer: &str) -> Result<(u16, Value)> {
        if path == "/control/pair" {
            let body: Pair = parse(bytes)?;
            if body.device_id != self.identity.device_id() {
                return Err(fail(
                    409,
                    "target-mismatch",
                    "Pairing invitation belongs to another device",
                ));
            }
            return Ok((200, self.identity.pair(&body.invitation, body.name)?));
        }
        // Auth is always checked before resolving an administrative route.
        let credential = self.identity.authenticate(bearer)?;
        if path == "/control/session" {
            let _: Empty = parse(bytes)?;
            return Ok((
                200,
                json!({"deviceId":self.identity.device_id(),"authorityId":self.authority_id,"revision":self.revision()?,"permissions":credential.permissions,"backendAvailable":false,"audioApplied":false}),
            ));
        }
        let envelope: Envelope = parse(bytes)?;
        self.scope(&envelope)?;
        let (status, body) = if path == "/control/request" {
            let route = envelope
                .path
                .as_deref()
                .ok_or_else(|| fail(400, "invalid-json", "Request requires path"))?;
            permission(&credential, route_permission(route)?)?;
            // Preserve canonical expectedRevision, settings validation and policy checks.
            let body = serde_json::to_vec(&envelope.body).expect("JSON body");
            self.canonical.dispatch(route, &body)
        } else {
            if envelope.path.is_some() {
                return Err(fail(
                    400,
                    "invalid-json",
                    "Administrative envelopes have no path",
                ));
            }
            permission(&credential, Permission::Settings)?;
            let body = serde_json::to_vec(&envelope.body).expect("JSON body");
            (
                200,
                match path {
                    "/control/credentials" => {
                        let _: Empty = parse(&body)?;
                        json!({"credentials":self.identity.credentials()})
                    }
                    "/control/revoke" => {
                        let body: Revoke = parse(&body)?;
                        self.identity.revoke(&body.credential_id)?;
                        json!({"revoked":true})
                    }
                    "/control/rotate" => {
                        let _: Empty = parse(&body)?;
                        self.identity.rotate(bearer)?
                    }
                    _ => return Err(fail(422, "unsupported-route", "Unknown control operation")),
                },
            )
        };
        // Serial dispatch means revocation/rotation cannot overtake an admitted write.
        // A disconnected client still must reread, never infer failure means no write.
        Ok((
            status,
            json!({"deviceId":self.identity.device_id(),"authorityId":self.authority_id,"revision":self.revision()?,"status":status,"body":body}),
        ))
    }
}

impl Control {
    pub async fn bind(identity: IdentityStore, options: Options) -> Result<Self> {
        if !options.enabled {
            return Err(fail(
                400,
                "opt-in-required",
                "Pass --enable-control explicitly",
            ));
        }
        if !allowed_ip(options.bind.ip())
            || (options.bind.ip() != identity.bind_ip() && !options.bind.ip().is_loopback())
        {
            return Err(fail(400,"unsafe-bind","Use exact provisioned private LAN/Tailscale IP or loopback; wildcard/public binds are forbidden"));
        }
        let tls = TlsAcceptor::from(Arc::new(identity.tls()?));
        let canonical = Canonical::new(&options.store, &options.assets, options.instance)
            .map_err(|e| fail(503, "store-unavailable", e.to_string()))?;
        let listener = TcpListener::bind(options.bind).await?;
        let address = listener.local_addr()?;
        Ok(Self {
            listener,
            tls,
            address,
            inner: Arc::new(Mutex::new(Inner {
                identity,
                canonical,
                authority_id: random()?,
            })),
            operations: Arc::new(Semaphore::new(1)),
        })
    }
    pub fn address(&self) -> SocketAddr {
        self.address
    }
    pub fn readiness(&self) -> Value {
        let inner = self.inner.lock().expect("new owner lock");
        json!({"event":"listening","address":self.address.to_string(),"deviceId":inner.identity.device_id(),"authorityId":inner.authority_id,"tls":true})
    }
    pub async fn serve(self) -> Result<()> {
        self.serve_until(std::future::pending()).await
    }
    pub async fn serve_until(self, shutdown: impl std::future::Future<Output = ()>) -> Result<()> {
        let slots = Arc::new(Semaphore::new(32));
        let mut tasks = tokio::task::JoinSet::new();
        tokio::pin!(shutdown);
        loop {
            let incoming = tokio::select! {
                _ = &mut shutdown => break,
                incoming = self.listener.accept() => incoming,
                Some(_) = tasks.join_next(), if !tasks.is_empty() => continue,
            };
            let (stream, _) = incoming?;
            let Ok(permit) = slots.clone().try_acquire_owned() else {
                continue;
            };
            let tls = self.tls.clone();
            let inner = self.inner.clone();
            let operations = self.operations.clone();
            let authority = self.address.to_string();
            tasks.spawn(async move {
                let _permit = permit;
                let work = async {
                    let stream = tls.accept(stream).await?;
                    let service = service_fn(move |req| {
                        let inner = inner.clone();
                        let operations = operations.clone();
                        let authority = authority.clone();
                        async move {
                            Ok::<_, Infallible>(handle(req, &authority, inner, operations).await)
                        }
                    });
                    let mut builder = http1::Builder::new();
                    builder
                        .keep_alive(false)
                        .max_buf_size(16 * 1024)
                        .max_headers(64);
                    builder
                        .serve_connection(TokioIo::new(stream), service)
                        .await
                        .map_err(std::io::Error::other)
                };
                let _ = timeout(Duration::from_secs(15), work).await;
            });
        }
        // Each owned connection has a 15s total deadline. Blocking commits retain
        // their owner reference until finished, even after a socket timeout.
        while tasks.join_next().await.is_some() {}
        Ok(())
    }
}
fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        None
    } else {
        Some(value)
    }
}
type Reply = Response<Full<Bytes>>;
fn reply(status: u16, body: Value) -> Reply {
    let bytes = serde_json::to_vec(&body).expect("response JSON");
    if bytes.len() > 9 * 1024 * 1024 {
        return failure(fail(
            503,
            "response-too-large",
            "Response exceeded budget; reread before retrying",
        ));
    }
    let mut response = Response::new(Full::new(bytes.into()));
    *response.status_mut() = hyper::StatusCode::from_u16(status).expect("valid status");
    for (name, value) in [
        ("content-type", "application/json"),
        ("cache-control", "no-store"),
        ("connection", "close"),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        (
            "content-security-policy",
            "default-src 'none'; frame-ancestors 'none'",
        ),
    ] {
        response
            .headers_mut()
            .insert(name, hyper::header::HeaderValue::from_static(value));
    }
    response
}
fn failure(error: Failure) -> Reply {
    reply(error.status, json!({"error":error}))
}
async fn handle(
    request: Request<Incoming>,
    authority: &str,
    inner: Arc<Mutex<Inner>>,
    operations: Arc<Semaphore>,
) -> Reply {
    if header(request.headers(), "host") != Some(authority)
        || request.uri().authority().is_some()
        || request.uri().query().is_some()
    {
        return failure(fail(
            403,
            "invalid-host",
            "Use the exact configured TLS authority with no query",
        ));
    }
    if request.headers().contains_key("origin")
        && header(request.headers(), "origin") != Some(format!("https://{authority}").as_str())
    {
        return failure(fail(
            403,
            "invalid-origin",
            "Only this TLS service origin is accepted",
        ));
    }
    if request.method() != Method::POST {
        return failure(fail(
            405,
            "method-not-allowed",
            "Control operations require POST JSON",
        ));
    }
    if header(request.headers(), "content-type")
        .and_then(|v| v.split(';').next())
        .map(str::trim)
        != Some("application/json")
    {
        return failure(fail(
            415,
            "json-required",
            "Content-Type must be application/json",
        ));
    }
    let bearer = header(request.headers(), "authorization")
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| v.len() == 64)
        .unwrap_or("")
        .to_string();
    let path = request.uri().path().to_string();
    // The only unauthenticated operation requires a high-entropy local invitation.
    if path != "/control/pair" && bearer.is_empty() {
        return failure(fail(401, "unauthorized", "Bearer credential required"));
    }
    let bytes = match timeout(
        Duration::from_secs(3),
        Limited::new(request.into_body(), 1024 * 1024).collect(),
    )
    .await
    {
        Err(_) => return failure(fail(408, "request-timeout", "Body deadline exceeded")),
        Ok(Err(_)) => {
            return failure(fail(
                413,
                "invalid-body",
                "Body incomplete or exceeds 1 MiB",
            ))
        }
        Ok(Ok(body)) => body.to_bytes(),
    };
    let Ok(permit) = operations.try_acquire_owned() else {
        return failure(fail(
            503,
            "service-busy",
            "Operation active; refresh before retrying a write",
        ));
    };
    let work = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        inner
            .lock()
            .map_err(|_| fail(503, "owner-unavailable", "Control owner lock failed"))?
            .dispatch(&path, &bytes, &bearer)
    });
    match timeout(Duration::from_secs(9), work).await {
        Ok(Ok(Ok((status, body)))) => reply(status, body),
        Ok(Ok(Err(error))) => failure(error),
        Ok(Err(_)) => failure(fail(
            503,
            "operation-failed",
            "Operation failed; reread before retrying",
        )),
        Err(_) => failure(fail(
            504,
            "operation-timeout",
            "Operation may complete; reread before retrying, never replay automatically",
        )),
    }
}
