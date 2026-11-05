//! Screen snapshots (SPEC.md "Test strategy"): each screen drawn with ratatui's TestBackend at
//! 80×24 and 200×50 and compared with an approved text file in `crates/desl/snapshots/`.
//! `DESL_BLESS=1 cargo test -p desl snapshot` rewrites them after a deliberate change; the diff
//! is reviewed like code.

use crate::app::{App, Focus, Msg, Overlay};
use crate::conn::Link;
use desl_ir::adapter::{Capabilities, Detection};
use desl_ir::{Binding, ChangeKind, Event, EventKind, GitState, PendingApproval, PermissionMode, Project, Risk, Status, Thread};
use desl_proto::{AgentInfo, Reply, ServerFrame, StreamDelta, Workspace};
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
            Binding { agent: agent.into(), session_id: Some("s".into()), seen_up_to: 0, model: Some("m".into()), last_used_at: 0 },
        )]
        .into(),
        queued: vec![],
        pending_approval: None,
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
