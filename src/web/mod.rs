//! Bounded loopback HTTP transport for the existing core APIs.
mod api;
mod assets;

use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    body::{Bytes, Incoming},
    header::{HeaderMap, HeaderValue},
    server::conn::http1,
    service::service_fn,
    Method, Request, Response, StatusCode,
};
use hyper_util::rt::TokioIo;
use mixyard::{profiles, runtime};
use serde::Serialize;
use std::{convert::Infallible, io, io::Write, path::PathBuf, sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::Semaphore, time::timeout};

const MAX_BODY: usize = 1024 * 1024;
const MAX_RESPONSE: usize = 8 * 1024 * 1024;
const CONNECTION_DEADLINE: Duration = Duration::from_secs(15);
type Reply = Response<Full<Bytes>>;

struct State {
    authority: String,
    token: String,
    instance: String,
    store: profiles::Store,
    assets: assets::Assets,
    operations: Arc<Semaphore>,
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut store = None;
    let mut instance = "default".to_string();
    let mut port = 8384;
    let mut assets = PathBuf::from("apps/web/dist");
    let mut args = std::env::args().skip(1);
    while let Some(option) = args.next() {
        if option == "--help" || option == "-h" {
            println!("Usage: mixyard-web [--store DIRECTORY] [--instance NAME] [--port PORT] [--assets DIRECTORY]\nLoopback draft editing and read-only diagnostics; no audio or runtime lifecycle mutations.");
            return Ok(());
        }
        if !["--store", "--instance", "--port", "--assets"].contains(&option.as_str()) {
            return Err(format!("Unknown option: {option}; see --help").into());
        }
        let value = args
            .next()
            .ok_or_else(|| format!("Missing value for {option}"))?;
        match option.as_str() {
            "--store" => store = Some(PathBuf::from(value)),
            "--instance" => instance = value,
            "--port" => port = value.parse::<u16>()?,
            "--assets" => assets = PathBuf::from(value),
            _ => unreachable!(),
        }
    }
    runtime::endpoint(&instance)?;
    let store = profiles::Store::new(match store {
        Some(path) => path,
        None => profiles::default_directory()?,
    })?;
    let assets = assets::Assets::load(&assets)?;
    let mut random = [0u8; 32];
    getrandom::fill(&mut random).map_err(|e| io::Error::other(e.to_string()))?;
    let token = random.iter().map(|byte| format!("{byte:02x}")).collect();
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(4)
        .enable_all()
        .build()?
        .block_on(async move {
            let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
            let authority = listener.local_addr()?.to_string();
            println!("Mixyard web: http://{authority}");
            io::stdout().flush()?;
            let state = Arc::new(State {
                authority,
                token,
                instance,
                store,
                assets,
                operations: Arc::new(Semaphore::new(4)),
            });
            let connections = Arc::new(Semaphore::new(32));
            loop {
                let (stream, _) = listener.accept().await?;
                // No unbounded queue of tasks or sockets under load.
                let Ok(permit) = connections.clone().try_acquire_owned() else {
                    drop(stream);
                    continue;
                };
                let state = state.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let service = service_fn(move |request| {
                        let state = state.clone();
                        async move { Ok::<_, Infallible>(handle(request, state).await) }
                    });
                    let mut builder = http1::Builder::new();
                    builder
                        .keep_alive(false)
                        .max_buf_size(16 * 1024)
                        .max_headers(64);
                    // Also bounds incomplete headers, slow readers and response writes.
                    let _ = timeout(
                        CONNECTION_DEADLINE,
                        builder.serve_connection(TokioIo::new(stream), service),
                    )
                    .await;
                });
            }
            #[allow(unreachable_code)]
            Ok::<(), io::Error>(())
        })?;
    Ok(())
}

fn one_header<'a>(headers: &'a HeaderMap, key: &str) -> Option<&'a str> {
    let mut values = headers.get_all(key).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        None
    } else {
        Some(value)
    }
}

async fn handle(request: Request<Incoming>, state: Arc<State>) -> Reply {
    if one_header(request.headers(), "host") != Some(state.authority.as_str())
        || request.uri().authority().is_some()
    {
        return error(
            StatusCode::FORBIDDEN,
            "invalid-host",
            "Use the printed loopback URL and port",
        );
    }
    if request.headers().contains_key("origin")
        && one_header(request.headers(), "origin")
            != Some(format!("http://{}", state.authority).as_str())
    {
        return error(
            StatusCode::FORBIDDEN,
            "invalid-origin",
            "Only the service's own origin is accepted",
        );
    }
    let path = request.uri().path().to_string();
    if path == "/api/session" && request.method() == Method::GET {
        return json(
            &serde_json::json!({"csrfToken":state.token,"apiVersion":1,"audioAvailable":false}),
        );
    }
    if !path.starts_with("/api/") && path != "/api" {
        if request.method() != Method::GET {
            return error(
                StatusCode::METHOD_NOT_ALLOWED,
                "method-not-allowed",
                "Static files require GET",
            );
        }
        return state.assets.get(&path);
    }
    if request.method() != Method::POST {
        return error(
            StatusCode::METHOD_NOT_ALLOWED,
            "method-not-allowed",
            "This API endpoint requires POST",
        );
    }
    if one_header(request.headers(), "x-mixyard-token") != Some(state.token.as_str()) {
        return error(
            StatusCode::FORBIDDEN,
            "invalid-token",
            "Refresh the service session before making API requests",
        );
    }
    let content_type = one_header(request.headers(), "content-type").unwrap_or("");
    if !content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .eq_ignore_ascii_case("application/json")
    {
        return error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "json-required",
            "Use Content-Type: application/json",
        );
    }
    let bytes = match timeout(
        Duration::from_secs(3),
        Limited::new(request.into_body(), MAX_BODY).collect(),
    )
    .await
    {
        Err(_) => {
            return error(
                StatusCode::REQUEST_TIMEOUT,
                "request-timeout",
                "Request body deadline exceeded",
            )
        }
        Ok(Err(_)) => {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "invalid-body",
                "Body is incomplete or exceeds the 1 MiB limit",
            )
        }
        Ok(Ok(body)) => body.to_bytes(),
    };
    let permit = match state.operations.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "service-busy",
                "Four operations are already in progress; refresh before retrying a write",
            )
        }
    };
    // The permit remains with blocking work after a timeout: abandoned work can
    // never create an unbounded blocking queue. Never cancel/retry a store write.
    let work = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        api::dispatch(&path, &bytes, &state)
    });
    match timeout(Duration::from_secs(6), work).await {
        Ok(Ok(response)) => response,
        Ok(Err(_)) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "operation-failed",
            "Operation failed; reread the store before retrying a write",
        ),
        Err(_) => error(
            StatusCode::GATEWAY_TIMEOUT,
            "operation-timeout",
            "Operation may still complete; reread the store and review before retrying a write",
        ),
    }
}

fn response(status: StatusCode, mime: &'static str, bytes: Bytes) -> Reply {
    let mut reply = Response::new(Full::new(bytes));
    *reply.status_mut() = status;
    for (name, value) in [
        ("content-type", mime), ("cache-control", "no-store"),
        ("x-content-type-options", "nosniff"), ("connection", "close"),
        ("referrer-policy", "no-referrer"),
        ("content-security-policy", "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'"),
    ] {
        reply.headers_mut().insert(name, HeaderValue::from_static(value));
    }
    reply
}
fn error(status: StatusCode, code: &str, message: &str) -> Reply {
    let value = serde_json::json!({"error":{"code":code,"message":message}});
    response(
        status,
        "application/json",
        serde_json::to_vec(&value).unwrap().into(),
    )
}
struct Bounded(Vec<u8>);
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_RESPONSE - self.0.len() {
            return Err(io::Error::other("Response exceeds 8 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn json(value: &impl Serialize) -> Reply {
    let mut output = Bounded(Vec::new());
    match serde_json::to_writer(&mut output, value) {
        Ok(()) => response(StatusCode::OK, "application/json", output.0.into()),
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "response-too-large",
            "Cannot serialize result within the 8 MiB limit; reread before retrying a write",
        ),
    }
}
