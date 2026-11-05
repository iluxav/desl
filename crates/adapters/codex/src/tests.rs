//! Replay tests: every fixture of real Codex output must produce exactly the IR recorded beside
//! it (`<fixture>.ir.json`). New agent versions add fixtures and never edit old ones.
//! `DESL_BLESS=1 cargo test -p adapter-codex` rewrites the expected files after a deliberate change.

use super::*;
use desl_ir::EventKind;
use serde_json::json;
use std::path::Path;

/// The directory the fixtures were captured in.
const CAPTURED_IN: &str = "/tmp/claude-1000/-home-iluxa-Projects-desl/45b03a3f-e369-4390-a009-4cec7ea5e2a4/scratchpad/api";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn check(name: &str) -> Vec<AdapterEvent> {
    let text = std::fs::read_to_string(fixtures().join(name)).unwrap();
    let mut parser = Parser::new(Path::new(CAPTURED_IN), "t", Some("gpt-test".into()));
    let events: Vec<AdapterEvent> =
        text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).flat_map(|v| parser.line(&v)).collect();
    let got = serde_json::to_string_pretty(&events).unwrap() + "\n";
    let path = fixtures().join(format!("{name}.ir.json"));
    if std::env::var_os("DESL_BLESS").is_some() {
        std::fs::write(&path, &got).unwrap();
    }
    let want =
        std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("no expected IR at {}; run with DESL_BLESS=1 once", path.display()));
    assert_eq!(got, want, "the IR for {name} changed");
    events
}

fn kinds(events: &[AdapterEvent]) -> Vec<&'static str> {
    events
        .iter()
        .filter_map(|e| match e {
            AdapterEvent::Event { event } => Some(event.name()),
            AdapterEvent::Session { .. } => Some("session"),
            AdapterEvent::TurnEnd { .. } => Some("turn_end"),
            _ => None,
        })
        .collect()
}

#[test]
fn hello_is_a_session_an_answer_usage_and_the_end_of_the_turn() {
    let events = check("0.165.0-hello.jsonl");
    assert_eq!(kinds(&events), ["session", "assistant_message", "usage", "turn_end"]);
}

#[test]
fn a_resumed_turn_reports_the_same_session() {
    let events = check("0.165.0-resume.jsonl");
    assert!(matches!(events.first(), Some(AdapterEvent::Session { .. })));
}

#[test]
fn a_command_is_one_tool_call_unwrapped_from_its_shell() {
    let events = check("0.165.0-tool-read.jsonl");
    let calls: Vec<Value> = events
        .iter()
        .filter_map(|e| match e {
            AdapterEvent::Event { event: EventKind::ToolCall { name, args, .. } } => Some(json!([name, args])),
            _ => None,
        })
        .collect();
    assert_eq!(calls, vec![json!(["bash", { "command": "cat src/auth/middleware.ts" }])], "started and completed announce one call");
}

#[test]
fn a_file_change_is_an_edit_with_a_relative_path_and_a_file_change_event() {
    let events = check("0.165.0-file-change.jsonl");
    let change = events.iter().find_map(|e| match e {
        AdapterEvent::Event { event: EventKind::FileChange { path, change, call_id, .. } } => {
            Some((path.clone(), *change, call_id.clone()))
        }
        _ => None,
    });
    assert_eq!(change, Some(("src/auth/tokens.ts".to_string(), desl_ir::ChangeKind::Modify, Some("t:item_3:0".to_string()))));
}

#[test]
fn a_blocked_command_leaves_no_approval_request() {
    for name in ["0.165.0-approval-home.jsonl", "0.165.0-approval-readonly.jsonl"] {
        let events = check(name);
        assert!(!events.iter().any(|e| matches!(e, AdapterEvent::Event { event: EventKind::ApprovalRequest { .. } })));
    }
}

fn tool_ids(turn: &str) -> Vec<String> {
    let line = json!({ "type": "item.completed", "item": { "id": "item_1", "type": "command_execution", "command": "bash -lc 'true'", "aggregated_output": "", "exit_code": 0, "status": "completed" } });
    Parser::new(Path::new("/p"), turn, None)
        .line(&line)
        .into_iter()
        .filter_map(|e| match e {
            AdapterEvent::Event { event: EventKind::ToolCall { id, .. } } => Some(id),
            _ => None,
        })
        .collect()
}

#[test]
fn the_same_item_id_in_two_turns_is_two_tool_calls() {
    assert_ne!(tool_ids("a"), tool_ids("b"));
    assert_eq!(tool_ids("a"), vec!["a:item_1".to_string()]);
}

#[test]
fn a_failed_command_leads_with_its_exit_code() {
    let line = json!({ "type": "item.completed", "item": { "id": "item_1", "type": "command_execution", "command": "bash -lc 'ls /nope'", "aggregated_output": "ls: cannot access\n", "exit_code": 2, "status": "failed" } });
    let result = Parser::new(Path::new("/p"), "t", None).line(&line).into_iter().find_map(|e| match e {
        AdapterEvent::Event { event: EventKind::ToolResult { is_error, output, exit_code, .. } } => Some((is_error, output, exit_code)),
        _ => None,
    });
    assert_eq!(result, Some((true, "exit 2\nls: cannot access\n".to_string(), Some(2))));
}

#[test]
fn a_file_change_is_diffed_against_the_file_as_it_was_when_the_change_started() {
    let dir = std::env::temp_dir().join(format!("desl-codex-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a.txt");
    std::fs::write(&path, "one\ntwo\n").unwrap();
    let item = |status: &str| {
        json!({ "type": if status == "completed" { "item.completed" } else { "item.started" },
            "item": { "id": "item_3", "type": "file_change", "changes": [{ "path": path.to_string_lossy(), "kind": "update" }], "status": status } })
    };
    let mut parser = Parser::new(&dir, "t", None);
    parser.line(&item("in_progress"));
    std::fs::write(&path, "one\n2\n").unwrap();
    let change = parser.line(&item("completed")).into_iter().find_map(|e| match e {
        AdapterEvent::Event { event: EventKind::FileChange { path, diff, .. } } => Some((path, diff)),
        _ => None,
    });
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(change, Some(("a.txt".to_string(), Some("@@ -1,2 +1,2 @@\n one\n-two\n+2\n".to_string()))));
}

#[test]
fn a_failed_turn_ends_with_a_sentence() {
    let events =
        Parser::new(Path::new("/p"), "t", None).line(&json!({ "type": "turn.failed", "error": { "message": "stream disconnected" } }));
    assert_eq!(
        events,
        vec![AdapterEvent::TurnEnd { error: Some("codex stopped: stream disconnected. Send your prompt again to retry.".into()) }]
    );
}
