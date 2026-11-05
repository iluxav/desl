//! Codex's session files (the native route): where they are, and their lines as events.
//! `~/.codex/sessions/YYYY/MM/DD/rollout-<time>-<session-id>.jsonl`, headed by `session_meta`.
//! Shapes verified against Codex CLI 0.166.0; see docs/adapters.md.

use crate::parse::unwrap_shell;
use desl_ir::adapter::{CopyItem, Reading, SessionReader, copy_items};
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
                // Codex's own /model changes only the running session; it records the choice here.
                Some("thread_settings_applied") => {
                    let s = &p["thread_settings"];
                    let get = |k: &str| s.get(k).and_then(Value::as_str).filter(|v| !v.is_empty()).map(str::to_string);
                    let (model, effort) = (get("model"), get("reasoning_effort"));
                    if model.is_none() && effort.is_none() {
                        return Vec::new();
                    }
                    vec![Reading::Settings { model, effort }]
                }
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

/// The shell command Codex would have run for a neutral call; None when there is none.
fn shell_for(name: &str, args: &Value) -> Option<String> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    match name {
        tool::BASH => Some(s("command")),
        tool::READ => Some(format!("cat {}", s("path"))),
        tool::SEARCH if !s("pattern").is_empty() => Some(format!("rg {}", s("pattern"))),
        _ => None,
    }
}

/// A new rollout holding `events`, written the way Codex writes its own: a `session_meta` header
/// with `history_mode: paginated` (without it Codex's screen shows none of the history), one
/// turn per user message, and no ids on assistant messages or tool calls (Codex's API refuses
/// ids it did not issue).
pub fn write(cwd: &Path, events: &[EventKind]) -> Result<(String, PathBuf, u64), String> {
    write_in(&home(), cwd, events)
}

fn write_in(home: &Path, cwd: &Path, events: &[EventKind]) -> Result<(String, PathBuf, u64), String> {
    let id = desl_ir::adapter::new_uuid();
    let t = chrono::Utc::now();
    let dir = home.join(".codex/sessions").join(t.format("%Y/%m/%d").to_string());
    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create {} ({e})", dir.display()))?;
    let path = dir.join(format!("rollout-{}-{id}.jsonl", t.format("%Y-%m-%dT%H-%M-%S")));
    let cwd_s = cwd.to_string_lossy().into_owned();
    let now = || chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let ms = t.timestamp_millis();
    let mut lines: Vec<Value> = vec![json!({ "timestamp": now(), "type": "session_meta", "payload": {
        "session_id": id, "id": id, "timestamp": now(), "cwd": cwd_s, "originator": "codex-tui",
        "cli_version": "0.166.0", "source": "cli", "thread_source": "user", "model_provider": "openai",
        "history_mode": "paginated" } })];
    let item = |turn: &str, item: Value| {
        json!({ "timestamp": now(), "type": "event_msg", "payload": { "type": "item_completed", "thread_id": id,
                "turn_id": turn, "item": item, "started_at_ms": ms, "completed_at_ms": ms } })
    };
    let response = |turn: &str, mut p: Value| {
        p["internal_chat_message_metadata_passthrough"] = json!({ "turn_id": turn });
        json!({ "timestamp": now(), "type": "response_item", "payload": p })
    };
    let start = |turn: &str| {
        json!({ "timestamp": now(), "type": "event_msg", "payload": { "type": "task_started",
            "turn_attribution": { "turn_id": turn, "turn_trigger": "user", "parent_turn_id": null,
                                  "initiating_agent_path": null, "root_turn_id": turn },
            "turn_id": turn, "root_turn_id": turn, "started_at": ms / 1000, "model_context_window": 258400,
            "collaboration_mode_kind": "default" } })
    };
    let end = |turn: &str, last: &str| {
        json!({ "timestamp": now(), "type": "event_msg", "payload": { "type": "task_complete", "root_turn_id": turn,
            "turn_id": turn, "last_agent_message": last, "started_at": ms / 1000, "completed_at": ms / 1000,
            "duration_ms": 0, "time_to_first_token_ms": 0 } })
    };
    let mut turn: Option<(String, String)> = None;
    for it in copy_items(events) {
        if matches!(it, CopyItem::User(_)) || turn.is_none() {
            if let Some((t, last)) = &turn {
                lines.push(end(t, last));
            }
            let t = desl_ir::adapter::new_uuid();
            lines.push(start(&t));
            turn = Some((t, String::new()));
        }
        let Some((tid, last)) = turn.as_mut() else { continue };
        match it {
            CopyItem::User(text) => {
                lines.push(response(
                    tid,
                    json!({ "type": "message", "id": format!("msg_{}", desl_ir::adapter::new_uuid()),
                    "role": "user", "content": [{ "type": "input_text", "text": text }] }),
                ));
                lines.push(item(
                    tid,
                    json!({ "type": "UserMessage", "id": desl_ir::adapter::new_uuid(),
                    "content": [{ "type": "text", "text": text, "text_elements": [] }] }),
                ));
            }
            CopyItem::Assistant(text) => {
                lines.push(response(
                    tid,
                    json!({ "type": "message", "status": "completed", "role": "assistant",
                    "content": [{ "type": "output_text", "text": text, "annotations": [], "logprobs": [] }], "phase": "final_answer" }),
                ));
                lines.push(item(
                    tid,
                    json!({ "type": "AgentMessage", "id": format!("msg_{}", desl_ir::adapter::new_uuid().replace('-', "")),
                    "content": [{ "type": "Text", "text": text }], "phase": "final_answer" }),
                ));
                last.clone_from(&text);
            }
            CopyItem::Call { id: call_id, name, args, output, diff, .. } => {
                let shell = shell_for(&name, &args);
                let script = match &shell {
                    Some(cmd) => format!("const r = await tools.exec_command({{cmd:{}}});\ntext(r.output);\n", json!(cmd)),
                    None => format!("// {name} {}\n", desl_ir::tool_target(&name, &args)),
                };
                let shown = diff.unwrap_or(output);
                lines.push(response(
                    tid,
                    json!({ "type": "custom_tool_call", "status": "completed", "call_id": call_id, "name": "exec", "input": script }),
                ));
                if let Some(cmd) = shell {
                    lines.push(item(
                        tid,
                        json!({ "type": "CommandExecution", "id": format!("exec-{}", desl_ir::adapter::new_uuid()),
                        "command": ["/usr/bin/bash", "-lc", cmd], "cwd": format!("file://{cwd_s}"),
                        "parsed_cmd": [{ "type": "unknown", "cmd": cmd }], "source": "unified_exec_startup",
                        "status": "completed", "aggregated_output": shown, "exit_code": 0, "duration": { "secs": 0, "nanos": 0 } }),
                    ));
                }
                lines.push(response(tid, json!({ "type": "custom_tool_call_output", "id": format!("ctco_{}", desl_ir::adapter::new_uuid()),
                    "call_id": call_id, "output": [{ "type": "input_text", "text": "Script completed\nOutput:\n" }, { "type": "input_text", "text": shown }] })));
            }
        }
    }
    if let Some((t, last)) = &turn {
        lines.push(end(t, last));
    }
    let mut out = String::new();
    for (n, mut l) in lines.into_iter().enumerate() {
        l["ordinal"] = json!(n);
        out.push_str(&l.to_string());
        out.push('\n');
    }
    std::fs::write(&path, &out).map_err(|e| format!("Could not write {} ({e})", path.display()))?;
    Ok((id, path, out.len() as u64))
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

    #[test]
    fn a_model_or_effort_chosen_in_codex_is_reported() {
        let mut r = Reader::new(Path::new("/tmp/demo"));
        let got = r.line(r#"{"type":"event_msg","payload":{"type":"thread_settings_applied","thread_settings":{"model":"gpt-6-astra","reasoning_effort":"high"}}}"#);
        assert_eq!(got, vec![Reading::Settings { model: Some("gpt-6-astra".into()), effort: Some("high".into()) }]);
        let got = r.line(r#"{"type":"event_msg","payload":{"type":"thread_settings_applied","thread_settings":{"model":"gpt-6-luna"}}}"#);
        assert_eq!(got, vec![Reading::Settings { model: Some("gpt-6-luna".into()), effort: None }]);
    }

    #[test]
    fn a_written_rollout_reads_back_as_the_same_conversation() {
        let home = std::env::temp_dir().join(format!("desl-codex-write-{}", std::process::id()));
        let events = vec![
            EventKind::UserMessage { text: "Read team.txt".into(), attachments: vec![] },
            EventKind::ToolCall { id: "toolu_1".into(), name: "read".into(), args: json!({ "path": "team.txt" }) },
            EventKind::ToolResult {
                call_id: "toolu_1".into(),
                output: "owner: Ana".into(),
                exit_code: None,
                is_error: false,
                truncated: false,
                blob: None,
            },
            EventKind::AssistantMessage { text: "The owner is Ana.".into(), model: None },
            EventKind::UserMessage { text: "Thanks".into(), attachments: vec![] },
            EventKind::AssistantMessage { text: "Any time.".into(), model: None },
        ];
        let (id, path, len) = write_in(&home, Path::new("/tmp/demo"), &events).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.len() as u64, len);
        assert_eq!(meta(&path), Some((id, "/tmp/demo".to_string())));
        assert!(text.contains("\"history_mode\":\"paginated\""));
        let mut r = Reader::new(Path::new("/tmp/demo"));
        let back: Vec<Reading> = text.lines().flat_map(|l| r.line(l)).collect();
        let events_back: Vec<&EventKind> = back.iter().filter_map(|x| if let Reading::Event(e) = x { Some(e) } else { None }).collect();
        assert_eq!(events_back.len(), 6);
        assert!(matches!(events_back[1], EventKind::ToolCall { name, args, .. } if name == "bash" && args["command"] == "cat team.txt"));
        assert!(matches!(events_back[2], EventKind::ToolResult { output, .. } if output.contains("owner: Ana")));
        assert_eq!(back.iter().filter(|x| **x == Reading::Idle).count(), 2, "one turn per prompt");
        // Codex refuses ids it did not issue on assistant messages and tool calls.
        for l in text.lines() {
            let v: Value = serde_json::from_str(l).unwrap();
            let p = &v["payload"];
            if p["type"] == "custom_tool_call" || (p["type"] == "message" && p["role"] == "assistant") {
                assert!(p.get("id").is_none(), "{l}");
            }
        }
        let _ = std::fs::remove_dir_all(&home);
    }
}
