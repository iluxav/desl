use super::*;
use crate::bridge::{Cache, HELLO, SUBSCRIBE, read_frame};
use desl_ir::{Project, Status, Thread};
use desl_proto::{Change, Command, PROTOCOL, Reply, Request, ServerFrame, Workspace};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UnixListener};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const KEY: &str = "0123456789abcdef0123456789abcdef";

fn thread(id: &str, status: Status) -> Thread {
    serde_json::from_value(json!({
        "id": id, "project_id": "desl", "name": "Web client", "cwd": "/src/desl", "worktree": null,
        "parent_id": null, "fork_point": null, "active_agent": "claude", "status": status,
        "permission_mode": "yolo", "created_at": 1, "updated_at": 0, "head": 0,
        "bindings": { "claude": { "agent": "claude", "session_id": null, "seen_up_to": 0,
            "model": "claude-opus-5-5", "model_choice": null, "effort_choice": "high", "last_used_at": 0 } },
        "git": { "branch": "main", "ahead": 0, "behind": 0 }
    }))
    .expect("a thread")
}

fn shell(id: &str) -> Thread {
    let mut t = thread(id, Status::Idle);
    t.active_agent = "shell".into();
    t
}

fn workspace() -> Workspace {
    let project = Project { id: "desl".into(), path: "/src/desl".into(), default_agent: None, created_at: 0, git: Default::default() };
    Workspace { projects: vec![project], threads: vec![thread("t-1", Status::Working), shell("t-2")], ..Workspace::default() }
}

fn headers(host: &str, origin: Option<&str>) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(header::HOST, HeaderValue::from_str(host).unwrap());
    if let Some(o) = origin {
        h.insert(header::ORIGIN, HeaderValue::from_str(o).unwrap());
    }
    h
}

#[test]
fn only_its_own_address_and_its_own_page_get_in() {
    assert!(host_ok(&headers("127.0.0.1:7428", None), 7428));
    assert!(host_ok(&headers("localhost:7428", None), 7428));
    assert!(!host_ok(&headers("127.0.0.1:7429", None), 7428), "another port");
    assert!(!host_ok(&headers("evil.example:7428", None), 7428), "a rebinding host name");
    assert!(!host_ok(&HeaderMap::new(), 7428), "no Host at all");
    assert!(origin_ok(&headers("127.0.0.1:7428", Some("http://127.0.0.1:7428")), 7428));
    assert!(origin_ok(&headers("127.0.0.1:7428", Some("http://localhost:7428")), 7428));
    assert!(!origin_ok(&headers("127.0.0.1:7428", Some("https://evil.example")), 7428), "another site's page");
    assert!(!origin_ok(&headers("127.0.0.1:7428", Some("null")), 7428), "a sandboxed frame or a file");
    assert!(!origin_ok(&headers("127.0.0.1:7428", None), 7428), "no Origin: not a browser page");
}

#[test]
fn a_key_is_32_hex_digits_and_new_each_time() {
    let (a, b) = (key().unwrap(), key().unwrap());
    assert_eq!(a.len(), 32);
    assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(a, b);
}

#[test]
fn the_view_follows_the_workspace_and_is_sent_only_when_it_changes() {
    let mut c = Cache::default();
    assert_eq!(c.next(0), None, "nothing before the workspace arrives");
    let ws = ServerFrame::Reply { id: SUBSCRIBE, ok: Some(Reply::Workspace(Box::new(workspace()))), error: None };
    assert!(c.fold(ws));
    let first: Value = serde_json::from_str(&c.next(0).unwrap()).unwrap();
    let threads = &first["view"]["projects"][0]["threads"];
    assert_eq!(threads.as_array().map(Vec::len), Some(1), "the shell stays in the terminal: {first}");
    assert_eq!(threads[0]["word"], "working");
    assert_eq!(threads[0]["header"]["model"], "claude-opus-5-5 · high");
    assert_eq!(threads[0]["header"]["yolo"], true);
    assert_eq!(c.next(0), None, "the same view is not sent twice");
    assert_eq!(c.next(120_000).map(|v| v.contains("\"age\":\"2m\"")), Some(true), "an age moved on");
    let waiting = ServerFrame::Event { change: Change::Thread { thread: Box::new(thread("t-1", Status::Waiting)) } };
    assert!(c.fold(waiting));
    let next: Value = serde_json::from_str(&c.next(120_000).unwrap()).unwrap();
    assert_eq!(next["view"]["waiting"], json!(["t-1"]));
    assert!(c.fold(ServerFrame::Event { change: Change::ThreadRemoved { thread_id: "t-1".into() } }));
    let gone: Value = serde_json::from_str(&c.next(120_000).unwrap()).unwrap();
    assert_eq!(gone["view"]["projects"], json!([]), "a project with nothing to show goes too");
}

/// A daemon that answers `hello` and `subscribe`, hands what else it gets to the test, and
/// sends what the test gives it.
async fn daemon(listener: UnixListener, got: tokio::sync::mpsc::Sender<Request>, mut give: tokio::sync::mpsc::Receiver<ServerFrame>) {
    let Ok((stream, _)) = listener.accept().await else { return };
    let (mut rd, mut wr) = stream.into_split();
    let out = tokio::spawn(async move {
        while let Some(f) = give.recv().await {
            if wr.write_all(&desl_proto::encode(&f).unwrap()).await.is_err() {
                break;
            }
        }
    });
    while let Ok(Some(bytes)) = read_frame(&mut rd).await {
        let r: Request = serde_json::from_slice(&bytes).unwrap();
        let _ = got.send(r).await;
    }
    out.abort();
}

struct Rig {
    port: u16,
    got: tokio::sync::mpsc::Receiver<Request>,
    give: tokio::sync::mpsc::Sender<ServerFrame>,
    _dir: Dir,
}

/// A folder of its own, short enough for a socket's path, removed at the end.
struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn rig() -> Rig {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("desl-web-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let socket = dir.join("d.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let (got_tx, got) = tokio::sync::mpsc::channel(16);
    let (give, give_rx) = tokio::sync::mpsc::channel(16);
    tokio::spawn(daemon(listener, got_tx, give_rx));
    let (tcp, port) = listen(Some(0)).await.unwrap();
    let shared = Shared { token: KEY.into(), port, socket: Arc::new(socket) };
    tokio::spawn(async move { axum::serve(tcp, app(shared)).await });
    Rig { port, got, give, _dir: Dir(dir) }
}

type Client = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

async fn open(port: u16, origin: &str) -> Result<Client, tokio_tungstenite::tungstenite::Error> {
    let mut req = format!("ws://127.0.0.1:{port}/ws").into_client_request().unwrap();
    req.headers_mut().insert("Origin", HeaderValue::from_str(origin).unwrap());
    tokio_tungstenite::connect_async(req).await.map(|(c, _)| c)
}

/// The next text message, as JSON, within five seconds.
async fn next(c: &mut Client) -> Value {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(5), c.next()).await.expect("a message in time").expect("open").expect("no error");
        if let Message::Text(t) = m {
            return serde_json::from_str(t.as_str()).unwrap();
        }
    }
}

async fn request(r: &mut tokio::sync::mpsc::Receiver<Request>) -> Request {
    tokio::time::timeout(Duration::from_secs(5), r.recv()).await.expect("a request in time").expect("the daemon is there")
}

#[tokio::test]
async fn a_tab_with_the_key_sees_the_workspace_and_drives_the_daemon() {
    let mut rig = rig().await;
    let origin = format!("http://127.0.0.1:{}", rig.port);
    let mut c = open(rig.port, &origin).await.unwrap();
    c.send(Message::Text(json!({ "token": KEY }).to_string().into())).await.unwrap();
    // The bridge says hello and subscribes for the tab.
    let hello = request(&mut rig.got).await;
    assert!(matches!(hello.command, Command::Hello { protocol: PROTOCOL, ref client } if client == "web"), "{hello:?}");
    assert_eq!(hello.id, HELLO);
    let sub = request(&mut rig.got).await;
    assert!(matches!(sub.command, Command::Subscribe { cwd: None }), "{sub:?}");
    rig.give.send(ServerFrame::Reply { id: sub.id, ok: Some(Reply::Workspace(Box::new(workspace()))), error: None }).await.unwrap();
    let view = next(&mut c).await;
    assert_eq!(view["type"], "view", "the bridge's own replies stay with it: {view}");
    assert_eq!(view["view"]["projects"][0]["threads"][0]["title"], "Web client");
    // A change the daemon sends reaches the tab as it is, and as a new view.
    rig.give.send(ServerFrame::Event { change: Change::Thread { thread: Box::new(thread("t-1", Status::Waiting)) } }).await.unwrap();
    let raw = next(&mut c).await;
    assert_eq!((raw["type"].as_str(), raw["change"]["what"].as_str()), (Some("event"), Some("thread")), "{raw}");
    let view = next(&mut c).await;
    assert_eq!(view["view"]["waiting"], json!(["t-1"]));
    // The tab's requests reach the daemon, and the answers come back.
    c.send(Message::Text(json!({ "id": 1, "command": "prompt", "thread_id": "t-1", "text": "go on" }).to_string().into())).await.unwrap();
    let prompt = request(&mut rig.got).await;
    assert!(matches!(prompt.command, Command::Prompt { ref text, .. } if text == "go on"));
    rig.give.send(ServerFrame::Reply { id: 1, ok: Some(Reply::Sent { queued: false }), error: None }).await.unwrap();
    let reply = next(&mut c).await;
    assert_eq!((reply["type"].as_str(), reply["id"].as_u64(), reply["ok"]["reply"].as_str()), (Some("reply"), Some(1), Some("sent")));
    // What stops the daemon that holds the agents is not the page's to send.
    c.send(Message::Text(json!({ "id": 2, "command": "shutdown" }).to_string().into())).await.unwrap();
    let refused = next(&mut c).await;
    assert_eq!(refused["id"], 2);
    assert!(refused["error"].as_str().is_some_and(|e| e.starts_with("desl web does not send that")), "{refused}");
    c.send(Message::Text(json!({ "id": 3, "command": "interrupt", "thread_id": "t-1" }).to_string().into())).await.unwrap();
    let interrupt = request(&mut rig.got).await;
    assert!(matches!(interrupt.command, Command::Interrupt { .. }), "the shutdown never reached the daemon: {interrupt:?}");
    // The page asks the bridge for a conversation: the bridge asks the daemon for the events.
    c.send(Message::Text(json!({ "type": "conversation", "id": 4, "thread_id": "t-1" }).to_string().into())).await.unwrap();
    let events = request(&mut rig.got).await;
    assert!(matches!(events.command, Command::Events { ref thread_id } if thread_id == "t-1"), "{events:?}");
    let said = desl_ir::Event {
        index: 1,
        thread_id: "t-1".into(),
        timestamp: 0,
        agent: "claude".into(),
        kind: desl_ir::EventKind::UserMessage { text: "hello".into(), attachments: vec![] },
    };
    rig.give.send(ServerFrame::Reply { id: events.id, ok: Some(Reply::Events { events: vec![said] }), error: None }).await.unwrap();
    let conv = next(&mut c).await;
    assert_eq!((conv["type"].as_str(), conv["id"].as_u64()), (Some("conversation"), Some(4)), "{conv}");
    assert_eq!(conv["items"][0]["text"], "hello");
}

#[test]
fn a_conversation_reaches_the_page_as_the_history_projects_it() {
    use desl_ir::{Event, EventKind};
    let ev = |index, kind| Event { index, thread_id: "t-1".into(), timestamp: 0, agent: "claude".into(), kind };
    let long: String = (1..=50).map(|n| format!("line {n}\n")).collect();
    let items = crate::bridge::items(&[
        ev(1, EventKind::UserMessage { text: "run the tests".into(), attachments: vec![] }),
        ev(2, EventKind::ToolCall { id: "c1".into(), name: "bash".into(), args: json!({ "command": "cargo test" }) }),
        ev(
            3,
            EventKind::ToolResult { call_id: "c1".into(), output: long, exit_code: Some(0), is_error: false, truncated: false, blob: None },
        ),
        ev(4, EventKind::AssistantMessage { text: "All **50** pass.".into(), model: None }),
    ]);
    let kinds: Vec<&str> = items.iter().filter_map(|i| i["kind"].as_str()).collect();
    assert_eq!(kinds, ["user", "tool", "assistant"]);
    assert_eq!(
        (items[1]["verb"].as_str(), items[1]["target"].as_str(), items[1]["ok"].as_bool()),
        (Some("run"), Some("cargo test"), Some(true))
    );
    assert_eq!(items[1]["detail"]["more"], 10, "40 lines are sent, and the count of the rest");
    assert_eq!(items[2]["text"], "All **50** pass.", "the page renders the Markdown itself, as text");
}

#[tokio::test]
async fn a_tab_without_the_key_is_turned_away() {
    let mut rig = rig().await;
    let origin = format!("http://127.0.0.1:{}", rig.port);
    let mut c = open(rig.port, &origin).await.unwrap();
    c.send(Message::Text(json!({ "token": "0123456789abcdef0123456789abcdee" }).to_string().into())).await.unwrap();
    let m = tokio::time::timeout(Duration::from_secs(5), c.next()).await.expect("an answer in time");
    match m {
        Some(Ok(Message::Close(Some(f)))) => assert_eq!(u16::from(f.code), 4401),
        other => panic!("expected a close: {other:?}"),
    }
    assert!(tokio::time::timeout(Duration::from_millis(300), rig.got.recv()).await.is_err(), "the daemon heard nothing");
}

#[tokio::test]
async fn another_sites_page_cannot_open_a_socket() {
    let rig = rig().await;
    let err = open(rig.port, "https://evil.example").await.expect_err("refused");
    match err {
        tokio_tungstenite::tungstenite::Error::Http(r) => assert_eq!(r.status(), StatusCode::FORBIDDEN),
        other => panic!("expected a refusal: {other:?}"),
    }
}

/// The page's own files, asked for under a host name, as a rebinding attack would.
#[tokio::test]
async fn a_rebinding_host_name_gets_nothing() {
    let rig = rig().await;
    let get = |host: String| async move {
        let mut s = TcpStream::connect(("127.0.0.1", rig.port)).await.unwrap();
        s.write_all(format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        out
    };
    let ok = get(format!("127.0.0.1:{}", rig.port)).await;
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    let csp = ok.to_lowercase().lines().find(|l| l.starts_with("content-security-policy:")).unwrap_or_default().to_string();
    assert!(csp.contains("default-src 'self'") && !csp.contains("script-src"), "scripts only from the page itself: {csp}");
    assert!(ok.contains("<title>desl</title>"));
    let evil = get(format!("evil.example:{}", rig.port)).await;
    assert!(evil.starts_with("HTTP/1.1 403"), "{evil}");
    assert!(!evil.contains("<title>"));
}
