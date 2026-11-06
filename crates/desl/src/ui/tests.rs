//! Screen snapshots (SPEC.md "Test strategy"): each screen drawn with ratatui's TestBackend at
//! 80×24 and 200×50 and compared with an approved text file in `crates/desl/snapshots/`.
//! `DESL_BLESS=1 cargo test -p desl snapshot` rewrites them after a deliberate change; the diff
//! is reviewed like code.

use crate::app::{App, Focus, Msg, Overlay};
use crate::conn::Link;
use desl_ir::adapter::{Capabilities, Detection};
use desl_ir::{
    Binding, ChangeKind, Effort, Event, EventKind, GitState, ModelInfo, PendingApproval, PermissionMode, Project, Risk, Status, Thread,
};
use desl_proto::{AgentInfo, Change, ModeNote, Reply, ServerFrame, StreamDelta, Workspace};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::json;
use std::os::unix::net::UnixStream;
use std::path::Path;

fn thread(id: &str, project: &str, name: &str, agent: &str, status: Status, created_at: i64) -> Thread {
    Thread {
        id: id.into(),
        project_id: project.into(),
        name: name.into(),
        cwd: format!("/home/me/{project}"),
        worktree: None,
        parent_id: None,
        fork_point: None,
        active_agent: agent.into(),
        status,
        permission_mode: PermissionMode::Edits,
        created_at,
        updated_at: created_at,
        head: 0,
        bindings: [(
            agent.to_string(),
            Binding {
                agent: agent.into(),
                session_id: Some("s".into()),
                seen_up_to: 0,
                model: Some("m".into()),
                model_choice: None,
                effort_choice: None,
                last_used_at: 0,
                file_offset: 0,
            },
        )]
        .into(),
        queued: vec![],
        pending_approval: None,
        subagents: Vec::new(),
        git: GitState { branch: Some("main".into()), ahead: 2, behind: 0 },
    }
}

fn agents() -> Vec<AgentInfo> {
    ["claude", "codex"]
        .iter()
        .map(|id| AgentInfo {
            id: (*id).to_string(),
            detection: Detection { installed: true, logged_in: Some(true), ..Detection::default() },
            capabilities: Capabilities::default(),
            // Like the real adapters: Claude Code can ask before each call, Codex cannot.
            mode_notes: match *id {
                "codex" => vec![
                    ModeNote { mode: PermissionMode::Ask, note: "reads only, because codex cannot ask".into() },
                    ModeNote { mode: PermissionMode::Edits, note: "edits and commands inside this project only".into() },
                ],
                _ => Vec::new(),
            },
        })
        .collect()
}

fn ev(index: u64, agent: &str, kind: EventKind) -> Event {
    Event { index, thread_id: "t-7f3a".into(), timestamp: 0, agent: agent.into(), kind }
}

/// A client attached to a workspace like the drawing in SPEC.md "Interface design".
fn attached(threads: Vec<Thread>, events: Vec<Event>, partial: Option<&str>) -> (App, UnixStream) {
    let (link, peer) = Link::detached();
    let mut app = App::new("/home/me/api".into(), link);
    app.clock = "12:41".into();
    let projects = vec![
        Project { id: "api".into(), path: "/home/me/api".into(), default_agent: None, created_at: 1 },
        Project { id: "web".into(), path: "/home/me/web".into(), default_agent: None, created_at: 2 },
    ];
    let current = threads[0].clone();
    let ws = Workspace { projects, threads, agents: agents(), view: None, project_id: Some("api".into()), focus: Some(current.id.clone()) };
    // Request 1 is the subscribe the client sends as it starts; request 2 opens the focused thread.
    app.handle(Msg::Daemon(ServerFrame::Reply { id: 1, ok: Some(Reply::Workspace(Box::new(ws))), error: None }));
    app.handle(Msg::Daemon(ServerFrame::Reply {
        id: 2,
        ok: Some(Reply::Thread { thread: Box::new(current), events, partial: partial.map(str::to_string) }),
        error: None,
    }));
    (app, peer)
}

fn conversation() -> Vec<Event> {
    vec![
        ev(1, "claude", EventKind::UserMessage { text: "Add refresh-token rotation to the auth middleware".into(), attachments: vec![] }),
        ev(2, "claude", EventKind::AssistantMessage { text: "I'll start by reading the current middleware.".into(), model: None }),
        ev(3, "claude", EventKind::ToolCall { id: "c1".into(), name: "read".into(), args: json!({ "path": "src/auth/middleware.ts" }) }),
        ev(4, "claude", EventKind::ToolResult { call_id: "c1".into(), output: "1  import\n2  export".into(), exit_code: None, is_error: false, truncated: false, blob: None }),
        ev(5, "claude", EventKind::ToolCall { id: "c2".into(), name: "edit".into(), args: json!({ "path": "src/auth/middleware.ts" }) }),
        ev(6, "claude", EventKind::ToolResult { call_id: "c2".into(), output: "ok".into(), exit_code: None, is_error: false, truncated: false, blob: None }),
        ev(
            7,
            "claude",
            EventKind::FileChange {
                path: "src/auth/middleware.ts".into(),
                diff: Some("@@ -8,3 +8,4 @@\n   const header = req.headers.authorization ?? \"\";\n-  const token = header.slice(7);\n+  const token = header.startsWith(\"Bearer \") ? header.slice(7) : null;\n+  if (!token) return res.status(401).end();\n   next();\n".into()),
                change: ChangeKind::Modify,
                call_id: Some("c2".into()),
            },
        ),
        ev(8, "claude", EventKind::ToolCall { id: "c3".into(), name: "bash".into(), args: json!({ "command": "pnpm test" }) }),
        ev(9, "claude", EventKind::ToolResult { call_id: "c3".into(), output: "84 passed".into(), exit_code: Some(0), is_error: false, truncated: false, blob: None }),
        ev(10, "claude", EventKind::AssistantMessage { text: "Rotation is in. The **refresh token** now rotates on every use; see `rotate()`.\n\nWant me to add the revocation list?".into(), model: None }),
        ev(11, "claude", EventKind::Usage { input_tokens: 10, output_tokens: 20, cost_estimate: Some(0.04) }),
    ]
}

fn workspace_threads(first: Thread) -> Vec<Thread> {
    let mut waiting = thread("t-91c2", "api", "tests", "codex", Status::Waiting, 2);
    waiting.pending_approval = Some(PendingApproval { call_id: "x".into(), tool: "bash".into(), summary: "pnpm test".into() });
    vec![
        first,
        waiting,
        thread("t-04be", "api", "docs", "claude", Status::Idle, 3),
        thread("t-5d10", "web", "landing", "codex", Status::Idle, 4),
    ]
}

fn screen(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| super::draw(f, app)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..height {
        let mut line = String::new();
        for x in 0..width {
            line.push_str(buffer[(x, y)].symbol());
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn snapshot(name: &str, got: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("snapshots").join(format!("{name}.txt"));
    if std::env::var_os("DESL_BLESS").is_some() {
        std::fs::write(&path, got).unwrap();
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("no snapshot at {}; run with DESL_BLESS=1 once", path.display()));
    assert_eq!(got, want, "the screen {name} changed");
}

#[test]
fn snapshot_first_run_is_one_empty_thread_and_a_hint() {
    let (mut app, _peer) = attached(vec![thread("t-7f3a", "api", "", "claude", Status::Idle, 1)], vec![], None);
    snapshot("first-run-80x24", &screen(&mut app, 80, 24));
}

#[test]
fn snapshot_a_conversation_reads_as_a_conversation_with_a_border() {
    let (mut app, _peer) = attached(workspace_threads(thread("t-7f3a", "api", "auth", "claude", Status::Idle, 1)), conversation(), None);
    let small = screen(&mut app, 80, 24);
    snapshot("conversation-80x24", &small);
    snapshot("conversation-200x50", &screen(&mut app, 200, 50));
    // Three things are always true (SPEC.md walkthrough): the agent, the branch and the help key are visible.
    let status = small.lines().rev().nth(1).unwrap();
    assert!(status.contains("claude") && status.contains("main ↑2") && status.contains("ctrl+b ? help"), "{status}");
}

#[test]
fn snapshot_an_approval_is_one_line_above_the_prompt_and_one_glyph_in_the_sidebar() {
    let mut first = thread("t-7f3a", "api", "auth", "claude", Status::Waiting, 1);
    first.pending_approval = Some(PendingApproval { call_id: "c9".into(), tool: "bash".into(), summary: "rm -rf dist/".into() });
    let mut events = conversation();
    events.push(ev(12, "claude", EventKind::ToolCall { id: "c9".into(), name: "bash".into(), args: json!({ "command": "rm -rf dist/" }) }));
    events.push(ev(
        13,
        "claude",
        EventKind::ApprovalRequest { call_id: "c9".into(), tool: "bash".into(), summary: "rm -rf dist/".into(), risk: Risk::Command },
    ));
    let (mut app, _peer) = attached(workspace_threads(first), events, None);
    let got = screen(&mut app, 80, 24);
    snapshot("approval-80x24", &got);
    assert!(got.contains("Allow claude: rm -rf dist/") && got.contains("[y]es [n]o [a]lways"));
    assert!(got.contains("WAITING (2)"));
}

#[test]
fn snapshot_the_help_overlay_lists_the_focused_elements_keys() {
    let (mut app, _peer) = attached(workspace_threads(thread("t-7f3a", "api", "auth", "claude", Status::Idle, 1)), conversation(), None);
    app.overlay = Overlay::Help;
    snapshot("help-prompt-80x24", &screen(&mut app, 80, 24));
    app.focus = Focus::Transcript;
    let transcript = screen(&mut app, 100, 40);
    snapshot("help-transcript-100x40", &transcript);
    assert!(transcript.contains("Open the selected file in $EDITOR"));
}

/// Moment 4: while the user reads further up, new output never moves the lines on screen; it
/// waits behind the `↓ n new` pill.
#[test]
fn output_that_arrives_while_the_user_reads_waits_behind_the_pill() {
    let (mut app, _peer) = attached(workspace_threads(thread("t-7f3a", "api", "auth", "claude", Status::Idle, 1)), conversation(), None);
    // A short pane, scrolled to the top: the user is reading the start of the conversation.
    screen(&mut app, 80, 16);
    let t = app.thread_mut();
    t.follow = false;
    t.scroll = 0;
    let before = screen(&mut app, 80, 16);
    assert!(before.contains("Add refresh-token rotation") && before.contains("new"), "reading the top, with output below:\n{before}");

    let stream = |delta| Msg::Daemon(ServerFrame::Stream { thread_id: "t-7f3a".into(), delta });
    app.handle(stream(StreamDelta::Start));
    app.handle(stream(StreamDelta::Text { text: "One more thing.\n\nAnd another paragraph.".into() }));
    let after = screen(&mut app, 80, 16);

    // Every row is as it was, except the one that carries the pill, whose count grew.
    let pill = |s: &str| s.lines().position(|l| l.contains("↓ ") && l.contains(" new"));
    let row = pill(&before).expect("a pill before");
    assert_eq!(pill(&after), Some(row));
    for (n, (a, b)) in before.lines().zip(after.lines()).enumerate() {
        if n != row {
            assert_eq!(a, b, "row {n} moved");
        }
    }
    assert_ne!(before.lines().nth(row), after.lines().nth(row), "the pill counts the new lines");
}

/// Following at the bottom, streamed text appears as it arrives.
#[test]
fn streamed_text_appears_as_it_arrives() {
    let (mut app, _peer) = attached(workspace_threads(thread("t-7f3a", "api", "auth", "claude", Status::Idle, 1)), conversation(), None);
    let stream = |delta| Msg::Daemon(ServerFrame::Stream { thread_id: "t-7f3a".into(), delta });
    app.handle(stream(StreamDelta::Start));
    app.handle(stream(StreamDelta::Text { text: "On it".into() }));
    assert!(screen(&mut app, 80, 30).contains("On it▍"), "the block in flight ends in a cursor");
    app.handle(Msg::Daemon(ServerFrame::Event {
        change: desl_proto::Change::Appended {
            event: ev(12, "claude", EventKind::AssistantMessage { text: "On it, done.".into(), model: None }),
        },
    }));
    let done = screen(&mut app, 80, 30);
    assert!(done.contains("On it, done.") && !done.contains('▍'), "the final text replaces the stream");
}

fn press(app: &mut App, code: ratatui::crossterm::event::KeyCode) {
    use ratatui::crossterm::event::{Event, KeyEvent, KeyModifiers};
    app.handle(Msg::Term(Event::Key(KeyEvent::new(code, KeyModifiers::NONE))));
}

fn type_line(app: &mut App, text: &str) {
    use ratatui::crossterm::event::KeyCode;
    for c in text.chars() {
        press(app, KeyCode::Char(c));
    }
    press(app, KeyCode::Enter);
}

/// What the client sent last, read from the other end of its socket.
fn last_request(peer: &mut UnixStream) -> serde_json::Value {
    peer.set_read_timeout(Some(std::time::Duration::from_millis(500))).unwrap();
    let mut last = serde_json::Value::Null;
    while let Ok(Some(v)) = desl_proto::read_frame::<_, serde_json::Value>(peer) {
        last = v;
    }
    last
}

#[test]
fn snapshot_switch_alone_lists_the_agents_with_the_current_one_marked() {
    let (mut app, mut peer) = attached(workspace_threads(thread("t-7f3a", "api", "auth", "claude", Status::Idle, 1)), conversation(), None);
    app.agents[1].detection.version = Some("0.165.0".into());
    type_line(&mut app, "/switch");
    let got = screen(&mut app, 80, 24);
    snapshot("switch-picker-80x24", &got);
    assert!(got.contains("✓ current") && got.contains("codex") && got.contains("ready · 0.165.0"), "{got}");
    // The row it starts on is the agent one would switch to; enter asks the daemon what a handoff would carry.
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    let sent = last_request(&mut peer);
    assert_eq!((sent["command"].as_str(), sent["to"].as_str()), (Some("brief"), Some("codex")));
}

#[test]
fn an_agent_that_is_not_ready_says_why_instead_of_switching() {
    let (mut app, _peer) = attached(workspace_threads(thread("t-7f3a", "api", "auth", "claude", Status::Idle, 1)), conversation(), None);
    app.agents[1].detection.installed = false;
    app.agents[1].detection.install_hint = "Install Codex with `npm install -g @openai/codex`, then run `codex login`.".into();
    type_line(&mut app, "/switch");
    assert!(screen(&mut app, 80, 24).contains("not installed"));
    press(&mut app, ratatui::crossterm::event::KeyCode::Down);
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    let got = screen(&mut app, 100, 24);
    assert!(got.contains("codex is not installed. Install Codex with"), "{got}");
}

#[test]
fn snapshot_model_alone_lists_the_agents_models_with_the_current_one_marked() {
    let (mut app, mut peer) = attached(workspace_threads(thread("t-7f3a", "api", "auth", "claude", Status::Idle, 1)), conversation(), None);
    type_line(&mut app, "/model");
    let asked = last_request(&mut peer);
    assert_eq!(asked["command"].as_str(), Some("models"));
    let model = |id: &str, name: &str, description: &str, default: bool| ModelInfo {
        id: id.into(),
        name: name.into(),
        description: description.into(),
        default,
        efforts: ["low", "medium", "high", "xhigh", "max"]
            .iter()
            .map(|l| Effort { id: l.to_string(), description: String::new() })
            .collect(),
        default_effort: None,
    };
    let models = vec![
        model("default", "Default (recommended)", "Opus 5.5 · Best for everyday, complex tasks", true),
        model("opus", "Opus 5.5", "For complex work and everyday tasks", false),
        model("fable", "Fable 5.1", "For your toughest challenges", false),
        model("sonnet", "Sonnet 5.5", "Most efficient for simpler tasks", false),
        model("haiku", "Haiku 5.5", "Fastest for quick answers", false),
    ];
    let id = asked["id"].as_u64().unwrap();
    app.handle(Msg::Daemon(ServerFrame::Reply {
        id,
        ok: Some(Reply::Models { agent: "claude".into(), models, current: Some("sonnet".into()), effort: None }),
        error: None,
    }));
    let got = screen(&mut app, 80, 24);
    snapshot("model-picker-80x24", &got);
    assert!(got.contains("claude models") && got.contains("✓ current · Most efficient for simpler tasks"), "{got}");

    // Up one row and enter: the pick goes to the daemon by id.
    press(&mut app, ratatui::crossterm::event::KeyCode::Up);
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    let sent = last_request(&mut peer);
    assert_eq!((sent["command"].as_str(), sent["model"].as_str()), (Some("set_model"), Some("fable")));
    let id = sent["id"].as_u64().unwrap();
    let picked = model("fable", "Fable 5.1", "For your toughest challenges", false);
    app.handle(Msg::Daemon(ServerFrame::Reply {
        id,
        ok: Some(Reply::ModelSet { agent: "claude".into(), model: Some(picked.clone()), effort: None }),
        error: None,
    }));

    // The model is set, and its effort levels are already on screen: Claude Code does not say
    // which one it uses, so the row that is current is "not choosing".
    let got = screen(&mut app, 80, 24);
    snapshot("effort-picker-80x24", &got);
    assert!(got.contains("claude will use Fable 5.1 from your next prompt."), "{got}");
    assert!(got.contains("Fable 5.1 effort") && got.contains("▸ default") && got.contains("✓ current · claude's own setting"), "{got}");
    for _ in 0..3 {
        press(&mut app, ratatui::crossterm::event::KeyCode::Down);
    }
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    let sent = last_request(&mut peer);
    assert_eq!((sent["command"].as_str(), sent["effort"].as_str()), (Some("set_effort"), Some("high")));
    app.handle(Msg::Daemon(ServerFrame::Reply {
        id: sent["id"].as_u64().unwrap(),
        ok: Some(Reply::ModelSet { agent: "claude".into(), model: Some(picked), effort: Some("high".into()) }),
        error: None,
    }));
    assert!(screen(&mut app, 80, 24).contains("Fable 5.1 at high effort from your next prompt."));
}

#[test]
fn the_effort_list_marks_the_level_in_use_and_its_first_row_goes_back_to_the_agents_own() {
    let (mut app, mut peer) = attached(workspace_threads(thread("t-7f3a", "api", "auth", "codex", Status::Idle, 1)), conversation(), None);
    type_line(&mut app, "/model");
    let id = last_request(&mut peer)["id"].as_u64().unwrap();
    let level = |id: &str, description: &str| Effort { id: id.into(), description: description.into() };
    let astra = ModelInfo {
        id: "gpt-6-astra".into(),
        name: "GPT-6-Astra".into(),
        description: "Frontier coding model".into(),
        default: true,
        efforts: vec![
            level("low", "Fast responses with lighter reasoning"),
            level("medium", "Balances speed and reasoning depth for everyday tasks"),
            level("high", "Greater reasoning depth for complex problems"),
        ],
        default_effort: Some("medium".into()),
    };
    // A model without levels has no second list.
    let plain = ModelInfo { efforts: Vec::new(), default_effort: None, default: false, ..astra.clone() };
    let plain = ModelInfo { id: "plain".into(), name: "Plain".into(), ..plain };
    app.handle(Msg::Daemon(ServerFrame::Reply {
        id,
        ok: Some(Reply::Models {
            agent: "codex".into(),
            models: vec![astra, plain],
            current: Some("gpt-6-astra".into()),
            effort: Some("high".into()),
        }),
        error: None,
    }));
    let models = screen(&mut app, 100, 24);
    assert!(models.contains("✓ current · high effort · Frontier coding model"), "{models}");

    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    assert_eq!(last_request(&mut peer)["command"].as_str(), Some("set_model"));
    let efforts = screen(&mut app, 100, 24);
    assert!(efforts.contains("GPT-6-Astra effort"), "{efforts}");
    assert!(efforts.contains("▸ high") && efforts.contains("✓ current · Greater reasoning depth for complex problems"), "{efforts}");
    assert!(efforts.contains("codex's own setting, now medium"), "{efforts}");

    // Up to the first row: not choosing. It reaches the daemon as no effort at all.
    for _ in 0..3 {
        press(&mut app, ratatui::crossterm::event::KeyCode::Up);
    }
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    let sent = last_request(&mut peer);
    assert_eq!((sent["command"].as_str(), sent["effort"].is_null()), (Some("set_effort"), true));

    // The model without levels: enter picks it and the list closes.
    type_line(&mut app, "/model");
    let id = last_request(&mut peer)["id"].as_u64().unwrap();
    let again = ModelInfo {
        id: "plain".into(),
        name: "Plain".into(),
        description: String::new(),
        default: true,
        efforts: Vec::new(),
        default_effort: None,
    };
    app.handle(Msg::Daemon(ServerFrame::Reply {
        id,
        ok: Some(Reply::Models { agent: "codex".into(), models: vec![again], current: Some("plain".into()), effort: None }),
        error: None,
    }));
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    assert!(!screen(&mut app, 100, 24).contains("effort"), "no levels, no second list");
}

#[test]
fn a_model_and_an_effort_typed_together_are_set_one_after_the_other() {
    let (mut app, mut peer) = attached(workspace_threads(thread("t-7f3a", "api", "auth", "claude", Status::Idle, 1)), conversation(), None);
    type_line(&mut app, "/model sonnet high");
    let first = last_request(&mut peer);
    assert_eq!((first["command"].as_str(), first["model"].as_str()), (Some("set_model"), Some("sonnet")));
    let sonnet = ModelInfo {
        id: "sonnet".into(),
        name: "Sonnet 5.5".into(),
        description: String::new(),
        default: false,
        efforts: Vec::new(),
        default_effort: None,
    };
    app.handle(Msg::Daemon(ServerFrame::Reply {
        id: first["id"].as_u64().unwrap(),
        ok: Some(Reply::ModelSet { agent: "claude".into(), model: Some(sonnet), effort: None }),
        error: None,
    }));
    let second = last_request(&mut peer);
    assert_eq!((second["command"].as_str(), second["effort"].as_str()), (Some("set_effort"), Some("high")));

    // A model the agent does not have stops there: the effort is not applied to the old model.
    type_line(&mut app, "/model sonet high");
    let bad = last_request(&mut peer);
    app.handle(Msg::Daemon(ServerFrame::Reply {
        id: bad["id"].as_u64().unwrap(),
        ok: None,
        error: Some("claude has no model named sonet. It offers default, sonnet.".into()),
    }));
    assert!(screen(&mut app, 100, 24).contains("claude has no model named sonet."));
    assert!(last_request(&mut peer).is_null(), "nothing more was sent");
}

#[test]
fn a_long_list_scrolls_with_the_selection() {
    let (mut app, mut peer) = attached(workspace_threads(thread("t-7f3a", "api", "auth", "claude", Status::Idle, 1)), conversation(), None);
    type_line(&mut app, "/model");
    let id = last_request(&mut peer)["id"].as_u64().unwrap();
    let models: Vec<ModelInfo> = (1..=13)
        .map(|n| ModelInfo {
            id: format!("m{n}"),
            name: format!("Model {n}"),
            description: String::new(),
            default: n == 1,
            efforts: Vec::new(),
            default_effort: None,
        })
        .collect();
    app.handle(Msg::Daemon(ServerFrame::Reply {
        id,
        ok: Some(Reply::Models { agent: "claude".into(), models, current: Some("m1".into()), effort: None }),
        error: None,
    }));
    let top = screen(&mut app, 80, 24);
    assert!(top.contains("Model 10") && !top.contains("Model 11") && top.contains("↓ 3 more"), "{top}");
    for _ in 0..12 {
        press(&mut app, ratatui::crossterm::event::KeyCode::Down);
    }
    let bottom = screen(&mut app, 80, 24);
    assert!(bottom.contains("▸ Model 13") && !bottom.contains("Model 3 ") && !bottom.contains("more"), "{bottom}");
}

#[test]
fn snapshot_permissions_alone_lists_the_modes_with_the_current_one_marked() {
    let first = thread("t-7f3a", "api", "auth", "claude", Status::Idle, 1);
    let (mut app, mut peer) = attached(workspace_threads(first.clone()), conversation(), None);
    type_line(&mut app, "/permissions");
    let got = screen(&mut app, 80, 24);
    snapshot("permissions-picker-80x24", &got);
    assert!(got.contains("▸ edits  ✓ current · file edits are allowed, commands ask"), "{got}");
    assert!(
        got.contains("ask    every edit and command asks first") && got.contains("yolo   everything is allowed, nothing asks"),
        "{got}"
    );
    assert!(!got.contains("worktree"), "a mode that cannot be picked yet is not offered");

    // Down one row and enter: yolo goes to the daemon, and its answer says what it means.
    press(&mut app, ratatui::crossterm::event::KeyCode::Down);
    press(&mut app, ratatui::crossterm::event::KeyCode::Enter);
    let sent = last_request(&mut peer);
    assert_eq!((sent["command"].as_str(), sent["mode"].as_str()), (Some("set_mode"), Some("yolo")));
    app.handle(Msg::Daemon(ServerFrame::Reply {
        id: sent["id"].as_u64().unwrap(),
        ok: Some(Reply::ModeSet { mode: PermissionMode::Yolo, meaning: "everything is allowed, nothing asks".into() }),
        error: None,
    }));
    assert!(screen(&mut app, 80, 24).contains("yolo: everything is allowed, nothing asks."));

    // The thread comes back in yolo: the status bar says so for as long as it lasts.
    let yolo = Thread { permission_mode: PermissionMode::Yolo, ..first };
    app.handle(Msg::Daemon(ServerFrame::Event { change: Change::Thread { thread: Box::new(yolo) } }));
    press(&mut app, ratatui::crossterm::event::KeyCode::Esc);
    let got = screen(&mut app, 80, 24);
    snapshot("yolo-80x24", &got);
    assert!(got.contains("○ idle  claude  ⚠ yolo  main"), "{got}");
    type_line(&mut app, "/permissions");
    assert!(screen(&mut app, 80, 24).contains("▸ yolo   ✓ current"), "the list starts on the mode in use");
}

#[test]
fn the_permissions_list_says_what_each_mode_means_with_an_agent_that_cannot_ask() {
    let (mut app, _peer) = attached(workspace_threads(thread("t-7f3a", "api", "auth", "codex", Status::Idle, 1)), conversation(), None);
    type_line(&mut app, "/permissions");
    let got = screen(&mut app, 80, 24);
    assert!(got.contains("ask    reads only, because codex cannot ask"), "{got}");
    assert!(got.contains("▸ edits  ✓ current · edits and commands inside this project only"), "{got}");
    assert!(got.contains("yolo   everything is allowed, nothing asks"), "{got}");
}

#[test]
fn a_permission_mode_can_be_typed_and_a_wrong_one_is_refused_with_the_choices() {
    let (mut app, mut peer) = attached(workspace_threads(thread("t-7f3a", "api", "auth", "claude", Status::Idle, 1)), conversation(), None);
    type_line(&mut app, "/permissions Ask");
    let sent = last_request(&mut peer);
    assert_eq!((sent["command"].as_str(), sent["mode"].as_str()), (Some("set_mode"), Some("ask")));
    type_line(&mut app, "/permissions never");
    assert!(last_request(&mut peer).is_null(), "nothing was sent");
    assert!(screen(&mut app, 100, 24).contains("There is no permission mode named never. Pick one of ask, edits or yolo."));
}
