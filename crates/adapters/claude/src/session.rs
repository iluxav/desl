//! Claude Code's session files (the native route): where they are, and their lines as events.
//! `~/.claude/projects/<cwd with every non-alphanumeric character as ->/<session-id>.jsonl`.
//! Message lines have the shape of the stream-json output, so [`Parser`] reads them; the
//! differences are handled here. Shapes verified against Claude Code 2.1.318; see docs/adapters.md.

use crate::parse::Parser;
use desl_ir::adapter::{AdapterEvent, CopyItem, Reading, SessionReader, copy_items, new_uuid};
use desl_ir::{EventKind, tool};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

pub fn dir(cwd: &Path) -> PathBuf {
    dir_in(&home(), cwd)
}

fn dir_in(home: &Path, cwd: &Path) -> PathBuf {
    let slug: String = cwd.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    home.join(".claude/projects").join(slug)
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
    /// The model of the last reply, to report a change once.
    model: Option<String>,
    /// Read a subagent's file, whose every line is a sidechain, instead of a session's.
    sidechain: bool,
}

impl Reader {
    pub fn new(cwd: &Path) -> Reader {
        Reader { parser: Parser::new(cwd), model: None, sidechain: false }
    }
    pub fn subagent(cwd: &Path) -> Reader {
        Reader { parser: Parser::new(cwd), model: None, sidechain: true }
    }
}

/// `/effort` answers "Set effort level to high (saved as your default …)".
fn effort_set(text: &str) -> Option<String> {
    let rest = text.split("Set effort level to ").nth(1)?;
    let level: String = rest.chars().take_while(char::is_ascii_alphanumeric).collect();
    (!level.is_empty()).then_some(level)
}

impl SessionReader for Reader {
    fn line(&mut self, line: &str) -> Vec<Reading> {
        let Ok(mut v) = serde_json::from_str::<Value>(line) else { return Vec::new() };
        if (v.get("isSidechain").and_then(Value::as_bool) == Some(true)) != self.sidechain {
            return Vec::new();
        }
        match v.get("type").and_then(Value::as_str) {
            Some("user") => {
                let content = &v["message"]["content"];
                if let Some(text) = content.as_str() {
                    if tagged(text) {
                        return effort_set(text)
                            .map(|effort| vec![Reading::Settings { model: None, effort: Some(effort) }])
                            .unwrap_or_default();
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
            // Claude Code's own title for the conversation, written again as the session goes on.
            Some("ai-title") => {
                return v["aiTitle"]
                    .as_str()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(|t| vec![Reading::Title(t.to_string())])
                    .unwrap_or_default();
            }
            _ => return Vec::new(),
        }
        let mut out = Vec::new();
        // Each reply names its model; a change made with /model shows here first.
        if let Some(model) = v["message"]["model"].as_str().filter(|m| m.starts_with("claude-"))
            && self.model.as_deref() != Some(model)
        {
            self.model = Some(model.to_string());
            out.push(Reading::Settings { model: Some(model.to_string()), effort: None });
        }
        // The session file names the structured result `toolUseResult`; the stream, `tool_use_result`.
        if let Some(r) = v.get("toolUseResult").cloned() {
            v["tool_use_result"] = r;
        }
        out.extend(self.parser.line(&v).into_iter().filter_map(|e| match e {
            AdapterEvent::Event { event } => Some(Reading::Event(event)),
            _ => None,
        }));
        out
    }
}

/// Claude Code's own dialogs at the bottom of its screen: a permission question with its first
/// option (the question alone can end an answer), and the folder-trust question.
pub fn waiting(tail: &str) -> bool {
    (tail.contains("Do you want to") && tail.contains("1. Yes")) || tail.contains("Is this a project you") || tail.contains("Do you trust")
}

/// Claude Code's own tool for a neutral call, with its input; None when Claude has none that
/// says the same thing, and the call is told in words instead.
fn native_tool(cwd: &Path, name: &str, args: &Value) -> Option<(&'static str, Value)> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let abs = |p: String| if p.starts_with('/') { p } else { cwd.join(p).to_string_lossy().into_owned() };
    match name {
        tool::BASH => Some(("Bash", json!({ "command": s("command") }))),
        tool::READ => Some(("Read", json!({ "file_path": abs(s("path")) }))),
        tool::SEARCH if !s("pattern").is_empty() => Some(("Grep", json!({ "pattern": s("pattern") }))),
        tool::SEARCH => Some(("WebSearch", json!({ "query": s("query") }))),
        tool::FETCH => Some(("WebFetch", json!({ "url": s("url"), "prompt": "" }))),
        _ => None,
    }
}

fn toolu(id: &str) -> String {
    if id.starts_with("toolu_") {
        return id.to_string();
    }
    let alnum: String = id.chars().filter(char::is_ascii_alphanumeric).collect();
    format!("toolu_{}", &alnum[alnum.len().saturating_sub(24)..])
}

/// A call Claude has no tool for, told as the assistant's own words.
fn told(name: &str, args: &Value, output: &str, diff: Option<&str>) -> String {
    let target = desl_ir::tool_target(name, args);
    match (name, diff) {
        (tool::EDIT, Some(d)) => format!("I edited {target}:\n```diff\n{}\n```", d.trim_end()),
        (tool::EDIT, None) => format!("I edited {target}."),
        _ if output.trim().is_empty() => format!("[{name} {target}]"),
        _ => format!("[{name} {target}]\n{}", output.trim_end()),
    }
}

/// A new session file holding `events`, chained the way Claude Code chains its own lines.
pub fn write(cwd: &Path, events: &[EventKind]) -> Result<(String, PathBuf, u64), String> {
    write_in(&home(), cwd, events)
}

fn write_in(home: &Path, cwd: &Path, events: &[EventKind]) -> Result<(String, PathBuf, u64), String> {
    let session_id = new_uuid();
    let dir = dir_in(home, cwd);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create {} ({e})", dir.display()))?;
    let path = dir.join(format!("{session_id}.jsonl"));
    let cwd_s = cwd.to_string_lossy().into_owned();
    let now = || chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut out = String::new();
    let mut parent = Value::Null;
    let mut push = |kind: &str, message: Value, out: &mut String| {
        let uuid = new_uuid();
        let line = json!({
            "parentUuid": parent, "isSidechain": false, "userType": "external", "cwd": cwd_s,
            "sessionId": session_id, "uuid": uuid, "timestamp": now(), "type": kind, "message": message,
        });
        out.push_str(&line.to_string());
        out.push('\n');
        parent = Value::String(uuid);
    };
    let assistant = |content: Value, stop: &str| {
        json!({ "id": format!("msg_{}", new_uuid().replace('-', "")), "type": "message", "role": "assistant",
                "model": "claude-opus-5-5", "content": content, "stop_reason": stop, "stop_sequence": null,
                "usage": { "input_tokens": 0, "output_tokens": 0 } })
    };
    for item in copy_items(events) {
        match item {
            CopyItem::User(text) => push("user", json!({ "role": "user", "content": text }), &mut out),
            CopyItem::Assistant(text) => push("assistant", assistant(json!([{ "type": "text", "text": text }]), "end_turn"), &mut out),
            CopyItem::Call { id, name, args, output, is_error, diff } => match native_tool(cwd, &name, &args) {
                Some((tool_name, input)) => {
                    let id = toolu(&id);
                    push(
                        "assistant",
                        assistant(json!([{ "type": "tool_use", "id": id, "name": tool_name, "input": input }]), "tool_use"),
                        &mut out,
                    );
                    let result = json!([{ "type": "tool_result", "tool_use_id": id, "content": output, "is_error": is_error }]);
                    push("user", json!({ "role": "user", "content": result }), &mut out);
                }
                None => {
                    let text = told(&name, &args, &output, diff.as_deref());
                    push("assistant", assistant(json!([{ "type": "text", "text": text }]), "end_turn"), &mut out);
                }
            },
        }
    }
    std::fs::write(&path, &out).map_err(|e| format!("Could not write {} ({e})", path.display()))?;
    Ok((session_id, path, out.len() as u64))
}

/// The subagents of session `session_id`, oldest first. Claude keeps each one as
/// `<session>/subagents/agent-<id>.jsonl` beside a `.meta.json` it writes at launch (agentType,
/// description) and marks `"finished": true` when the subagent is done.
pub fn subagents(cwd: &Path, session_id: &str) -> Vec<desl_ir::Subagent> {
    match file(cwd, session_id) {
        Some(f) => subagents_in(&f.with_extension("").join("subagents")),
        None => Vec::new(),
    }
}

/// What subagent `agent_id` of session `session_id` said and did, as IR events.
pub fn subagent_events(cwd: &Path, session_id: &str, agent_id: &str) -> Vec<desl_ir::EventKind> {
    let Some(f) = file(cwd, session_id) else { return Vec::new() };
    if agent_id.contains(['/', '.']) {
        return Vec::new();
    }
    let path = f.with_extension("").join("subagents").join(format!("agent-{agent_id}.jsonl"));
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let mut reader = Reader::subagent(cwd);
    text.lines()
        .flat_map(|l| reader.line(l))
        .filter_map(|r| match r {
            Reading::Event(kind) => Some(kind),
            _ => None,
        })
        .collect()
}

fn subagents_in(dir: &Path) -> Vec<desl_ir::Subagent> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<desl_ir::Subagent> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let id = name.strip_prefix("agent-")?.strip_suffix(".meta.json")?.to_string();
            let meta: serde_json::Value = serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok()?;
            let md = e.metadata().ok()?;
            let when = md.created().or_else(|_| md.modified()).ok()?;
            let started_at = when.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
            Some(desl_ir::Subagent {
                id,
                kind: meta.get("agentType").and_then(|v| v.as_str()).unwrap_or("agent").to_string(),
                description: meta.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                finished: meta.get("finished").and_then(|v| v.as_bool()).unwrap_or(false),
                started_at,
                ..Default::default()
            })
        })
        .collect();
    out.sort_by(|a, b| a.started_at.cmp(&b.started_at).then(a.id.cmp(&b.id)));
    out
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
                Reading::Settings { .. } => "settings".into(),
                Reading::Title(_) => "title".into(),
            })
            .collect();
        assert_eq!(names, ["working", "user_message", "tool_call", "tool_result", "assistant_message", "idle"]);
        assert!(matches!(&got[2], Reading::Event(EventKind::ToolCall { name, args, .. }) if name == "read" && args["path"] == "notes.txt"));
        assert!(matches!(&got[3], Reading::Event(EventKind::ToolResult { output, .. }) if output.contains("deadline: Friday")));
    }

    #[test]
    fn a_model_or_effort_chosen_in_claude_is_reported_once() {
        let got = read(&[
            r#"{"type":"assistant","message":{"model":"claude-opus-5-5","role":"assistant","content":[{"type":"text","text":"a"}]}}"#,
            r#"{"type":"assistant","message":{"model":"claude-opus-5-5","role":"assistant","content":[{"type":"text","text":"b"}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":"<local-command-stdout>Set effort level to high (saved as your default for new sessions): Complex tasks</local-command-stdout>"}}"#,
        ]);
        let settings: Vec<&Reading> = got.iter().filter(|r| matches!(r, Reading::Settings { .. })).collect();
        assert_eq!(
            settings,
            [
                &Reading::Settings { model: Some("claude-opus-5-5".into()), effort: None },
                &Reading::Settings { model: None, effort: Some("high".into()) }
            ]
        );
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

    #[test]
    fn a_written_session_reads_back_as_the_same_conversation() {
        let home = std::env::temp_dir().join(format!("desl-claude-write-{}", std::process::id()));
        let events = vec![
            EventKind::UserMessage { text: "Read notes.txt".into(), attachments: vec![] },
            EventKind::ToolCall { id: "call_9".into(), name: "read".into(), args: json!({ "path": "notes.txt" }) },
            EventKind::ToolResult {
                call_id: "call_9".into(),
                output: "deadline: Friday".into(),
                exit_code: None,
                is_error: false,
                truncated: false,
                blob: None,
            },
            EventKind::ToolCall { id: "call_10".into(), name: "edit".into(), args: json!({ "path": "net.py" }) },
            EventKind::ToolResult {
                call_id: "call_10".into(),
                output: "ok".into(),
                exit_code: None,
                is_error: false,
                truncated: false,
                blob: None,
            },
            EventKind::AssistantMessage { text: "The deadline is Friday.".into(), model: None },
        ];
        let (id, path, len) = write_in(&home, Path::new("/tmp/demo"), &events).unwrap();
        assert!(path.ends_with(format!("{id}.jsonl")));
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.len() as u64, len);
        let mut r = Reader::new(Path::new("/tmp/demo"));
        let back: Vec<Reading> = text.lines().flat_map(|l| r.line(l)).filter(|x| !matches!(x, Reading::Settings { .. })).collect();
        assert!(matches!(&back[1], Reading::Event(EventKind::UserMessage { text, .. }) if text == "Read notes.txt"));
        assert!(
            matches!(&back[2], Reading::Event(EventKind::ToolCall { name, args, .. }) if name == "read" && args["path"] == "notes.txt")
        );
        assert!(matches!(&back[3], Reading::Event(EventKind::ToolResult { output, .. }) if output == "deadline: Friday"));
        assert!(matches!(&back[4], Reading::Event(EventKind::AssistantMessage { text, .. }) if text == "I edited net.py."));
        assert!(matches!(&back[5], Reading::Event(EventKind::AssistantMessage { text, .. }) if text == "The deadline is Friday."));
        // Every line hangs off the one before it.
        let lines: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        for w in lines.windows(2) {
            assert_eq!(w[1]["parentUuid"], w[0]["uuid"]);
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn subagents_come_from_their_meta_files() {
        let dir = std::env::temp_dir().join(format!("desl-subagents-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("agent-a1.meta.json"), r#"{"agentType":"Explore","description":"find auth"}"#).unwrap();
        std::fs::write(dir.join("agent-a1.jsonl"), "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::write(dir.join("agent-b2.meta.json"), r#"{"agentType":"general-purpose","description":"write tests","finished":true}"#)
            .unwrap();
        let got = subagents_in(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(
            (got[0].id.as_str(), got[0].kind.as_str(), got[0].description.as_str(), got[0].finished),
            ("a1", "Explore", "find auth", false)
        );
        assert_eq!((got[1].id.as_str(), got[1].finished), ("b2", true));
        assert!(subagents_in(Path::new("/nonexistent")).is_empty());
    }

    #[test]
    fn a_subagent_file_reads_as_its_own_conversation() {
        let lines = [
            r#"{"type":"user","isSidechain":true,"agentId":"a1","uuid":"u1","message":{"role":"user","content":"Write a haiku"}}"#,
            r#"{"type":"assistant","isSidechain":true,"agentId":"a1","uuid":"u2","message":{"role":"assistant","model":"claude-haiku-5-5","content":[{"type":"text","text":"Rivers run"}]}}"#,
        ];
        let mut sub = Reader::subagent(Path::new("/w"));
        let mut main = Reader::new(Path::new("/w"));
        let got: Vec<Reading> = lines.iter().flat_map(|l| sub.line(l)).collect();
        assert!(
            got.iter().any(|r| matches!(r, Reading::Event(desl_ir::EventKind::UserMessage { text, .. }) if text == "Write a haiku")),
            "{got:?}"
        );
        assert!(
            got.iter().any(|r| matches!(r, Reading::Event(desl_ir::EventKind::AssistantMessage { text, .. }) if text == "Rivers run")),
            "{got:?}"
        );
        assert!(lines.iter().flat_map(|l| main.line(l)).next().is_none(), "the session reader skips subagent lines");
    }

    #[test]
    fn claude_codes_own_title_reads_as_the_threads_title() {
        let got = read(&[
            r#"{"type":"ai-title","aiTitle":"Herdr API awareness in desl","sessionId":"s1"}"#,
            r#"{"type":"ai-title","aiTitle":"  ","sessionId":"s1"}"#,
        ]);
        assert_eq!(got, vec![Reading::Title("Herdr API awareness in desl".into())]);
        let mut sub = Reader::subagent(Path::new("/tmp/demo"));
        assert!(sub.line(r#"{"type":"ai-title","aiTitle":"x","sessionId":"s1"}"#).is_empty(), "a subagent's reader leaves titles alone");
    }
}
