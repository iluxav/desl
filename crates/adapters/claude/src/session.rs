//! Claude Code's session files (the native route): where they are, and their lines as events.
//! `~/.claude/projects/<cwd with every non-alphanumeric character as ->/<session-id>.jsonl`.
//! Message lines have the shape of the stream-json output, so [`Parser`] reads them; the
//! differences are handled here. Shapes verified against Claude Code 2.1.318; see docs/adapters.md.

use crate::parse::Parser;
use desl_ir::EventKind;
use desl_ir::adapter::{AdapterEvent, Reading, SessionReader};
use serde_json::Value;
use std::path::{Path, PathBuf};

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

pub fn dir(cwd: &Path) -> PathBuf {
    let slug: String = cwd.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    home().join(".claude/projects").join(slug)
}

pub fn file(cwd: &Path, session_id: &str) -> Option<PathBuf> {
    let p = dir(cwd).join(format!("{session_id}.jsonl"));
    if p.exists() {
        return Some(p);
    }
    // A slug rule that moved: look in every project directory.
    std::fs::read_dir(home().join(".claude/projects"))
        .ok()?
        .flatten()
        .map(|d| d.path().join(format!("{session_id}.jsonl")))
        .find(|p| p.exists())
}

/// Claude Code's own records in the user's place: local commands, caveats, bash mode.
fn tagged(text: &str) -> bool {
    text.trim_start().starts_with('<')
}

pub struct Reader {
    parser: Parser,
}

impl Reader {
    pub fn new(cwd: &Path) -> Reader {
        Reader { parser: Parser::new(cwd) }
    }
}

impl SessionReader for Reader {
    fn line(&mut self, line: &str) -> Vec<Reading> {
        let Ok(mut v) = serde_json::from_str::<Value>(line) else { return Vec::new() };
        if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return Vec::new();
        }
        match v.get("type").and_then(Value::as_str) {
            Some("user") => {
                let content = &v["message"]["content"];
                if let Some(text) = content.as_str() {
                    if tagged(text) {
                        return Vec::new();
                    }
                    return vec![
                        Reading::Working,
                        Reading::Event(EventKind::UserMessage { text: text.to_string(), attachments: Vec::new() }),
                    ];
                }
                let interrupted = content.as_array().into_iter().flatten().any(|b| {
                    b.get("type").and_then(Value::as_str) == Some("text")
                        && b.get("text").and_then(Value::as_str).is_some_and(|t| t.starts_with("[Request interrupted"))
                });
                if interrupted {
                    return vec![Reading::Event(EventKind::Interrupt), Reading::Idle];
                }
            }
            Some("system") => {
                // Written once after every finished turn: the dependable end.
                if v.get("subtype").and_then(Value::as_str) == Some("turn_duration") {
                    return vec![Reading::Idle];
                }
                return Vec::new();
            }
            Some("assistant") => {}
            _ => return Vec::new(),
        }
        // The session file names the structured result `toolUseResult`; the stream, `tool_use_result`.
        if let Some(r) = v.get("toolUseResult").cloned() {
            v["tool_use_result"] = r;
        }
        self.parser
            .line(&v)
            .into_iter()
            .filter_map(|e| match e {
                AdapterEvent::Event { event } => Some(Reading::Event(event)),
                _ => None,
            })
            .collect()
    }
}

/// Claude Code's own dialogs at the bottom of its screen: a permission question with its first
/// option (the question alone can end an answer), and the folder-trust question.
pub fn waiting(tail: &str) -> bool {
    (tail.contains("Do you want to") && tail.contains("1. Yes")) || tail.contains("Is this a project you") || tail.contains("Do you trust")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(lines: &[&str]) -> Vec<Reading> {
        let mut r = Reader::new(Path::new("/tmp/demo"));
        lines.iter().flat_map(|l| r.line(l)).collect()
    }

    #[test]
    fn a_turn_reads_as_prompt_tool_call_result_reply_and_end() {
        let got = read(&[
            r#"{"type":"user","message":{"role":"user","content":"Read notes.txt"},"uuid":"a"}"#,
            r#"{"type":"user","message":{"role":"user","content":"<command-name>/model</command-name>"},"isMeta":true}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"","signature":"x"}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"/tmp/demo/notes.txt"}}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"1\tdeadline: Friday"}]},"toolUseResult":{"type":"text","file":{"filePath":"/tmp/demo/notes.txt","content":"deadline: Friday\n"}}}"#,
            r#"{"type":"assistant","isSidechain":true,"message":{"role":"assistant","content":[{"type":"text","text":"subagent"}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"The deadline is Friday."}]}}"#,
            r#"{"type":"system","subtype":"turn_duration","durationMs":4000}"#,
        ]);
        let names: Vec<String> = got
            .iter()
            .map(|r| match r {
                Reading::Event(e) => e.name().to_string(),
                Reading::Working => "working".into(),
                Reading::Idle => "idle".into(),
            })
            .collect();
        assert_eq!(names, ["working", "user_message", "tool_call", "tool_result", "assistant_message", "idle"]);
        assert!(matches!(&got[2], Reading::Event(EventKind::ToolCall { name, args, .. }) if name == "read" && args["path"] == "notes.txt"));
        assert!(matches!(&got[3], Reading::Event(EventKind::ToolResult { output, .. }) if output.contains("deadline: Friday")));
    }

    #[test]
    fn an_interrupt_ends_the_turn() {
        let got =
            read(&[r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user]"}]}}"#]);
        assert_eq!(got, vec![Reading::Event(EventKind::Interrupt), Reading::Idle]);
    }

    #[test]
    fn the_dialog_needs_its_first_option() {
        assert!(waiting(" Do you want to proceed?\n ❯ 1. Yes\n   2. No"));
        assert!(!waiting("● Shall I go on? Do you want to proceed?"));
    }
}
