//! Reads either agent's session file into one neutral list, and writes that list out as a
//! brand-new session of either agent. Throwaway: the findings are in `docs/adapters.md`.

use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Claude,
    Codex,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Claude => "claude",
            Kind::Codex => "codex",
        }
    }
    pub fn other(self) -> Kind {
        match self {
            Kind::Claude => Kind::Codex,
            Kind::Codex => Kind::Claude,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Item {
    User(String),
    Assistant(String),
    /// A tool call in Claude's terms: tool name and input, with its id.
    Call { id: String, tool: String, input: Value },
    Result { id: String, text: String },
}

const CAP: usize = 4000;

fn cap(s: &str) -> String {
    if s.len() <= CAP {
        return s.to_string();
    }
    let mut end = CAP;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[truncated]", &s[..end])
}

pub fn uuid() -> String {
    fs::read_to_string("/proc/sys/kernel/random/uuid").map(|s| s.trim().to_string()).unwrap_or_else(|_| {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
        format!("{:08x}-0000-4000-8000-{:012x}", (n >> 64) as u32, n as u64 & 0xffff_ffff_ffff)
    })
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().filter_map(|x| x.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join(""),
        _ => String::new(),
    }
}

fn is_tagged(s: &str) -> bool {
    let t = s.trim_start();
    t.starts_with('<') || t.starts_with("# AGENTS.md")
}

// ---------- locating session files ----------

pub fn claude_dir(cwd: &Path) -> PathBuf {
    let slug: String = cwd.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    home().join(".claude/projects").join(slug)
}

pub fn claude_file(cwd: &Path, id: &str) -> Option<PathBuf> {
    let p = claude_dir(cwd).join(format!("{id}.jsonl"));
    if p.exists() {
        return Some(p);
    }
    let root = home().join(".claude/projects");
    fs::read_dir(root).ok()?.flatten().map(|d| d.path().join(format!("{id}.jsonl"))).find(|p| p.exists())
}

fn codex_rollouts() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![home().join(".codex/sessions")];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
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

pub fn codex_file(id: &str) -> Option<PathBuf> {
    codex_rollouts().into_iter().find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().contains(id)))
}

/// The newest Codex session for this directory begun after `since`: how a session Codex
/// started on its own (no id handed to it) is found.
pub fn codex_find_new(cwd: &Path, since: std::time::SystemTime) -> Option<(String, PathBuf)> {
    let mut best: Option<(std::time::SystemTime, String, PathBuf)> = None;
    for p in codex_rollouts() {
        let Ok(m) = fs::metadata(&p).and_then(|m| m.modified()) else { continue };
        if m < since {
            continue;
        }
        let Ok(first) = fs::read_to_string(&p).map(|s| s.lines().next().unwrap_or("").to_string()) else { continue };
        let Ok(d) = serde_json::from_str::<Value>(&first) else { continue };
        let pl = &d["payload"];
        if pl["cwd"].as_str() != Some(&*cwd.to_string_lossy()) {
            continue;
        }
        let id = pl["id"].as_str().unwrap_or_default().to_string();
        if best.as_ref().is_none_or(|b| m > b.0) {
            best = Some((m, id, p));
        }
    }
    best.map(|(_, id, p)| (id, p))
}

// ---------- reading ----------

pub fn read_claude(path: &Path) -> Result<Vec<Item>> {
    let mut out = Vec::new();
    for line in fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?.lines() {
        let Ok(d) = serde_json::from_str::<Value>(line) else { continue };
        if d["isSidechain"].as_bool() == Some(true) {
            continue;
        }
        let c = &d["message"]["content"];
        match d["type"].as_str() {
            Some("user") => {
                if let Some(s) = c.as_str() {
                    if !is_tagged(s) {
                        out.push(Item::User(s.to_string()));
                    }
                    continue;
                }
                for b in c.as_array().into_iter().flatten() {
                    match b["type"].as_str() {
                        Some("tool_result") => out.push(Item::Result {
                            id: b["tool_use_id"].as_str().unwrap_or_default().to_string(),
                            text: cap(&text_of(&b["content"])),
                        }),
                        Some("text") => {
                            let t = b["text"].as_str().unwrap_or_default();
                            if !is_tagged(t) && !t.starts_with("[Request interrupted") {
                                out.push(Item::User(t.to_string()));
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some("assistant") => {
                for b in c.as_array().into_iter().flatten() {
                    match b["type"].as_str() {
                        Some("text") => out.push(Item::Assistant(b["text"].as_str().unwrap_or_default().to_string())),
                        Some("tool_use") => out.push(Item::Call {
                            id: b["id"].as_str().unwrap_or_default().to_string(),
                            tool: b["name"].as_str().unwrap_or_default().to_string(),
                            input: b["input"].clone(),
                        }),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// `tools.exec_command({cmd:"..."})` inside Codex's `exec` script, as a shell command.
fn exec_cmd(script: &str) -> Option<String> {
    let at = script.find("exec_command(")?;
    let rest = &script[at..];
    let k = rest.find("cmd:")? + 4;
    let s = rest[k..].trim_start();
    let mut de = serde_json::Deserializer::from_str(s).into_iter::<String>();
    de.next()?.ok()
}

fn apply_patch_arg(script: &str) -> Option<String> {
    let at = script.find("apply_patch(")? + "apply_patch(".len();
    let s = script[at..].trim_start();
    let mut de = serde_json::Deserializer::from_str(s).into_iter::<String>();
    de.next()?.ok()
}

pub fn read_codex(path: &Path) -> Result<Vec<Item>> {
    let mut out = Vec::new();
    for line in fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?.lines() {
        let Ok(d) = serde_json::from_str::<Value>(line) else { continue };
        if d["type"] != "response_item" {
            continue;
        }
        let p = &d["payload"];
        match (p["type"].as_str(), p["role"].as_str()) {
            (Some("message"), Some("user")) => {
                let t = text_of(&p["content"]);
                if !is_tagged(&t) {
                    out.push(Item::User(t.trim_end_matches('\n').to_string()));
                }
            }
            (Some("message"), Some("assistant")) => out.push(Item::Assistant(text_of(&p["content"]))),
            (Some("custom_tool_call"), _) => {
                let id = p["call_id"].as_str().unwrap_or_default().to_string();
                let script = p["input"].as_str().unwrap_or_default();
                let (tool, input) = if let Some(cmd) = exec_cmd(script) {
                    ("Bash".to_string(), json!({ "command": cmd }))
                } else if let Some(patch) = apply_patch_arg(script) {
                    ("Bash".to_string(), json!({ "command": format!("apply_patch <<'PATCH'\n{patch}\nPATCH") }))
                } else {
                    (p["name"].as_str().unwrap_or("exec").to_string(), json!({ "script": script }))
                };
                out.push(Item::Call { id, tool, input });
            }
            (Some("function_call"), _) => {
                let args = p["arguments"].as_str().and_then(|a| serde_json::from_str(a).ok()).unwrap_or(json!({}));
                out.push(Item::Call {
                    id: p["call_id"].as_str().unwrap_or_default().to_string(),
                    tool: p["name"].as_str().unwrap_or_default().to_string(),
                    input: args,
                });
            }
            (Some("custom_tool_call_output" | "function_call_output"), _) => {
                let o = &p["output"];
                let text = if o.is_string() { o.as_str().unwrap_or_default().to_string() } else { text_of(o) };
                out.push(Item::Result { id: p["call_id"].as_str().unwrap_or_default().to_string(), text: cap(&text) });
            }
            _ => {}
        }
    }
    Ok(out)
}

pub fn read(kind: Kind, path: &Path) -> Result<Vec<Item>> {
    match kind {
        Kind::Claude => read_claude(path),
        Kind::Codex => read_codex(path),
    }
}

/// Calls the receiving agent has no tool for become words, with their output, in the
/// assistant's own message.
fn fold_unknown(items: &[Item], known: impl Fn(&str) -> bool) -> Vec<Item> {
    let mut out = Vec::new();
    let mut folded: Vec<String> = Vec::new();
    for i in items {
        match i {
            Item::Call { id, tool, input } if !known(tool) => {
                folded.push(id.clone());
                let res = items.iter().find_map(|r| match r {
                    Item::Result { id: rid, text } if rid == id => Some(text.clone()),
                    _ => None,
                });
                out.push(Item::Assistant(format!("[used {tool} {input}]\n{}", res.unwrap_or_default())));
            }
            Item::Result { id, .. } if folded.contains(id) => {}
            other => out.push(other.clone()),
        }
    }
    out
}

// ---------- writing a new Claude session ----------

const CLAUDE_TOOLS: &[&str] = &["Bash", "Read", "Edit", "Write", "Glob", "Grep"];

fn toolu(id: &str) -> String {
    if id.starts_with("toolu_") {
        return id.to_string();
    }
    let alnum: String = id.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    format!("toolu_{}", &alnum[alnum.len().saturating_sub(24)..])
}

pub fn write_claude(items: &[Item], cwd: &Path) -> Result<String> {
    let items = fold_unknown(items, |t| CLAUDE_TOOLS.contains(&t));
    let sid = uuid();
    let dir = claude_dir(cwd);
    fs::create_dir_all(&dir)?;
    let mut f = fs::File::create(dir.join(format!("{sid}.jsonl")))?;
    let mut parent = Value::Null;
    for i in &items {
        let u = uuid();
        let mut line = json!({
            "parentUuid": parent, "isSidechain": false, "userType": "external",
            "cwd": cwd.to_string_lossy(), "sessionId": sid, "uuid": u, "timestamp": now(),
        });
        let msg = |content: Value, stop: &str| {
            json!({ "id": format!("msg_{}", uuid().replace('-', "")), "type": "message", "role": "assistant",
                    "model": "claude-opus-5-5", "content": content, "stop_reason": stop, "stop_sequence": null,
                    "usage": { "input_tokens": 0, "output_tokens": 0 } })
        };
        match i {
            Item::User(t) => {
                line["type"] = "user".into();
                line["message"] = json!({ "role": "user", "content": t });
            }
            Item::Assistant(t) => {
                line["type"] = "assistant".into();
                line["message"] = msg(json!([{ "type": "text", "text": t }]), "end_turn");
            }
            Item::Call { id, tool, input } => {
                line["type"] = "assistant".into();
                line["message"] = msg(json!([{ "type": "tool_use", "id": toolu(id), "name": tool, "input": input }]), "tool_use");
            }
            Item::Result { id, text } => {
                line["type"] = "user".into();
                line["message"] = json!({ "role": "user", "content": [{ "type": "tool_result", "tool_use_id": toolu(id), "content": text }] });
            }
        }
        writeln!(f, "{line}")?;
        parent = Value::String(u);
    }
    Ok(sid)
}

// ---------- writing a new Codex session ----------

fn shell_for(tool: &str, input: &Value) -> Option<String> {
    let s = |k: &str| input[k].as_str().unwrap_or_default().to_string();
    match tool {
        "Bash" => Some(s("command")),
        "Read" => Some(format!("cat {}", s("file_path"))),
        "Grep" => Some(format!("rg {} {}", s("pattern"), s("path"))),
        "Glob" => Some(format!("rg --files -g '{}'", s("pattern"))),
        _ => None,
    }
}

fn patch_for(tool: &str, input: &Value) -> Option<String> {
    let s = |k: &str| input[k].as_str().unwrap_or_default().to_string();
    let lines = |t: &str, sign: char| t.split('\n').map(|l| format!("{sign}{l}\n")).collect::<String>();
    match tool {
        "Edit" => Some(format!(
            "*** Begin Patch\n*** Update File: {}\n@@\n{}{}*** End Patch",
            s("file_path"),
            lines(&s("old_string"), '-'),
            lines(&s("new_string"), '+')
        )),
        "Write" => Some(format!("*** Begin Patch\n*** Add File: {}\n{}*** End Patch", s("file_path"), lines(&s("content"), '+'))),
        _ => None,
    }
}

pub fn write_codex(items: &[Item], cwd: &Path) -> Result<String> {
    let items = fold_unknown(items, |t| shell_for(t, &json!({})).is_some() || patch_for(t, &json!({})).is_some());
    let sid = uuid();
    let t = chrono::Utc::now();
    let dir = home().join(".codex/sessions").join(t.format("%Y/%m/%d").to_string());
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("rollout-{}-{sid}.jsonl", t.format("%Y-%m-%dT%H-%M-%S")));
    let cwd_s = cwd.to_string_lossy().to_string();
    let ms = t.timestamp_millis();
    let mut lines: Vec<Value> = vec![json!({ "timestamp": now(), "type": "session_meta", "payload": {
        "session_id": sid, "id": sid, "timestamp": now(), "cwd": cwd_s, "originator": "codex-tui",
        "cli_version": "0.166.0", "source": "cli", "thread_source": "user", "model_provider": "openai",
        "history_mode": "paginated" } })];
    let ev = |turn: &str, item: Value| {
        json!({ "timestamp": now(), "type": "event_msg", "payload": { "type": "item_completed", "thread_id": sid,
                "turn_id": turn, "item": item, "started_at_ms": ms, "completed_at_ms": ms } })
    };
    let ri = |turn: &str, mut p: Value| {
        p["internal_chat_message_metadata_passthrough"] = json!({ "turn_id": turn });
        json!({ "timestamp": now(), "type": "response_item", "payload": p })
    };
    // One turn per user message, bracketed the way Codex writes its own.
    let mut turn: Option<(String, String)> = None; // (turn id, last agent message)
    let close = |lines: &mut Vec<Value>, turn: &Option<(String, String)>| {
        if let Some((id, last)) = turn {
            lines.push(json!({ "timestamp": now(), "type": "event_msg", "payload": { "type": "task_complete",
                "root_turn_id": id, "turn_id": id, "last_agent_message": last, "started_at": ms / 1000,
                "completed_at": ms / 1000, "duration_ms": 0, "time_to_first_token_ms": 0 } }));
        }
    };
    let open = |lines: &mut Vec<Value>| {
        let id = uuid();
        lines.push(json!({ "timestamp": now(), "type": "event_msg", "payload": { "type": "task_started",
            "turn_attribution": { "turn_id": id, "turn_trigger": "user", "parent_turn_id": null,
                                  "initiating_agent_path": null, "root_turn_id": id },
            "turn_id": id, "root_turn_id": id, "started_at": ms / 1000, "model_context_window": 258400,
            "collaboration_mode_kind": "default" } }));
        id
    };
    for (k, i) in items.iter().enumerate() {
        if matches!(i, Item::User(_)) || turn.is_none() {
            close(&mut lines, &turn);
            turn = Some((open(&mut lines), String::new()));
        }
        let tid = turn.as_ref().map(|t| t.0.clone()).unwrap_or_default();
        match i {
            Item::User(t) => {
                lines.push(ri(&tid, json!({ "type": "message", "id": format!("msg_{}", uuid()), "role": "user",
                    "content": [{ "type": "input_text", "text": t }] })));
                lines.push(ev(&tid, json!({ "type": "UserMessage", "id": uuid(),
                    "content": [{ "type": "text", "text": t, "text_elements": [] }] })));
            }
            Item::Assistant(t) => {
                lines.push(ri(&tid, json!({ "type": "message", "status": "completed", "role": "assistant",
                    "content": [{ "type": "output_text", "text": t, "annotations": [], "logprobs": [] }],
                    "phase": "final_answer" })));
                lines.push(ev(&tid, json!({ "type": "AgentMessage", "id": format!("msg_{}", uuid().replace('-', "")),
                    "content": [{ "type": "Text", "text": t }], "phase": "final_answer" })));
                if let Some(tt) = turn.as_mut() {
                    tt.1 = t.clone();
                }
            }
            Item::Call { id, tool, input } => {
                let js = if let Some(cmd) = shell_for(tool, input) {
                    format!("const r = await tools.exec_command({{cmd:{}}});\ntext(r.output);\n", json!(cmd))
                } else if let Some(patch) = patch_for(tool, input) {
                    format!("const r = await tools.apply_patch({});\ntext(r);\n", json!(patch))
                } else {
                    format!("// {tool} {input}\n")
                };
                lines.push(ri(&tid, json!({ "type": "custom_tool_call", "status": "completed", "call_id": id,
                    "name": "exec", "input": js })));
                // What Codex's own screen shows for a command, with its output.
                let output = items[k..].iter().find_map(|r| match r {
                    Item::Result { id: rid, text } if rid == id => Some(text.clone()),
                    _ => None,
                });
                let shown = shell_for(tool, input).unwrap_or_else(|| format!("apply_patch {}", input["file_path"].as_str().unwrap_or("")));
                lines.push(ev(&tid, json!({ "type": "CommandExecution", "id": format!("exec-{}", uuid()),
                    "command": ["/usr/bin/bash", "-lc", shown], "cwd": format!("file://{cwd_s}"),
                    "parsed_cmd": [{ "type": "unknown", "cmd": shown }], "source": "unified_exec_startup",
                    "status": "completed", "aggregated_output": output.unwrap_or_default(), "exit_code": 0,
                    "duration": { "secs": 0, "nanos": 0 } })));
            }
            Item::Result { id, text } => {
                lines.push(ri(&tid, json!({ "type": "custom_tool_call_output", "id": format!("ctco_{}", uuid()), "call_id": id,
                    "output": [{ "type": "input_text", "text": "Script completed\nOutput:\n" }, { "type": "input_text", "text": text }] })));
            }
        }
    }
    close(&mut lines, &turn);
    let mut f = fs::File::create(&path)?;
    for (n, mut l) in lines.into_iter().enumerate() {
        l["ordinal"] = json!(n);
        writeln!(f, "{l}")?;
    }
    Ok(sid)
}

pub fn write(kind: Kind, items: &[Item], cwd: &Path) -> Result<String> {
    match kind {
        Kind::Claude => write_claude(items, cwd),
        Kind::Codex => write_codex(items, cwd),
    }
}
