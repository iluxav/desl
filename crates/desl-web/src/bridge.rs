//! One browser tab joined to the daemon (web-spec.md "Architecture"). The tab proves it holds
//! the key, then gets a connection of its own to the daemon: every frame the daemon sends goes
//! to the tab as its JSON, and every request the tab sends goes to the daemon, so the page
//! speaks the protocol in `docs/protocol.md`. The bridge adds two messages of its own: `view`,
//! the sidebar as `desl_proto::view` words it for the TUI, sent whenever it changes, and
//! `conversation`, a thread's conversation as `desl_proto::conversation` projects it for the
//! TUI's history, sent when the page asks; so the two clients cannot show different things. The
//! page gets the last part of a long conversation and asks for earlier parts as it scrolls up;
//! the bridge reads the thread's log from the daemon once and keeps it from the changes after.
//! It keeps nothing that matters: closing it, or the tab, loses nothing.

use crate::Shared;
use axum::extract::ws::{CloseFrame, Message, WebSocket};
use desl_ir::{Event, Project, Thread};
use desl_proto::conversation::{Item, conversation};
use desl_proto::{Change, Command, PROTOCOL, Reply, Request, ServerFrame, view};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::UnixStream;

/// Request ids the bridge uses for itself. The page's stay below them, and replies to these go
/// no further than the bridge. Below 2^53, so a page's numbers never reach them.
const OWN: u64 = 1 << 52;
pub(crate) const HELLO: u64 = OWN;
pub(crate) const SUBSCRIBE: u64 = OWN + 1;
/// The bridge's own requests for the page start here.
const ASKED: u64 = OWN + 16;

/// Items of a conversation the page gets at first, and more with each step up.
const WINDOW: usize = 300;

/// Lines of a tool's output, and of an edit's diff, sent before `more` says how many were left.
const DETAIL_LINES: usize = 40;
const DIFF_LINES: usize = 400;

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
    // Which page this is: one loaded before an install reloads itself. The prefix key and the
    // shell are the TUI's, for a page used without it.
    let ready = json!({ "type": "bridge", "state": "ready", "build": crate::build(), "prefix": s.host.prefix, "shell": s.host.shell });
    if say(&mut ws, ready).await.is_err() {
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
    // The thread whose log the bridge asked the daemon for, by the id it asked with.
    let mut asked: HashMap<u64, String> = HashMap::new();
    // The log of the thread the page reads, and what the page asked of it meanwhile.
    let mut log: Option<Log> = None;
    let mut next = ASKED;
    // The directories `prefix o` lists, gathered off the runtime: each answer for the page.
    let (dirs_tx, mut dirs_done) = tokio::sync::mpsc::channel::<Value>(4);
    loop {
        tokio::select! {
            Some(answer) = dirs_done.recv() => {
                if say(&mut ws, answer).await.is_err() {
                    break;
                }
            }
            frame = frames.recv() => {
                let Some(bytes) = frame else {
                    // The daemon stopped or handed over: the page connects again.
                    let _ = say(&mut ws, json!({ "type": "bridge", "state": "daemon_gone" })).await;
                    break;
                };
                let parsed = serde_json::from_slice::<ServerFrame>(&bytes).ok();
                if let Some(ServerFrame::Reply { id, ok, error }) = &parsed
                    && let Some(thread_id) = asked.remove(id)
                {
                    let Some(l) = log.as_mut().filter(|l| l.thread_id == thread_id) else { continue };
                    let answers: Vec<Value> = match (ok, error) {
                        (Some(Reply::Events { events }), _) => {
                            l.load(events.clone());
                            std::mem::take(&mut l.waiting).into_iter().map(|(page_id, ask)| l.answer(page_id, ask)).collect()
                        }
                        (_, Some(e)) => {
                            let waiting = std::mem::take(&mut l.waiting);
                            log = None;
                            waiting
                                .into_iter()
                                .map(|(page_id, _)| json!({ "type": "conversation", "id": page_id, "thread_id": thread_id, "error": e }))
                                .collect()
                        }
                        _ => continue,
                    };
                    for a in answers {
                        if say(&mut ws, a).await.is_err() {
                            break;
                        }
                    }
                    continue;
                }
                if let Some(ServerFrame::Event { change: Change::Appended { event } }) = &parsed
                    && let Some(l) = log.as_mut().filter(|l| l.thread_id == event.thread_id)
                {
                    l.append(event.clone());
                }
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
                // The page asks the bridge itself for a thread's conversation.
                Some(Ok(Message::Text(t))) if dirs_asked(t.as_str()).is_some() => {
                    let Some(page_id) = dirs_asked(t.as_str()) else { continue };
                    let host = s.host.clone();
                    let known = cache.paths();
                    let tx = dirs_tx.clone();
                    tokio::spawn(async move {
                        let found = tokio::task::spawn_blocking(move || (host.dirs)(&known)).await.unwrap_or_default();
                        let dirs: Vec<Value> =
                            found.into_iter().map(|(path, shown, note)| json!({ "path": path, "shown": shown, "note": note })).collect();
                        let _ = tx.send(json!({ "type": "dirs", "id": page_id, "dirs": dirs })).await;
                    });
                }
                Some(Ok(Message::Text(t))) if conversation_asked(t.as_str()).is_some() => {
                    let Some((page_id, thread_id, ask)) = conversation_asked(t.as_str()) else { continue };
                    match log.as_mut().filter(|l| l.thread_id == thread_id) {
                        Some(l) if l.loaded => {
                            let answer = l.answer(page_id, ask);
                            if say(&mut ws, answer).await.is_err() {
                                break;
                            }
                        }
                        // Asked already: answered when the log comes.
                        Some(l) => l.waiting.push((page_id, ask)),
                        None => {
                            log = Some(Log::new(thread_id.clone(), (page_id, ask)));
                            asked.insert(next, thread_id.clone());
                            if send(&mut wr, &Request { id: next, command: Command::Events { thread_id } }).await.is_err() {
                                break;
                            }
                            next += 1;
                        }
                    }
                }
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

/// `{"type": "dirs", "id": …}`: the page's id, asking for the directories `prefix o` lists.
fn dirs_asked(text: &str) -> Option<Value> {
    let v: Value = serde_json::from_str(text).ok()?;
    (v.get("type").and_then(Value::as_str) == Some("dirs")).then(|| v.get("id").cloned().unwrap_or(Value::Null))
}

/// `{"type": "conversation", "id": …, "thread_id": …, "from": …, "event": …}`: the page's id, the
/// thread and how much of it.
fn conversation_asked(text: &str) -> Option<(Value, String, Ask)> {
    let v: Value = serde_json::from_str(text).ok()?;
    if v.get("type").and_then(Value::as_str) != Some("conversation") {
        return None;
    }
    let ask = Ask { from: v.get("from").and_then(Value::as_u64).map(|n| n as usize), event: v.get("event").and_then(Value::as_u64) };
    Some((v.get("id").cloned().unwrap_or(Value::Null), v.get("thread_id")?.as_str()?.to_string(), ask))
}

/// How much of a conversation the page asks for: the last [`WINDOW`] items at least, and from
/// item `from` on, or from the item that shows event `event`, when it says so.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Ask {
    pub(crate) from: Option<usize>,
    pub(crate) event: Option<u64>,
}

/// The log of the thread the page reads: read from the daemon once, then kept from the events
/// appended to it, so a page reading a long thread as it grows costs no second copy of it.
struct Log {
    thread_id: String,
    events: Vec<Event>,
    /// The daemon's answer came: `events` is the whole log.
    loaded: bool,
    /// Events appended before the daemon's answer came, which it may hold or not.
    early: Vec<Event>,
    /// The page's requests waiting for the log.
    waiting: Vec<(Value, Ask)>,
}

impl Log {
    fn new(thread_id: String, first: (Value, Ask)) -> Log {
        Log { thread_id, events: Vec::new(), loaded: false, early: Vec::new(), waiting: vec![first] }
    }

    fn load(&mut self, events: Vec<Event>) {
        self.events = events;
        self.loaded = true;
        for e in std::mem::take(&mut self.early) {
            self.append(e);
        }
    }

    fn append(&mut self, e: Event) {
        if !self.loaded {
            self.early.push(e);
        } else if self.events.last().is_none_or(|l| e.index > l.index) {
            self.events.push(e);
        }
    }

    fn answer(&self, page_id: Value, ask: Ask) -> Value {
        let (start, total, items) = window(&self.events, ask);
        json!({
            "type": "conversation",
            "id": page_id,
            "thread_id": self.thread_id,
            "items": items,
            "start": start,
            "total": total,
            "highlights": desl_proto::highlights::highlights(&self.events),
        })
    }
}

/// The part of `events`' conversation that `ask` asks for: the index of its first item, the
/// count of all items, and the items. It begins at a prompt when one is near, so a turn is not
/// cut in two.
pub(crate) fn window(events: &[Event], ask: Ask) -> (usize, usize, Vec<Value>) {
    let (thread, prompts) = conversation(events, true);
    let total = thread.items.len();
    let mut start = total.saturating_sub(WINDOW);
    if let Some(from) = ask.from {
        start = start.min(from);
    }
    if let Some(event) = ask.event {
        start = start.min(thread.origins.iter().position(|&o| o >= event).unwrap_or(total));
    }
    if let Some(prompt) = (start.saturating_sub(WINDOW)..=start.min(total.saturating_sub(1)))
        .rev()
        .find(|&i| matches!(thread.items.get(i), Some(Item::User { .. })))
    {
        start = prompt;
    }
    (start, total, project_items(&thread.items[start..], &thread.origins[start..], &prompts, start))
}

/// The first `lines` lines of `text`, and how many more there were.
fn capped(text: &str, lines: usize) -> Value {
    let all: Vec<&str> = text.lines().collect();
    let shown = all.len().min(lines);
    json!({ "text": all[..shown].join("\n"), "more": all.len() - shown })
}

/// A thread's whole conversation as the page draws it: the items of the TUI's history, each a
/// JSON object with its `kind` and the `event` behind it. A prompt says when it was sent.
#[cfg(test)]
pub(crate) fn items(events: &[Event]) -> Vec<Value> {
    window(events, Ask { from: Some(0), event: None }).2
}

/// `items`, which begin at item `start` of the conversation, with `origins`, their events, and
/// `prompts`, when each prompt of the whole conversation was sent, by its item.
fn project_items(items: &[Item], origins: &[u64], prompts: &[(usize, i64)], start: usize) -> Vec<Value> {
    let sent: HashMap<usize, i64> = prompts.iter().copied().collect();
    let mut out: Vec<Value> = items
        .iter()
        .enumerate()
        .map(|(i, item)| (start + i, item))
        .map(|(i, item)| match item {
            Item::User { text } => json!({ "kind": "user", "text": text, "at": sent.get(&i) }),
            Item::Assistant { text, .. } => json!({ "kind": "assistant", "text": text }),
            Item::Tool { verb, target, outcome, decision, detail, diff, .. } => json!({
                "kind": "tool",
                "verb": verb,
                "target": target,
                "ok": outcome.as_ref().map(|o| o.ok),
                "summary": outcome.as_ref().map(|o| o.summary.as_str()),
                "decision": decision,
                "detail": capped(detail, DETAIL_LINES),
                "diff": diff.as_deref().map(|d| capped(d, DIFF_LINES)),
            }),
            Item::Divider(label) => json!({ "kind": "rule", "text": label }),
            Item::Brief { label, text, tokens, .. } => json!({ "kind": "brief", "label": label, "text": text, "tokens": tokens }),
            Item::Note(text) => json!({ "kind": "note", "text": text }),
            Item::Error(text) => json!({ "kind": "error", "text": text }),
            // The page asks for the form itself (`form_get`) to draw its card.
            Item::Form { form_id, title, by, .. } => json!({ "kind": "form", "form_id": form_id, "title": title, "by": by }),
        })
        .collect();
    for (item, event) in out.iter_mut().zip(origins) {
        item["event"] = json!(event);
    }
    out
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
    /// The user's file manager, when the daemon found one: the new-thread menu offers it.
    file_manager: Option<String>,
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
                self.file_manager = ws.file_manager;
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
                Change::ProjectRemoved { project_id } => {
                    self.projects.retain(|p| p.id != project_id);
                    self.threads.retain(|t| t.project_id != project_id);
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
        let text = json!({ "type": "view", "view": v, "file_manager": self.file_manager }).to_string();
        if self.sent.as_deref() == Some(text.as_str()) {
            return None;
        }
        self.sent = Some(text.clone());
        Some(text)
    }

    /// Every project's folder.
    fn paths(&self) -> Vec<String> {
        self.projects.iter().map(|p| p.path.clone()).collect()
    }

    async fn push(&mut self, ws: &mut WebSocket) -> Result<(), axum::Error> {
        match self.next(now()) {
            Some(text) => ws.send(Message::Text(text.into())).await,
            None => Ok(()),
        }
    }
}
