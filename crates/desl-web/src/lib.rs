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
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use std::future::IntoFuture;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// The port `desl web` listens on unless told otherwise. When it is taken, the next free one.
pub const PORT: u16 = 7428;

/// How many ports from [`PORT`] on it tries.
const TRIES: u16 = 20;

/// The page's files, compiled in: each address, its media type and its text.
const FILES: &[(&str, &str, &str)] = &[
    ("/", "text/html; charset=utf-8", include_str!("../page/index.html")),
    ("/app.js", "text/javascript; charset=utf-8", include_str!("../page/app.js")),
    ("/forms.js", "text/javascript; charset=utf-8", include_str!("../page/forms.js")),
    ("/tasks.js", "text/javascript; charset=utf-8", include_str!("../page/tasks.js")),
    ("/attention.js", "text/javascript; charset=utf-8", include_str!("../page/attention.js")),
    ("/app.css", "text/css; charset=utf-8", include_str!("../page/app.css")),
];

/// What every connection is checked against.
#[derive(Clone)]
pub(crate) struct Shared {
    /// The key in the address `desl web` printed: 128 random bits, new at each start.
    token: Arc<str>,
    port: u16,
    /// The daemon's socket.
    socket: Arc<PathBuf>,
}

/// How [`run`] ended.
pub enum Ended {
    /// ctrl+c, or `desl web --stop`.
    Stopped,
    /// An install replaced the binary it was started from: it is to start again from the new
    /// one, on the same port with the same key, so that open pages connect again and carry on.
    Replaced { port: u16, key: String },
}

/// How often desl web looks at its binary for a new one.
const BINARY_CHECK: Duration = Duration::from_secs(2);

/// Serves the page until ctrl+c, or until `exe`, the binary it runs from, is replaced. `ready`
/// gets the address to open, the key included, as soon as the page can be served. `port` 0
/// takes any free port; `key`, when given, is the one desl web had before it started again.
pub fn run(socket: &Path, port: Option<u16>, key: Option<String>, exe: Option<PathBuf>, ready: impl FnOnce(&str)) -> Result<Ended> {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().context("desl web could not start its runtime")?;
    let again = key.is_some();
    rt.block_on(async move {
        let (listener, port) = if again { listen_again(port).await? } else { listen(port).await? };
        let token: Arc<str> = match key.filter(|k| k.len() == 32 && k.bytes().all(|c| c.is_ascii_hexdigit())) {
            Some(k) => k.into(),
            None => self::key()?.into(),
        };
        ready(&format!("http://127.0.0.1:{port}/#t={token}"));
        let shared = Shared { token: token.clone(), port, socket: Arc::new(socket.to_path_buf()) };
        let replaced = async {
            match exe {
                Some(exe) => replaced(&exe, BINARY_CHECK).await,
                None => std::future::pending().await,
            }
        };
        // Not a graceful stop: an open page keeps its socket open for as long as it is open.
        tokio::select! {
            served = axum::serve(listener, app(shared)).into_future() => {
                served.context("desl web stopped")?;
                Ok(Ended::Stopped)
            }
            _ = tokio::signal::ctrl_c() => Ok(Ended::Stopped),
            () = replaced => Ok(Ended::Replaced { port, key: token.to_string() }),
        }
    })
}

/// Resolves once the file at `exe` is another one than at the start, and has stayed the same
/// for one more look: an install that writes it in place is finished by then.
async fn replaced(exe: &Path, every: Duration) {
    use std::os::unix::fs::MetadataExt;
    let id = |p: &Path| std::fs::metadata(p).ok().map(|m| (m.dev(), m.ino(), m.mtime(), m.mtime_nsec(), m.len()));
    let first = id(exe);
    let mut last = first;
    loop {
        tokio::time::sleep(every).await;
        let now = id(exe);
        if now.is_some() && now != first && now == last {
            return;
        }
        last = now;
    }
}

/// The port desl web had before it started again from a new binary, taken again: the old
/// listener closed with the old program, a moment ago.
async fn listen_again(port: Option<u16>) -> Result<(tokio::net::TcpListener, u16)> {
    let p = port.context("desl web started again without its port")?;
    for _ in 0..30 {
        if let Ok(l) = tokio::net::TcpListener::bind(("127.0.0.1", p)).await {
            return Ok((l, p));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!("Port {p} stayed taken, so desl web could not start again there. Run `desl web` for a new address.")
}

/// What tells one build of the page from another: a hash of its files. A page that finds it
/// changed loads itself again.
pub(crate) fn build() -> &'static str {
    static BUILD: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    BUILD.get_or_init(|| {
        let h = FILES
            .iter()
            .flat_map(|(_, _, body)| body.bytes())
            .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
        format!("{h:016x}")
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
    Router::new().route("/ws", get(socket)).fallback(asset).with_state(shared)
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

/// The page's Content-Security-Policy: everything from the page itself, nothing inline, the
/// socket only to this desl web (web-spec.md "Security", rule 4). No agent text is ever HTML.
pub(crate) fn csp(p: u16) -> String {
    format!(
        "default-src 'self'; connect-src 'self' ws://127.0.0.1:{p} ws://localhost:{p}; img-src 'self' data:; base-uri 'none'; \
         form-action 'none'; frame-ancestors 'none'"
    )
}

/// One of the page's files, with the headers that keep agent text from ever running as code.
fn file(h: &HeaderMap, s: &Shared, kind: &'static str, body: &'static str) -> Response {
    if !host_ok(h, s.port) {
        return refused();
    }
    let csp = csp(s.port);
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

/// One of the page's files, or a sentence that there is no such page.
async fn asset(State(s): State<Shared>, h: HeaderMap, uri: Uri) -> Response {
    match FILES.iter().find(|(path, _, _)| *path == uri.path()) {
        Some((_, kind, body)) => file(&h, &s, kind, body),
        None => (StatusCode::NOT_FOUND, "desl web has no such page.\n").into_response(),
    }
}

async fn socket(State(s): State<Shared>, h: HeaderMap, up: WebSocketUpgrade) -> Response {
    if !host_ok(&h, s.port) || !origin_ok(&h, s.port) {
        return refused();
    }
    up.on_upgrade(move |ws| bridge::serve(ws, s))
}

#[cfg(test)]
mod tests;
