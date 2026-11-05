//! Claude Code session files: `~/.claude/projects/<cwd with / as ->/<session-id>.jsonl`.
//! Verified against Claude Code 2.1.312; fixture `fixtures/transcripts/claude-2.1.312-*.jsonl`.

use super::{TranscriptEvent, home, newest_since};
use crate::agent::AgentEvent;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub fn project_dir(cwd: &Path) -> PathBuf {
    let slug: String = cwd.to_string_lossy().chars().map(|c| if c == '/' { '-' } else { c }).collect();
    home().join(".claude").join("projects").join(slug)
}

/// The session file an agent started after `since` is writing, if it exists yet.
pub fn find_session(cwd: &Path, since: SystemTime, taken: &[PathBuf]) -> Option<PathBuf> {
    newest_since(&project_dir(cwd), since, taken, |p| p.extension().is_some_and(|e| e == "jsonl"))
}

/// A fresh version-4 UUID for `claude --session-id` (verified against 2.1.312; see docs/adapters.md).
/// Choosing the id up front names the session file before it exists, so another Claude Code working in
/// the same directory can never be mistaken for this one.
pub fn new_session_id() -> String {
    use std::io::Read;
    let mut b = [0u8; 16];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).is_err() {
        let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        b = (nanos ^ ((std::process::id() as u128) << 96)).to_le_bytes();
    }
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

/// Arguments that start the interactive interface on a session id of our choosing.
pub fn session_args(session_id: &str) -> Vec<String> {
    vec!["--session-id".to_string(), session_id.to_string()]
}

/// Where Claude Code writes that session.
pub fn session_path(cwd: &Path, session_id: &str) -> PathBuf {
    project_dir(cwd).join(format!("{session_id}.jsonl"))
}

#[derive(Default)]
pub struct State {
    started: bool,
    ended_message_ids: Vec<String>,
    turn_open: bool,
    /// Text or a tool call arrived after its message had already ended the turn.
    trailing: bool,
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// What the user typed, as opposed to what Claude Code wrote on their behalf. Its own commands (`/model`,
/// `/cost`, `!ls`) and their output are recorded as user lines that start with a tag; no turn follows them.
fn is_prompt(text: &str) -> bool {
    let t = text.trim_start();
    !t.is_empty() && !t.starts_with('<')
}

fn open_turn(state: &mut State, out: &mut Vec<TranscriptEvent>) {
    if !state.turn_open {
        state.turn_open = true;
        out.push(TranscriptEvent::TurnStarted);
    }
}

fn close_turn(state: &mut State, out: &mut Vec<TranscriptEvent>, subtype: &str) {
    if state.turn_open {
        state.turn_open = false;
        out.push(TranscriptEvent::Agent(AgentEvent::TurnDone {
            is_error: false,
            cost_usd: 0.0,
            duration_ms: 0,
            result: None,
            subtype: subtype.into(),
        }));
    }
}

pub fn parse_line(state: &mut State, line: &str) -> Vec<TranscriptEvent> {
    let mut out = Vec::new();
    let Ok(v) = serde_json::from_str::<Value>(line) else { return out };
    if v.get("isSidechain").and_then(Value::as_bool).unwrap_or(false) {
        return out;
    }
    let msg = v.get("message").cloned().unwrap_or(Value::Null);
    match str_of(&v, "type").as_str() {
        "user" => {
            if !state.started {
                let sid = str_of(&v, "sessionId");
                if !sid.is_empty() {
                    state.started = true;
                    out.push(TranscriptEvent::Agent(AgentEvent::Started { session_id: sid, model: String::new() }));
                }
            }
            // Bookkeeping lines (a local command's caveat, a skill's expanded prompt) are marked `isMeta`.
            let meta = v.get("isMeta").and_then(Value::as_bool).unwrap_or(false);
            // Claude Code marks the user lines it will answer with `turnOrigin`: a typed prompt, and also the
            // output of bash mode (`!ls`). Its local commands (`/model`) carry no such mark.
            let answered = !str_of(&v, "turnOrigin").is_empty();
            match msg.get("content") {
                Some(Value::String(text)) => {
                    if text.starts_with("[Request interrupted") {
                        close_turn(state, &mut out, "interrupted");
                    } else if !meta && is_prompt(text) {
                        // Announced on every prompt, open turn or not: the host may have given up on a silent one.
                        state.turn_open = true;
                        out.push(TranscriptEvent::TurnStarted);
                        out.push(TranscriptEvent::User(text.clone()));
                    } else if answered {
                        open_turn(state, &mut out);
                    }
                }
                Some(Value::Array(blocks)) => {
                    for b in blocks {
                        match str_of(b, "type").as_str() {
                            "tool_result" => out.push(TranscriptEvent::Agent(AgentEvent::ToolResult {
                                tool_use_id: str_of(b, "tool_use_id"),
                                is_error: b.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                                content: text_of(&b["content"]),
                                diff: None,
                            })),
                            "text" => {
                                let text = str_of(b, "text");
                                if text.starts_with("[Request interrupted") {
                                    close_turn(state, &mut out, "interrupted");
                                } else if !meta && is_prompt(&text) {
                                    state.turn_open = true;
                                    out.push(TranscriptEvent::TurnStarted);
                                    out.push(TranscriptEvent::User(text));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        "assistant" => {
            let model = str_of(&msg, "model");
            if !state.started {
                let sid = str_of(&v, "sessionId");
                state.started = true;
                out.push(TranscriptEvent::Agent(AgentEvent::Started { session_id: sid, model: model.clone() }));
            }
            // A turn no prompt announced: a native command such as `/init` puts the agent to work on its own.
            let id = str_of(&msg, "id");
            if !state.ended_message_ids.contains(&id) {
                open_turn(state, &mut out);
            }
            if let Some(blocks) = msg.get("content").and_then(Value::as_array) {
                for b in blocks {
                    match str_of(b, "type").as_str() {
                        "text" => {
                            let text = str_of(b, "text");
                            if !text.is_empty() {
                                state.trailing = !state.turn_open;
                                out.push(TranscriptEvent::Agent(AgentEvent::TextStart));
                                out.push(TranscriptEvent::Agent(AgentEvent::TextFinal(text)));
                            }
                        }
                        "tool_use" => out.push(TranscriptEvent::Agent(AgentEvent::ToolUse {
                            id: str_of(b, "id"),
                            name: str_of(b, "name"),
                            input: b.get("input").cloned().unwrap_or(Value::Null),
                        })),
                        _ => {}
                    }
                }
            }
            if str_of(&msg, "stop_reason") == "end_turn" && !state.ended_message_ids.contains(&id) {
                state.ended_message_ids.push(id);
                // Counted even if the turn was never seen opening, so the thread always lands on idle.
                state.turn_open = true;
                close_turn(state, &mut out, "success");
            }
        }
        // Written once after every finished turn, after the last line of its last message. A message's first
        // line (its thinking) can already say `end_turn` while its text is still to come; this line then ends
        // the turn a second time, after that text. It also ends a turn whose last message never said `end_turn`.
        "system" if str_of(&v, "subtype") == "turn_duration" => {
            if state.turn_open || state.trailing {
                state.turn_open = true;
                state.trailing = false;
                close_turn(state, &mut out, "success");
            }
        }
        _ => {}
    }
    out
}
