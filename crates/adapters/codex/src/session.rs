//! Codex's session files (the native route): where they are, and their lines as events.
//! `~/.codex/sessions/YYYY/MM/DD/rollout-<time>-<session-id>.jsonl`, headed by `session_meta`.
//! Shapes verified against Codex CLI 0.166.0; see docs/adapters.md.

use crate::parse::unwrap_shell;
use desl_ir::adapter::{Reading, SessionReader};
use desl_ir::{EventKind, tool};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Tool output kept inline; the store moves longer text to a blob.
const OUTPUT_CAP: usize = 64 * 1024;

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

fn rollouts() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![home().join(".codex/sessions")];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "jsonl") {
                out.push(p);
            }
        }
    }
    out
}

/// The session's id and directory, from its first line.
fn meta(path: &Path) -> Option<(String, String)> {
    use std::io::BufRead;
    let f = std::fs::File::open(path).ok()?;
    let mut first = String::new();
    std::io::BufReader::new(f).read_line(&mut first).ok()?;
    let v: Value = serde_json::from_str(&first).ok()?;
    let p = &v["payload"];
    Some((p["id"].as_str()?.to_string(), p["cwd"].as_str()?.to_string()))
}

/// By id; or the newest session in `cwd` begun after `since`, which is how a session Codex
/// named itself is found.
pub fn file(cwd: &Path, session_id: Option<&str>, since: SystemTime) -> Option<(String, PathBuf)> {
    let all = rollouts();
    if let Some(id) = session_id {
        let p = all.into_iter().find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().contains(id)))?;
        return Some((id.to_string(), p));
    }
    let cwd = cwd.to_string_lossy();
    all.into_iter()
        .filter_map(|p| {
            let created = std::fs::metadata(&p).and_then(|m| m.created().or_else(|_| m.modified())).ok()?;
            (created >= since).then_some((created, p))
        })
        .filter_map(|(t, p)| meta(&p).filter(|(_, dir)| *dir == cwd).map(|(id, _)| (t, id, p)))
        .max_by_key(|(t, _, _)| *t)
        .map(|(_, id, p)| (id, p))
}

fn text_of(content: &Value) -> String {
    content.as_array().into_iter().flatten().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("")
}

/// Codex's own records in the user's place: environment, instructions, AGENTS.md.
fn tagged(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with('<') || t.starts_with("# AGENTS.md")
}

/// The first JSON string after `key` in an `exec` script: `tools.exec_command({cmd:"…"})`.
fn string_after(script: &str, key: &str) -> Option<String> {
    let at = script.find(key)? + key.len();
    let rest = script[at..].trim_start();
    serde_json::Deserializer::from_str(rest).into_iter::<String>().next()?.ok()
}

fn patch_path(patch: &str) -> String {
    patch
        .lines()
        .find_map(|l| ["*** Update File: ", "*** Add File: ", "*** Delete File: "].iter().find_map(|p| l.strip_prefix(p)))
        .unwrap_or("")
        .trim()
        .to_string()
}

fn cap(s: String) -> String {
    if s.len() <= OUTPUT_CAP {
        return s;
    }
    let mut end = OUTPUT_CAP;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

pub struct Reader {
    cwd: String,
}

impl Reader {
    pub fn new(cwd: &Path) -> Reader {
        Reader { cwd: cwd.to_string_lossy().into_owned() }
    }

    fn rel(&self, path: &str) -> String {
        match path.strip_prefix(&self.cwd) {
            Some(r) if r.starts_with('/') => r.trim_start_matches('/').to_string(),
            _ => path.to_string(),
        }
    }

    /// An `exec` script as the neutral tool call it makes.
    fn exec(&self, script: &str) -> (String, Value) {
        if let Some(cmd) = string_after(script, "cmd:") {
            return (tool::BASH.into(), json!({ "command": unwrap_shell(&cmd) }));
        }
        if let Some(patch) = string_after(script, "apply_patch(") {
            return (tool::EDIT.into(), json!({ "path": self.rel(&patch_path(&patch)) }));
        }
        ("exec".into(), json!({ "script": script }))
    }
}

impl SessionReader for Reader {
    fn line(&mut self, line: &str) -> Vec<Reading> {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return Vec::new() };
        let p = &v["payload"];
        let kind = p.get("type").and_then(Value::as_str);
        if v["type"] == "event_msg" {
            return match kind {
                Some("task_started") => vec![Reading::Working],
                Some("task_complete") => vec![Reading::Idle],
                Some("turn_aborted") => vec![Reading::Event(EventKind::Interrupt), Reading::Idle],
                _ => Vec::new(),
            };
        }
        if v["type"] != "response_item" {
            return Vec::new();
        }
        let id = || p.get("call_id").and_then(Value::as_str).unwrap_or("").to_string();
        let event = match (kind, p.get("role").and_then(Value::as_str)) {
            (Some("message"), Some("user")) => {
                let text = text_of(&p["content"]);
                if tagged(&text) {
                    return Vec::new();
                }
                EventKind::UserMessage { text: text.trim_end_matches('\n').to_string(), attachments: Vec::new() }
            }
            (Some("message"), Some("assistant")) => {
                let text = text_of(&p["content"]);
                if text.trim().is_empty() {
                    return Vec::new();
                }
                EventKind::AssistantMessage { text, model: None }
            }
            (Some("custom_tool_call"), _) => {
                let (name, args) = self.exec(p.get("input").and_then(Value::as_str).unwrap_or(""));
                EventKind::ToolCall { id: id(), name, args }
            }
            (Some("function_call"), _) => {
                let name = p.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                let args: Value =
                    p.get("arguments").and_then(Value::as_str).and_then(|a| serde_json::from_str(a).ok()).unwrap_or(json!({}));
                match args.get("cmd").or_else(|| args.get("command")) {
                    Some(Value::String(c)) => {
                        EventKind::ToolCall { id: id(), name: tool::BASH.into(), args: json!({ "command": unwrap_shell(c) }) }
                    }
                    _ => EventKind::ToolCall { id: id(), name, args },
                }
            }
            (Some("custom_tool_call_output" | "function_call_output"), _) => {
                let o = &p["output"];
                let text = o.as_str().map(str::to_string).unwrap_or_else(|| text_of(o));
                EventKind::ToolResult { call_id: id(), output: cap(text), exit_code: None, is_error: false, truncated: false, blob: None }
            }
            _ => return Vec::new(),
        };
        vec![Reading::Event(event)]
    }
}

/// Codex's own questions at the bottom of its screen: an approval, or the folder-trust question.
pub fn waiting(tail: &str) -> bool {
    tail.contains("Would you like to") || tail.contains("Trust this folder") || tail.contains("Allow") && tail.contains("1. Yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_reads_as_prompt_command_result_reply_and_end() {
        let mut r = Reader::new(Path::new("/tmp/demo"));
        let lines = [
            r#"{"type":"session_meta","payload":{"id":"s1","cwd":"/tmp/demo"}}"#,
            r#"{"type":"event_msg","payload":{"type":"task_started","turn_id":"t1"}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>…"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Read team.txt\n"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"custom_tool_call","call_id":"c1","name":"exec","input":"const r = await tools.exec_command({cmd:\"cat team.txt\",\"max_output_tokens\":2000});\ntext(r.output);\n"}}"#,
            r#"{"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"c1","output":[{"type":"input_text","text":"owner: Ana\n"}]}}"#,
            r#"{"type":"response_item","payload":{"type":"custom_tool_call","call_id":"c2","name":"exec","input":"const r = await tools.apply_patch(\"*** Begin Patch\\n*** Update File: /tmp/demo/net.py\\n@@\\n-a\\n+b\\n*** End Patch\");"}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"The owner is Ana."}]}}"#,
            r#"{"type":"event_msg","payload":{"type":"task_complete","turn_id":"t1"}}"#,
        ];
        let got: Vec<Reading> = lines.iter().flat_map(|l| r.line(l)).collect();
        assert_eq!(got.len(), 7);
        assert_eq!(got[0], Reading::Working);
        assert!(matches!(&got[1], Reading::Event(EventKind::UserMessage { text, .. }) if text == "Read team.txt"));
        assert!(
            matches!(&got[2], Reading::Event(EventKind::ToolCall { name, args, .. }) if name == "bash" && args["command"] == "cat team.txt")
        );
        assert!(matches!(&got[3], Reading::Event(EventKind::ToolResult { output, .. }) if output == "owner: Ana\n"));
        assert!(matches!(&got[4], Reading::Event(EventKind::ToolCall { name, args, .. }) if name == "edit" && args["path"] == "net.py"));
        assert!(matches!(&got[5], Reading::Event(EventKind::AssistantMessage { text, .. }) if text == "The owner is Ana."));
        assert_eq!(got[6], Reading::Idle);
    }
}
