//! One browser tab joined to the daemon (web-spec.md "Architecture"). The tab proves it holds
//! the key, then gets a connection of its own to the daemon: every frame the daemon sends goes
//! to the tab as its JSON, and every request the tab sends goes to the daemon, so the page
//! speaks the protocol in `docs/protocol.md`. The bridge adds one message of its own, `view`:
//! the sidebar as `desl_proto::view` words it for the TUI, sent whenever it changes, so the
//! two clients cannot show different things. It keeps nothing that matters: closing it, or the
//! tab, loses nothing.

use crate::Shared;
use axum::extract::ws::{CloseFrame, Message, WebSocket};
use desl_ir::{Project, Thread};
use desl_proto::{Change, Command, PROTOCOL, Reply, Request, ServerFrame, view};
use serde_json::{Value, json};
use std::io;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::UnixStream;

/// Request ids the bridge uses for itself. The page's stay below them, and replies to these go
/// no further than the bridge. Below 2^53, so a page's numbers never reach them.
const OWN: u64 = 1 << 52;
pub(crate) const HELLO: u64 = OWN;
pub(crate) const SUBSCRIBE: u64 = OWN + 1;

/// How long a tab has to say the key before it is turned away.
const KEY_WAIT: Duration = Duration::from_secs(5);

/// How often the view is worked out again when nothing changed, for the ages in it (`21m`).
const AGES: Duration = Duration::from_secs(15);

/// A command the page may send. Not the ones that stop or replace the daemon that holds every
/// agent, and not `hello`, which the bridge has said.
fn allowed(c: &Command) -> bool {
    !matches!(c, Command::Shutdown | Command::Handover { .. } | Command::Hello { .. })
}

/// The key, said first and in time, compared without a shortcut that would time it.
async fn keyed(ws: &mut WebSocket, token: &str) -> bool {
    let Ok(Some(Ok(Message::Text(t)))) = tokio::time::timeout(KEY_WAIT, ws.recv()).await else { return false };
    let given = serde_json::from_str::<Value>(t.as_str()).ok().and_then(|v| v.get("token").and_then(Value::as_str).map(str::to_string));
    given.is_some_and(|g| same(g.as_bytes(), token.as_bytes()))
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub(crate) async fn serve(mut ws: WebSocket, s: Shared) {
    if !keyed(&mut ws, &s.token).await {
        let reason = "Open the address desl web printed: it carries the key.";
        let _ = ws.send(Message::Close(Some(CloseFrame { code: 4401, reason: reason.into() }))).await;
        return;
    }
    let daemon = match UnixStream::connect(s.socket.as_path()).await {
        Ok(d) => d,
        Err(_) => {
            let _ = say(&mut ws, json!({ "type": "bridge", "state": "no_daemon" })).await;
            return;
        }
    };
    let (mut rd, mut wr) = daemon.into_split();
    let hello = Command::Hello { protocol: PROTOCOL, client: "web".into() };
    if send(&mut wr, &Request { id: HELLO, command: hello }).await.is_err()
        || send(&mut wr, &Request { id: SUBSCRIBE, command: Command::Subscribe { cwd: None } }).await.is_err()
    {
        let _ = say(&mut ws, json!({ "type": "bridge", "state": "no_daemon" })).await;
        return;
    }
    let (tx, mut frames) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
    let reader = tokio::spawn(async move {
        while let Ok(Some(f)) = read_frame(&mut rd).await {
            if tx.send(f).await.is_err() {
                break;
            }
        }
    });
    let mut cache = Cache::default();
    let mut ages = tokio::time::interval(AGES);
    loop {
        tokio::select! {
            frame = frames.recv() => {
                let Some(bytes) = frame else {
                    // The daemon stopped or handed over: the page connects again.
                    let _ = say(&mut ws, json!({ "type": "bridge", "state": "daemon_gone" })).await;
                    break;
                };
                let parsed = serde_json::from_slice::<ServerFrame>(&bytes).ok();
                let own = matches!(&parsed, Some(ServerFrame::Reply { id, .. }) if *id >= OWN);
                let changed = parsed.is_some_and(|f| cache.fold(f));
                if !own && ws.send(Message::Text(String::from_utf8_lossy(&bytes).into_owned().into())).await.is_err() {
                    break;
                }
                if changed && cache.push(&mut ws).await.is_err() {
                    break;
                }
            }
            msg = ws.recv() => match msg {
                Some(Ok(Message::Text(t))) => match serde_json::from_str::<Request>(t.as_str()) {
                    Ok(r) if r.id < OWN && allowed(&r.command) => {
                        if send(&mut wr, &r).await.is_err() {
                            break;
                        }
                    }
                    Ok(r) => {
                        let sentence = "desl web does not send that to the daemon. Run it from desl's own CLI.";
                        let refused = json!({ "type": "reply", "id": r.id, "ok": null, "error": sentence });
                        if say(&mut ws, refused).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => {
                        let bad = json!({ "type": "bridge", "error": "desl web could not read that request." });
                        if say(&mut ws, bad).await.is_err() {
                            break;
                        }
                    }
                },
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            _ = ages.tick() => {
                if cache.push(&mut ws).await.is_err() {
                    break;
                }
            }
        }
    }
    reader.abort();
}

async fn say(ws: &mut WebSocket, v: Value) -> Result<(), axum::Error> {
    ws.send(Message::Text(v.to_string().into())).await
}

async fn send(wr: &mut (impl AsyncWrite + Unpin), r: &Request) -> io::Result<()> {
    let bytes = desl_proto::encode(r)?;
    wr.write_all(&bytes).await
}

/// One frame from the daemon: `None` when it closed the connection between frames.
pub(crate) async fn read_frame(rd: &mut (impl AsyncRead + Unpin)) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match rd.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let n = u32::from_be_bytes(len) as usize;
    if n > desl_proto::MAX_FRAME {
        return Err(io::Error::other("frame too large"));
    }
    let mut body = vec![0u8; n];
    rd.read_exact(&mut body).await?;
    Ok(Some(body))
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// The workspace as the daemon last said it, enough to work out the view.
#[derive(Default)]
pub(crate) struct Cache {
    projects: Vec<Project>,
    threads: Vec<Thread>,
    /// The subscription was answered: the view can be worked out.
    ready: bool,
    /// The view last sent, to send nothing when it is the same.
    sent: Option<String>,
}

impl Cache {
    /// Folds a frame in; true when the sidebar may have changed.
    pub(crate) fn fold(&mut self, f: ServerFrame) -> bool {
        match f {
            ServerFrame::Reply { id: SUBSCRIBE, ok: Some(Reply::Workspace(ws)), .. } => {
                self.projects = ws.projects;
                self.threads = ws.threads;
                self.ready = true;
                true
            }
            ServerFrame::Event { change } => match change {
                Change::Project { project } => {
                    match self.projects.iter_mut().find(|p| p.id == project.id) {
                        Some(p) => *p = project,
                        None => self.projects.push(project),
                    }
                    true
                }
                Change::ProjectOrder { ids } => {
                    let rank = |p: &Project| ids.iter().position(|id| *id == p.id).unwrap_or(usize::MAX);
                    self.projects.sort_by_key(rank);
                    true
                }
                Change::Thread { thread } => {
                    match self.threads.iter_mut().find(|t| t.id == thread.id) {
                        Some(t) => *t = *thread,
                        None => self.threads.push(*thread),
                    }
                    true
                }
                Change::ThreadRemoved { thread_id } => {
                    self.threads.retain(|t| t.id != thread_id);
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }

    /// The view as it is now, when it differs from the one sent last.
    pub(crate) fn next(&mut self, now: i64) -> Option<String> {
        if !self.ready {
            return None;
        }
        let v = view::sidebar(&self.projects, &self.threads, now, view::on_page);
        let text = json!({ "type": "view", "view": v }).to_string();
        if self.sent.as_deref() == Some(text.as_str()) {
            return None;
        }
        self.sent = Some(text.clone());
        Some(text)
    }

    async fn push(&mut self, ws: &mut WebSocket) -> Result<(), axum::Error> {
        match self.next(now()) {
            Some(text) => ws.send(Message::Text(text.into())).await,
            None => Ok(()),
        }
    }
}
