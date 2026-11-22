//! `desl web` (web-spec.md): the workspace in a browser. A client of the daemon like the TUI:
//! it serves the page from memory on 127.0.0.1 and joins each browser tab to the daemon over the
//! documented protocol (see [`bridge`]), adding the key and origin checks and what the protocol
//! does not carry. Its async runtime starts only when `desl web` runs; the daemon and the TUI
//! stay on threads.

mod bridge;

use anyhow::{Context, Result, bail};
use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::extract::ws::WebSocketUpgrade;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The port `desl web` listens on unless told otherwise. When it is taken, the next free one.
pub const PORT: u16 = 7428;

/// How many ports from [`PORT`] on it tries.
const TRIES: u16 = 20;

const INDEX: &str = include_str!("../page/index.html");
const SCRIPT: &str = include_str!("../page/app.js");
const STYLE: &str = include_str!("../page/app.css");

/// What every connection is checked against.
#[derive(Clone)]
pub(crate) struct Shared {
    /// The key in the address `desl web` printed: 128 random bits, new at each start.
    token: Arc<str>,
    port: u16,
    /// The daemon's socket.
    socket: Arc<PathBuf>,
}

/// Serves the page until ctrl+c. `ready` gets the address to open, the key included, as soon as
/// the page can be served. `port` 0 takes any free port.
pub fn run(socket: &Path, port: Option<u16>, ready: impl FnOnce(&str)) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().context("desl web could not start its runtime")?;
    rt.block_on(async move {
        let (listener, port) = listen(port).await?;
        let token: Arc<str> = key()?.into();
        ready(&format!("http://127.0.0.1:{port}/#t={token}"));
        let shared = Shared { token, port, socket: Arc::new(socket.to_path_buf()) };
        axum::serve(listener, app(shared))
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
            .context("desl web stopped")
    })
}

/// A listener on 127.0.0.1, never on another address (web-spec.md "Security").
async fn listen(port: Option<u16>) -> Result<(tokio::net::TcpListener, u16)> {
    let ports: Vec<u16> = match port {
        Some(p) => vec![p],
        None => (PORT..PORT + TRIES).collect(),
    };
    for p in ports {
        if let Ok(l) = tokio::net::TcpListener::bind(("127.0.0.1", p)).await {
            let port = l.local_addr().map(|a| a.port()).unwrap_or(p);
            return Ok((l, port));
        }
    }
    match port {
        Some(p) => bail!("Port {p} is taken on this machine. Run `desl web` without --port to take a free one."),
        None => bail!("Ports {PORT} to {} are all taken on this machine. Run `desl web --port <port>` with a free one.", PORT + TRIES - 1),
    }
}

/// 128 random bits as 32 hex digits.
fn key() -> Result<String> {
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .context("desl web could not read random bytes for its key from /dev/urandom")?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

pub(crate) fn app(shared: Shared) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/app.js", get(script))
        .route("/app.css", get(style))
        .route("/ws", get(socket))
        .fallback(missing)
        .with_state(shared)
}

/// The request names this server by an address it answers at. Anything else (a rebinding host
/// name above all) is refused before it gets a byte.
fn host_ok(h: &HeaderMap, port: u16) -> bool {
    let host = h.get(header::HOST).and_then(|v| v.to_str().ok());
    host.is_some_and(|v| v == format!("127.0.0.1:{port}") || v == format!("localhost:{port}"))
}

/// A WebSocket opened by the page itself: browsers let any site open one to localhost, and say
/// which site it was in `Origin`.
fn origin_ok(h: &HeaderMap, port: u16) -> bool {
    let origin = h.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    origin.is_some_and(|v| v == format!("http://127.0.0.1:{port}") || v == format!("http://localhost:{port}"))
}

fn refused() -> Response {
    (StatusCode::FORBIDDEN, "desl web answers only to its own page, at 127.0.0.1 or localhost.\n").into_response()
}

/// One of the page's files, with the headers that keep agent text from ever running as code.
fn file(h: &HeaderMap, s: &Shared, kind: &'static str, body: &'static str) -> Response {
    if !host_ok(h, s.port) {
        return refused();
    }
    let p = s.port;
    let csp = format!(
        "default-src 'self'; connect-src 'self' ws://127.0.0.1:{p} ws://localhost:{p}; img-src 'self' data:; \
         base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
    );
    let mut r = Response::new(Body::from(body));
    let hs = r.headers_mut();
    hs.insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    hs.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    hs.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    hs.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    if let Ok(v) = HeaderValue::from_str(&csp) {
        hs.insert(header::CONTENT_SECURITY_POLICY, v);
    }
    r
}

async fn index(State(s): State<Shared>, h: HeaderMap) -> Response {
    file(&h, &s, "text/html; charset=utf-8", INDEX)
}

async fn script(State(s): State<Shared>, h: HeaderMap) -> Response {
    file(&h, &s, "text/javascript; charset=utf-8", SCRIPT)
}

async fn style(State(s): State<Shared>, h: HeaderMap) -> Response {
    file(&h, &s, "text/css; charset=utf-8", STYLE)
}

async fn missing() -> Response {
    (StatusCode::NOT_FOUND, "desl web has no such page.\n").into_response()
}

async fn socket(State(s): State<Shared>, h: HeaderMap, up: WebSocketUpgrade) -> Response {
    if !host_ok(&h, s.port) || !origin_ok(&h, s.port) {
        return refused();
    }
    up.on_upgrade(move |ws| bridge::serve(ws, s))
}

#[cfg(test)]
mod tests;
