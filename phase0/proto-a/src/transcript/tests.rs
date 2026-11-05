//! Replay tests over the real session files in `fixtures/transcripts/`.

use super::{TranscriptEvent, claude, codex};
use crate::agent::AgentEvent;
use std::path::PathBuf;

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures/transcripts").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn claude_session_replays_to_user_text_and_turn_end() {
    let mut st = claude::State::default();
    let mut users = 0;
    let mut texts = 0;
    let mut ends = 0;
    let mut started = false;
    for line in fixture("claude-2.1.312-session-a47b5d37.jsonl").lines() {
        for ev in claude::parse_line(&mut st, line) {
            match ev {
                TranscriptEvent::User(t) => {
                    users += 1;
                    assert!(!t.starts_with('<'));
                }
                TranscriptEvent::Agent(AgentEvent::Started { session_id, .. }) => {
                    started = true;
                    assert_eq!(session_id, "a47b5d37-fa5c-4255-9fe4-d4a0b9a29bb3");
                }
                TranscriptEvent::Agent(AgentEvent::TextFinal(_)) => texts += 1,
                TranscriptEvent::Agent(AgentEvent::TurnDone { .. }) => ends += 1,
                _ => {}
            }
        }
    }
    assert!(started);
    assert_eq!(users, 1, "one typed prompt");
    assert!(texts >= 1);
    assert_eq!(ends, 1, "one end_turn, counted once across the message's lines");
}

#[test]
fn codex_rollout_replays_tools_and_turn() {
    let mut st = codex::State::default();
    let mut users = Vec::new();
    let mut tools = Vec::new();
    let mut results = 0;
    let mut turn_started = 0;
    let mut ends = 0;
    for line in fixture("codex-0.165.0-rollout-file-change.jsonl").lines() {
        for ev in codex::parse_line(&mut st, line) {
            match ev {
                TranscriptEvent::User(t) => users.push(t),
                TranscriptEvent::TurnStarted => turn_started += 1,
                TranscriptEvent::Agent(AgentEvent::ToolUse { name, input, .. }) => tools.push((name, input)),
                TranscriptEvent::Agent(AgentEvent::ToolResult { .. }) => results += 1,
                TranscriptEvent::Agent(AgentEvent::TurnDone { .. }) => ends += 1,
                _ => {}
            }
        }
    }
    assert_eq!(users.len(), 1, "environment context is not a user prompt: {users:?}");
    assert!(users[0].starts_with("Add a one-line comment"));
    assert_eq!(turn_started, 1);
    assert_eq!(ends, 1);
    assert!(tools.iter().any(|(n, i)| n == "Bash" && i["command"].as_str().is_some_and(|c| c.starts_with("pwd; rg"))), "{tools:?}");
    assert!(tools.iter().any(|(n, i)| n == "Edit" && i["file_path"].as_str().is_some_and(|f| f.ends_with("src/auth/tokens.ts"))), "{tools:?}");
    assert_eq!(results, 2);
}

/// A user line; `answered` adds the `turnOrigin` mark Claude Code puts on the lines it will answer.
fn claude_user(content: &str, meta: bool, answered: bool) -> String {
    let mut v = serde_json::json!({
        "type": "user", "isSidechain": false, "isMeta": meta, "sessionId": "00000000-0000-4000-8000-000000000001",
        "version": "2.1.312", "message": {"role": "user", "content": content}
    });
    if answered {
        v["turnOrigin"] = serde_json::json!("human");
    }
    v.to_string()
}

fn claude_assistant(id: &str, block: serde_json::Value, stop: &str) -> String {
    serde_json::json!({
        "type": "assistant", "isSidechain": false, "sessionId": "00000000-0000-4000-8000-000000000001",
        "message": {"id": id, "role": "assistant", "model": "claude-test", "content": [block], "stop_reason": stop}
    })
    .to_string()
}

/// Replays lines and reports the typed prompts and whether a turn is left open at the end.
fn claude_replay(lines: &[String]) -> (Vec<String>, bool) {
    let mut st = claude::State::default();
    let mut users = Vec::new();
    let mut open = false;
    for line in lines {
        for ev in claude::parse_line(&mut st, line) {
            match ev {
                TranscriptEvent::User(t) => users.push(t),
                TranscriptEvent::TurnStarted => open = true,
                TranscriptEvent::Agent(AgentEvent::TurnDone { .. }) => open = false,
                _ => {}
            }
        }
    }
    (users, open)
}

/// Claude Code 2.1.312 records its own commands (`/model`, `/cost`, `!ls`) as tagged user lines with no turn
/// after them. Counting one as a prompt left the thread "working" forever and made `/switch` refuse.
/// The line shapes are copied from a real session; the values are made up.
#[test]
fn claude_local_command_is_not_a_prompt_and_opens_no_turn() {
    let lines = vec![
        claude_user("what is the directory you are in?", false, true),
        claude_assistant("msg_1", serde_json::json!({"type": "text", "text": "`/work/api`"}), "end_turn"),
        claude_user("<local-command-caveat>The command below was run directly in Claude Code, not sent to you as a request.</local-command-caveat>", true, false),
        claude_user("<command-name>/model</command-name>\n            <command-message>model</command-message>\n            <command-args></command-args>", false, false),
        claude_user("<local-command-stdout>Set model to `Opus` and saved as your default for new sessions</local-command-stdout>", false, false),
    ];
    let (users, open) = claude_replay(&lines);
    assert_eq!(users, vec!["what is the directory you are in?".to_string()]);
    assert!(!open, "the /model lines after the answer must not leave a turn open");
}

fn claude_user_blocks(blocks: serde_json::Value) -> String {
    serde_json::json!({
        "type": "user", "isSidechain": false, "sessionId": "00000000-0000-4000-8000-000000000001",
        "message": {"role": "user", "content": blocks}
    })
    .to_string()
}

/// The order of what a replay yields, one word per event that matters to a thread's status.
fn claude_kinds(lines: &[String]) -> Vec<&'static str> {
    let mut st = claude::State::default();
    let mut kinds = Vec::new();
    for line in lines {
        for ev in claude::parse_line(&mut st, line) {
            kinds.push(match ev {
                TranscriptEvent::User(_) => "prompt",
                TranscriptEvent::TurnStarted => "start",
                TranscriptEvent::Agent(AgentEvent::TextFinal(_)) => "text",
                TranscriptEvent::Agent(AgentEvent::ToolUse { .. }) => "tool",
                TranscriptEvent::Agent(AgentEvent::ToolResult { .. }) => "result",
                TranscriptEvent::Agent(AgentEvent::TurnDone { .. }) => "done",
                _ => continue,
            });
        }
    }
    kinds
}

/// Bash mode (`!echo hi`) is recorded as tagged user lines, and Claude then answers the output on its own.
/// The answer's first line (its thinking) already says `end_turn`, its text comes after, and the
/// `turn_duration` line closes the turn. Shapes from a real 2.1.312 session.
#[test]
fn claude_turn_without_a_prompt_opens_and_ends_after_its_last_line() {
    let lines = vec![
        claude_user("<bash-input>echo hi</bash-input>", false, false),
        claude_user("<bash-stdout>hi</bash-stdout><bash-stderr></bash-stderr>", false, true),
        claude_assistant("msg_2", serde_json::json!({"type": "thinking", "thinking": ""}), "end_turn"),
        claude_assistant("msg_2", serde_json::json!({"type": "text", "text": "The shell works."}), "end_turn"),
        r#"{"type":"system","subtype":"turn_duration","durationMs":3000}"#.to_string(),
    ];
    assert_eq!(claude_kinds(&lines), vec!["start", "done", "text", "done"]);
}

/// Esc during a tool run: the tool is rejected and an interruption marker follows. No `end_turn` and no
/// `turn_duration` are written, so the marker itself must end the turn.
#[test]
fn claude_interrupt_ends_the_turn() {
    let lines = vec![
        claude_user("Run sleep 22 in the foreground.", false, true),
        claude_assistant("msg_3", serde_json::json!({"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "sleep 22"}}), "tool_use"),
        claude_user_blocks(serde_json::json!([{"type": "tool_result", "tool_use_id": "toolu_1", "is_error": true, "content": "The user doesn't want to proceed with this tool use."}])),
        claude_user_blocks(serde_json::json!([{"type": "text", "text": "[Request interrupted by user for tool use]"}])),
    ];
    assert_eq!(claude_kinds(&lines), vec!["start", "prompt", "tool", "result", "done"]);
}
