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

/// The rollout held open by this Codex interface, including after /new or /resume.
pub fn running(pid: u32) -> Option<(String, PathBuf)> {
    running_in(Path::new("/proc"), pid)
}

fn running_in(proc: &Path, pid: u32) -> Option<(String, PathBuf)> {
    use std::io::{BufRead, Read};
    // The npm launcher may own the PTY, with Codex in a child. Search nearest processes
    // first, with a bound; never scan unrelated interfaces sharing the same directory.
    let mut level = vec![pid];
    for _ in 0..4 {
        let mut found = std::collections::BTreeMap::new();
        let mut children = Vec::new();
        for pid in level.into_iter().take(32) {
            let base = proc.join(pid.to_string());
            let name = std::fs::read_to_string(base.join("comm")).unwrap_or_default();
            if !matches!(name.trim(), "codex" | "node") {
                continue;
            }
            for entry in std::fs::read_dir(base.join("fd")).into_iter().flatten().flatten() {
                let flags = std::fs::read_to_string(base.join("fdinfo").join(entry.file_name())).ok().and_then(|text| {
                    text.lines().find_map(|line| line.strip_prefix("flags:").and_then(|v| u32::from_str_radix(v.trim(), 8).ok()))
                });
                // A /resume preview or a tool reading another rollout is not its writer.
                if flags.is_none_or(|flags| flags & 3 == 0) {
                    continue;
                }
                let Ok(path) = std::fs::read_link(entry.path()) else { continue };
                if !path.file_name().is_some_and(|n| n.to_string_lossy().starts_with("rollout-"))
                    || path.extension().is_none_or(|e| e != "jsonl")
                {
                    continue;
                }
                if !std::fs::metadata(&path).is_ok_and(|m| m.is_file()) {
                    continue;
                }
                let Ok(file) = std::fs::File::open(&path) else { continue };
                let mut first = String::new();
                if std::io::BufReader::new(file.take(256 * 1024)).read_line(&mut first).is_err() {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<Value>(&first) else { continue };
                let p = &v["payload"];
                if v["type"] == "session_meta"
                    && p["source"] == "cli"
                    && p["thread_source"] != "subagent"
                    && let Some(id) = p["id"].as_str()
                {
                    found.insert(id.to_string(), path);
                }
            }
            if let Ok(text) = std::fs::read_to_string(base.join(format!("task/{pid}/children"))) {
                children.extend(text.split_whitespace().filter_map(|s| s.parse::<u32>().ok()));
            }
        }
        if !found.is_empty() {
            // Ambiguous evidence must leave the known binding alone.
            return (found.len() == 1).then(|| found.into_iter().next()).flatten();
        }
        if children.is_empty() {
            break;
        }
        level = children;
    }
    None
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
        // An MCP tool, called from the runner as `tools.mcp__<server>__<tool>(…)`: named as Claude
        // Code names one, before anything in its arguments is taken for a command.
        if let Some(at) = script.find("tools.mcp__") {
            let name: String =
                script[at + "tools.".len()..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
            return (name, json!({ "script": script }));
        }
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

/// Codex's own questions at the bottom of its screen: an approval, the folder-trust question,
/// which directory a resumed session works in, or whether an MCP server's tool may run. That
/// last one is told by its title or, when long arguments push the title up, by its choices.
pub fn waiting(tail: &str) -> bool {
    tail.contains("Would you like to")
        || tail.contains("Trust this folder")
        || tail.contains("Use session directory")
        || tail.contains("Allow") && tail.contains("1. Yes")
        || tail.contains("MCP server to run tool")
        || tail.contains("1. Allow") && tail.contains("Cancel this tool call")
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

/// What desl knows of one session's subagents, read a little more each time it is asked.
#[derive(Default)]
struct Spawns {
    path: Option<PathBuf>,
    offset: u64,
    partial: Vec<u8>,
    found: Vec<desl_ir::Subagent>,
    /// Each subagent's own session file, once found.
    files: std::collections::HashMap<String, PathBuf>,
}

fn spawns() -> &'static std::sync::Mutex<std::collections::HashMap<String, Spawns>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Spawns>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The subagents in one session line: Codex records a spawn as a `CollabAgentToolCall` item
/// (`tool: spawn_agent`) naming each new agent's session (`receiver_agents`), with its task.
fn spawned(line: &str) -> Vec<desl_ir::Subagent> {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return Vec::new() };
    let p = &v["payload"];
    let item = &p["item"];
    if p["type"] != "item_completed" || item["type"] != "CollabAgentToolCall" || item["tool"] != "spawn_agent" {
        return Vec::new();
    }
    let prompt = item["prompt"].as_str().unwrap_or("").lines().next().unwrap_or("").to_string();
    let when = p["completed_at_ms"].as_i64().or_else(|| p["started_at_ms"].as_i64()).unwrap_or(0);
    item["receiver_agents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| {
            Some(desl_ir::Subagent {
                id: a["thread_id"].as_str()?.to_string(),
                kind: a["agent_nickname"].as_str().or_else(|| a["agent_role"].as_str()).unwrap_or("agent").to_string(),
                description: prompt.clone(),
                finished: false,
                started_at: when,
                ..Default::default()
            })
        })
        .collect()
}

/// Whether a subagent's own session has ended its last task.
fn finished(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else { return false };
    let mut done = false;
    for line in text.lines() {
        if line.contains("\"task_started\"") {
            done = false;
        } else if line.contains("\"task_complete\"") || line.contains("\"turn_aborted\"") {
            done = true;
        }
    }
    done
}

/// The subagents session `session_id` spawned, oldest first.
pub fn subagents(cwd: &Path, session_id: &str) -> Vec<desl_ir::Subagent> {
    use std::io::{Read, Seek, SeekFrom};
    let _ = cwd;
    let mut cache = spawns().lock().unwrap_or_else(|e| e.into_inner());
    let s = cache.entry(session_id.to_string()).or_default();
    if s.path.is_none() {
        s.path = file(Path::new(""), Some(session_id), std::time::UNIX_EPOCH).map(|(_, p)| p);
    }
    let Some(path) = s.path.clone() else { return Vec::new() };
    if let Ok(mut f) = std::fs::File::open(&path)
        && f.seek(SeekFrom::Start(s.offset)).is_ok()
    {
        let mut more = Vec::new();
        if f.read_to_end(&mut more).is_ok() {
            s.offset += more.len() as u64;
            s.partial.extend_from_slice(&more);
            let end = s.partial.iter().rposition(|b| *b == b'\n').map_or(0, |n| n + 1);
            let whole: Vec<u8> = s.partial.drain(..end).collect();
            for line in String::from_utf8_lossy(&whole).lines() {
                for sub in spawned(line) {
                    if !s.found.iter().any(|f| f.id == sub.id) {
                        s.found.push(sub);
                    }
                }
            }
        }
    }
    let waiting: Vec<String> = s.found.iter().filter(|f| !f.finished && !s.files.contains_key(&f.id)).map(|f| f.id.clone()).collect();
    if !waiting.is_empty() {
        for p in rollouts() {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if let Some(id) = waiting.iter().find(|id| name.contains(id.as_str())) {
                s.files.insert(id.clone(), p);
            }
        }
    }
    for sub in s.found.iter_mut().filter(|f| !f.finished) {
        if let Some(p) = s.files.get(&sub.id) {
            sub.finished = finished(p);
        }
    }
    s.found.clone()
}

/// What subagent `agent_id` said and did, read from its own session file.
pub fn subagent_events(cwd: &Path, agent_id: &str) -> Vec<EventKind> {
    let Some((_, path)) = file(cwd, Some(agent_id), std::time::UNIX_EPOCH) else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let mut reader = Reader::new(cwd);
    text.lines()
        .flat_map(|l| reader.line(l))
        .filter_map(|r| match r {
            Reading::Event(kind) => Some(kind),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn the_running_session_belongs_to_the_interface_not_its_helper_or_another_process() {
        use std::os::unix::fs::symlink;
        let dir = std::env::temp_dir().join(format!("desl-codex-running-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("10/fd")).unwrap();
        std::fs::create_dir_all(dir.join("10/fdinfo")).unwrap();
        std::fs::write(dir.join("10/comm"), "codex\n").unwrap();
        for fd in [41, 99] {
            std::fs::write(dir.join(format!("10/fdinfo/{fd}")), "flags:\t0100001\n").unwrap();
        }
        let main = dir.join("rollout-main.jsonl");
        let helper = dir.join("rollout-helper.jsonl");
        std::fs::write(&main, include_str!("../fixtures/0.172.0-active-session.jsonl")).unwrap();
        std::fs::write(&helper, include_str!("../fixtures/0.172.0-helper-session.jsonl")).unwrap();
        symlink(&main, dir.join("10/fd/99")).unwrap();
        symlink(&helper, dir.join("10/fd/41")).unwrap();
        assert_eq!(running_in(&dir, 10), Some(("main-session".into(), main.clone())));
        assert_eq!(running_in(&dir, 11), None);
        // An npm launcher has the interface in a child process.
        std::fs::create_dir_all(dir.join("9/task/9")).unwrap();
        std::fs::write(dir.join("9/comm"), "node\n").unwrap();
        std::fs::write(dir.join("9/task/9/children"), "10").unwrap();
        assert_eq!(running_in(&dir, 9), Some(("main-session".into(), main)));
        std::fs::write(dir.join("10/fdinfo/99"), "flags:\t0100000\n").unwrap();
        assert_eq!(running_in(&dir, 9), None, "a read-only preview is not the active conversation");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_captured_0172_session_reports_an_active_turn() {
        let mut reader = Reader::new(Path::new("/tmp/demo"));
        let readings: Vec<_> =
            include_str!("../fixtures/0.172.0-active-session.jsonl").lines().flat_map(|line| reader.line(line)).collect();
        assert_eq!(readings, [Reading::Working]);
        assert!(include_str!("../fixtures/0.172.0-helper-session.jsonl").lines().flat_map(|line| reader.line(line)).next().is_none());
    }

    #[test]
    fn codexs_mcp_approval_is_a_question_on_its_screen() {
        // Codex 0.171's dialog when an MCP server's tool needs the user's yes (seen 2026-11-22).
        let dialog = r#"  Field 1/1
  Allow the dtest MCP server to run tool "show_form"?
  title: Test
  widgets: [{"kind": "toggle", "id": "a", "label": "A"}]
  › 1. Allow                   Run the tool and continue
    2. Allow for this session  Run the tool and remember this choice for this session
    3. Always allow            Run the tool and remember this choice for future tool calls
    4. Cancel                  Cancel this tool call
  enter to submit | esc to cancel"#;
        assert!(waiting(dialog));
        // Long arguments push its first line out of the rows desl reads: its choices still say it.
        let bottom: String = dialog.lines().skip(4).collect::<Vec<_>>().join("\n");
        assert!(waiting(&bottom), "{bottom}");
        assert!(!waiting("• Called Evaluating expression\n• The result is 2.\n› Ask Codex to do anything"));
    }

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
    fn an_mcp_tool_called_from_the_runner_is_named_as_the_tool() {
        // Codex calls MCP tools from its JavaScript runner (checked with Codex CLI 0.171.0).
        let mut r = Reader::new(Path::new("/tmp/demo"));
        let line = r#"{"type":"response_item","payload":{"type":"custom_tool_call","call_id":"c9","name":"exec","input":"const r = await tools.mcp__desl__show_form({title:\"Run which cmd: tests?\", widgets:[]});\ntext(r);"}}"#;
        let got = r.line(line);
        assert!(
            matches!(&got[0], Reading::Event(EventKind::ToolCall { name, .. }) if name == "mcp__desl__show_form"),
            "a form whose text says cmd: is still the form: {got:?}"
        );
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

    #[test]
    fn a_spawn_names_its_subagents_and_their_task() {
        let line = r#"{"type":"event_msg","payload":{"type":"item_completed","completed_at_ms":5,"item":{"type":"CollabAgentToolCall","tool":"spawn_agent","prompt":"Research ARTEX\nin depth","receiver_thread_ids":["01a1"],"receiver_agents":[{"thread_id":"01a1","agent_nickname":"Feynman"}]}}}"#;
        let got = spawned(line);
        assert_eq!(got.len(), 1);
        assert_eq!(
            (got[0].id.as_str(), got[0].kind.as_str(), got[0].description.as_str(), got[0].started_at),
            ("01a1", "Feynman", "Research ARTEX", 5)
        );
        assert!(spawned(&line.replace("spawn_agent", "wait")).is_empty());
        let dir = std::env::temp_dir().join(format!("desl-codex-sub-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("rollout-x-01a1.jsonl");
        std::fs::write(&f, "{\"payload\":{\"type\":\"task_started\"}}\n").unwrap();
        assert!(!finished(&f));
        std::fs::write(&f, "{\"payload\":{\"type\":\"task_started\"}}\n{\"payload\":{\"type\":\"task_complete\"}}\n").unwrap();
        assert!(finished(&f));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
