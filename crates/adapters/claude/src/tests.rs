//! Replay tests: every fixture of real Claude Code output must produce exactly the IR recorded
//! beside it (`<fixture>.ir.json`). New agent versions add fixtures and never edit old ones.
//! `DESL_BLESS=1 cargo test -p adapter-claude` rewrites the expected files after a deliberate change.

use super::*;
use desl_ir::EventKind;
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn replay(name: &str, cwd: &str) -> Vec<AdapterEvent> {
    let text = std::fs::read_to_string(fixtures().join(name)).unwrap();
    let mut parser = Parser::new(Path::new(cwd));
    text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).flat_map(|v| parser.line(&v)).collect()
}

/// Token deltas are the bulk of a stream and say nothing the final text does not; the expected
/// files keep them as one joined string per block.
fn compact(events: Vec<AdapterEvent>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut streaming: Option<String> = None;
    for ev in events {
        match ev {
            AdapterEvent::TextStart => streaming = Some(String::new()),
            AdapterEvent::TextDelta { text } => streaming.get_or_insert_default().push_str(&text),
            other => {
                if let Some(s) = streaming.take() {
                    out.push(json!({ "type": "streamed_text", "text": s }));
                }
                out.push(serde_json::to_value(&other).unwrap());
            }
        }
    }
    if let Some(s) = streaming {
        out.push(json!({ "type": "streamed_text", "text": s }));
    }
    out
}

fn check(name: &str, cwd: &str) -> Vec<AdapterEvent> {
    let events = replay(name, cwd);
    let got = serde_json::to_string_pretty(&compact(events.clone())).unwrap() + "\n";
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
    let events = check("2.1.312-hello.jsonl", "/home/iluxa/Projects/desl");
    assert_eq!(kinds(&events), ["session", "assistant_message", "usage", "turn_end"]);
    assert!(events.iter().any(|e| matches!(e, AdapterEvent::TextDelta { .. })), "tokens stream");
    assert!(matches!(events.last(), Some(AdapterEvent::TurnEnd { error: None })));
}

#[test]
fn a_read_is_a_tool_call_with_a_relative_path_and_numbered_output() {
    let events = check("2.1.312-tool-read.jsonl", "/home/iluxa/Projects/desl");
    let call = events.iter().find_map(|e| match e {
        AdapterEvent::Event { event: EventKind::ToolCall { name, args, .. } } => Some((name.clone(), args.clone())),
        _ => None,
    });
    assert_eq!(call, Some(("read".to_string(), json!({ "path": "CLAUDE.md" }))));
    let output = events.iter().find_map(|e| match e {
        AdapterEvent::Event { event: EventKind::ToolResult { output, is_error: false, .. } } => Some(output.clone()),
        _ => None,
    });
    assert!(output.unwrap().starts_with("1  # desl"), "numbered from the structured result, no tabs");
}

#[test]
fn an_interrupted_turn_ends_without_an_error() {
    let events = check("2.1.312-interrupt.jsonl", "/home/iluxa/Projects/desl");
    assert!(matches!(events.last(), Some(AdapterEvent::TurnEnd { error: None })));
}

#[test]
fn a_permission_request_becomes_an_approval_request_the_session_can_answer() {
    let name = "2.1.312-approval-can-use-tool.jsonl";
    let events = check(name, "/home/iluxa/Projects/desl");
    let request = events.iter().find_map(|e| match e {
        AdapterEvent::Event { event: EventKind::ApprovalRequest { call_id, tool, summary, risk } } => {
            Some((call_id.clone(), tool.clone(), summary.clone(), *risk))
        }
        _ => None,
    });
    let (call_id, tool, summary, risk) = request.expect("an approval request");
    assert_eq!((tool.as_str(), summary.as_str(), risk), ("bash", "touch /tmp/desl-approval-probe-2", desl_ir::Risk::Command));
    // The request is tied to the tool call it is about.
    assert!(events.iter().any(|e| matches!(e, AdapterEvent::Event { event: EventKind::ToolCall { id, .. } } if *id == call_id)));
}

#[test]
fn without_the_prompt_tool_a_denied_call_is_an_error_result() {
    let events = check("2.1.312-approval-denied-no-prompt-tool.jsonl", "/home/iluxa/Projects/desl");
    assert!(events.iter().any(|e| matches!(e, AdapterEvent::Event { event: EventKind::ToolResult { is_error: true, .. } })));
    assert!(!events.iter().any(|e| matches!(e, AdapterEvent::Event { event: EventKind::ApprovalRequest { .. } })));
}

fn tool_result(structured: Value) -> Value {
    json!({
        "type": "user",
        "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": "raw text" }] },
        "tool_use_result": structured,
    })
}

fn edit_call() -> Value {
    json!({ "type": "assistant", "message": { "content": [{ "type": "tool_use", "id": "t1", "name": "Edit", "input": { "file_path": "/p/src/a.ts" } }] } })
}

fn file_change(events: &[AdapterEvent]) -> Option<(String, Option<String>, desl_ir::ChangeKind)> {
    events.iter().find_map(|e| match e {
        AdapterEvent::Event { event: EventKind::FileChange { path, diff, change, call_id } } => {
            assert_eq!(call_id.as_deref(), Some("t1"));
            Some((path.clone(), diff.clone(), *change))
        }
        _ => None,
    })
}

#[test]
fn an_edit_result_carries_its_patch_as_a_file_change() {
    let mut p = Parser::new(Path::new("/p"));
    p.line(&edit_call());
    let events = p.line(&tool_result(json!({
        "filePath": "/p/src/a.ts",
        "originalFile": "x",
        "structuredPatch": [
            { "oldStart": 2, "oldLines": 2, "newStart": 2, "newLines": 3, "lines": [" a", "+b", " c"] },
            { "oldStart": 9, "oldLines": 1, "newStart": 10, "newLines": 1, "lines": ["-d", "+e"] }
        ]
    })));
    let (path, diff, change) = file_change(&events).expect("a file change");
    assert_eq!(path, "src/a.ts");
    assert_eq!(diff.as_deref(), Some("@@ -2,2 +2,3 @@\n a\n+b\n c\n@@ -9,1 +10,1 @@\n-d\n+e\n"));
    assert_eq!(change, desl_ir::ChangeKind::Modify);
}

#[test]
fn a_new_file_is_a_create_with_every_line_added() {
    let mut p = Parser::new(Path::new("/p"));
    p.line(&edit_call());
    let events = p.line(&tool_result(
        json!({ "type": "create", "filePath": "/p/src/a.ts", "content": "one\ntwo\n", "structuredPatch": [], "originalFile": null }),
    ));
    let (_, diff, change) = file_change(&events).expect("a file change");
    assert_eq!(diff.as_deref(), Some("@@ -0,0 +1,2 @@\n+one\n+two\n"));
    assert_eq!(change, desl_ir::ChangeKind::Create);
}

#[test]
fn cost_is_per_turn_not_the_running_total() {
    let mut p = Parser::new(Path::new("/p"));
    let result = |total: f64| json!({ "type": "result", "subtype": "success", "is_error": false, "total_cost_usd": total, "usage": { "input_tokens": 1, "output_tokens": 1 } });
    let cost = |events: Vec<AdapterEvent>| {
        events.into_iter().find_map(|e| match e {
            AdapterEvent::Event { event: EventKind::Usage { cost_estimate, .. } } => cost_estimate,
            _ => None,
        })
    };
    assert_eq!(cost(p.line(&result(0.25))), Some(0.25));
    assert_eq!(cost(p.line(&result(0.75))), Some(0.5));
}

#[test]
fn a_failed_turn_ends_with_a_sentence() {
    let mut p = Parser::new(Path::new("/p"));
    let events = p.line(&json!({ "type": "result", "subtype": "error_max_turns", "is_error": true, "result": "Reached the turn limit." }));
    assert_eq!(
        events.last(),
        Some(&AdapterEvent::TurnEnd { error: Some("claude stopped: Reached the turn limit. Send your prompt again to retry.".into()) })
    );
}

#[test]
fn the_initialize_reply_lists_the_models_of_claudes_own_picker() {
    let text = std::fs::read_to_string(fixtures().join("2.1.312-initialize-models.json")).unwrap();
    let models = parse::models(&serde_json::from_str::<Value>(&text).unwrap()).expect("a model list");
    assert_eq!(models.len(), 13);
    let first = &models[0];
    assert_eq!((first.id.as_str(), first.name.as_str(), first.default), ("default", "Default (recommended)", true));
    assert_eq!(models.iter().filter(|m| m.default).count(), 1);
    let sonnet = models.iter().find(|m| m.id == "sonnet").unwrap();
    assert_eq!((sonnet.name.as_str(), sonnet.description.as_str()), ("Sonnet 5.5", "Most efficient for simpler tasks"));
    // Any other line of the stream is not a model list.
    assert_eq!(parse::models(&json!({ "type": "system", "subtype": "hook_started" })), None);
}
