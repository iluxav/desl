//! Copilot's session files (the native route): where they are, and their lines as events.
//! `$COPILOT_HOME/session-state/<session-id>/events.jsonl`, one event per line, beside a
//! `workspace.yaml` that holds the session's directory and its name. Shapes verified against
//! GitHub Copilot CLI 1.0.112; see docs/adapters.md.

use desl_ir::adapter::{CopyItem, Reading, SessionReader, copy_items, new_uuid};
use desl_ir::{ChangeKind, EventKind, tool};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Tool output kept inline; the store moves longer text to a blob.
const OUTPUT_CAP: usize = 64 * 1024;

fn session_dir(home: &Path, id: &str) -> Option<PathBuf> {
    // An id is a UUID; anything that could leave the folder is not one.
    (!id.is_empty() && !id.contains(['/', '\\']) && !id.starts_with('.')).then(|| home.join("session-state").join(id))
}

/// A top-level scalar of a session's `workspace.yaml`: `cwd`, `name`.
fn yaml_value(path: &Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines().find_map(|l| {
        let v = l.strip_prefix(key)?.strip_prefix(':')?.trim();
        let v = match (v.chars().next(), v.chars().last()) {
            (Some('\''), Some('\'')) if v.len() >= 2 => v[1..v.len() - 1].replace("''", "'"),
            (Some('"'), Some('"')) if v.len() >= 2 => v[1..v.len() - 1].replace("\\\"", "\""),
            _ => v.to_string(),
        };
        (!v.is_empty()).then_some(v)
    })
}

/// By id; or the newest session in `cwd` begun after `since`. The file appears with the
/// session's first event, which may be after a folder-trust question.
pub fn file_in(home: &Path, cwd: &Path, session_id: Option<&str>, since: SystemTime) -> Option<(String, PathBuf)> {
    if let Some(id) = session_id {
        let p = session_dir(home, id)?.join("events.jsonl");
        return p.is_file().then(|| (id.to_string(), p));
    }
    let cwd = cwd.to_string_lossy();
    std::fs::read_dir(home.join("session-state"))
        .ok()?
        .flatten()
        .filter_map(|e| {
            let events = e.path().join("events.jsonl");
            let created = std::fs::metadata(&events).and_then(|m| m.created().or_else(|_| m.modified())).ok()?;
            (created >= since && yaml_value(&e.path().join("workspace.yaml"), "cwd")? == cwd)
                .then(|| (created, e.file_name().to_string_lossy().into_owned(), events))
        })
        .max_by_key(|(t, _, _)| *t)
        .map(|(_, id, p)| (id, p))
}

/// Copilot's own name for the session, written into `workspace.yaml` after its first turn.
pub fn title_in(home: &Path, session_id: &str) -> Option<String> {
    yaml_value(&session_dir(home, session_id)?.join("workspace.yaml"), "name")
}

fn cap(s: &str) -> String {
    if s.len() <= OUTPUT_CAP {
        return s.to_string();
    }
    let mut end = OUTPUT_CAP;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// The first file a patch names: `*** Add File: notes.txt`.
fn patch_path(patch: &str) -> String {
    patch
        .lines()
        .find_map(|l| ["*** Update File: ", "*** Add File: ", "*** Delete File: "].iter().find_map(|p| l.strip_prefix(p)))
        .unwrap_or("")
        .trim()
        .to_string()
}

/// A shell result without Copilot's own last line for the model: `<shellId: 0 completed with
/// exit code 0>`, `<exited with exit code 0>`.
fn without_shell_note(output: &str) -> &str {
    let body = output.trim_end_matches('\n');
    match body.rfind('\n').map_or((0, body), |i| (i, &body[i + 1..])) {
        (at, last) if last.starts_with('<') && last.ends_with('>') && last.contains("exit code") => &body[..at],
        _ => output,
    }
}

pub struct Reader {
    cwd: String,
    /// The model and effort last reported, to report a change once.
    settings: (Option<String>, Option<String>),
    /// The last reply asked for tools: the turn goes on when this round of it ends.
    calls_pending: bool,
}

impl Reader {
    pub fn new(cwd: &Path) -> Reader {
        Reader { cwd: cwd.to_string_lossy().into_owned(), settings: (None, None), calls_pending: false }
    }

    fn rel(&self, path: &str) -> String {
        match path.strip_prefix(&self.cwd) {
            Some(r) if r.starts_with('/') => r.trim_start_matches('/').to_string(),
            _ => path.to_string(),
        }
    }

    /// Copilot's tool call as the neutral one. Its tools differ by model: GPT models patch
    /// files with `apply_patch`, whose argument is the patch itself; others `edit` and `create`.
    fn call(&self, name: &str, args: &Value) -> (String, Value) {
        let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        match name {
            "bash" | "powershell" => (tool::BASH.into(), json!({ "command": s("command") })),
            "view" => (tool::READ.into(), json!({ "path": self.rel(&s("path")) })),
            "edit" | "create" | "str_replace_editor" => (tool::EDIT.into(), json!({ "path": self.rel(&s("path")) })),
            "apply_patch" => {
                let patch = args.as_str().map(str::to_string).unwrap_or_else(|| s("input"));
                (tool::EDIT.into(), json!({ "path": self.rel(&patch_path(&patch)) }))
            }
            "rg" | "grep" | "glob" => (tool::SEARCH.into(), json!({ "pattern": s("pattern") })),
            "web_fetch" => (tool::FETCH.into(), json!({ "url": s("url") })),
            "task" => (tool::AGENT.into(), json!({ "description": s("description") })),
            _ => (name.to_string(), if args.is_object() { args.clone() } else { json!({}) }),
        }
    }
}

impl SessionReader for Reader {
    fn line(&mut self, line: &str) -> Vec<Reading> {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return Vec::new() };
        let d = &v["data"];
        // What a subagent says and does belongs to its own conversation.
        if d.get("parentToolCallId").is_some_and(|p| !p.is_null()) {
            return Vec::new();
        }
        let text = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or("");
        match v["type"].as_str().unwrap_or("") {
            // Written when a session starts or resumes, and when the user picks in `/model`.
            "session.model_change" => {
                let get = |k: &str| d.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
                let now = (get("newModel"), get("reasoningEffort"));
                if now == self.settings || now == (None, None) {
                    return Vec::new();
                }
                self.settings = now.clone();
                vec![Reading::Settings { model: now.0, effort: now.1 }]
            }
            // Copilot writes its own records in the user's place too, each with a `source`:
            // the memories it recalls before a turn, for one.
            "user.message" => {
                let own = d.get("source").and_then(Value::as_str).is_some_and(|s| s != "user");
                if own || text("content").trim().is_empty() {
                    return Vec::new();
                }
                self.calls_pending = false;
                vec![
                    Reading::Working,
                    Reading::Event(EventKind::UserMessage { text: text("content").to_string(), attachments: Vec::new() }),
                ]
            }
            // One per model call; a turn with tool calls has several.
            "assistant.turn_start" => vec![Reading::Working],
            "assistant.message" => {
                self.calls_pending = d["toolRequests"].as_array().is_some_and(|r| !r.is_empty());
                if text("content").trim().is_empty() {
                    return Vec::new();
                }
                let model = d.get("model").and_then(Value::as_str).map(str::to_string);
                vec![Reading::Event(EventKind::AssistantMessage { text: text("content").to_string(), model })]
            }
            "tool.execution_start" => {
                let (name, args) = self.call(text("toolName"), &d["arguments"]);
                vec![Reading::Event(EventKind::ToolCall { id: text("toolCallId").to_string(), name, args })]
            }
            "tool.execution_complete" => {
                let call_id = text("toolCallId").to_string();
                let result = &d["result"];
                let output = result["content"].as_str().or_else(|| d["error"]["message"].as_str()).unwrap_or("");
                let exit_code = d["shellExecution"]["exitCode"].as_i64().and_then(|c| i32::try_from(c).ok());
                let mut out = vec![Reading::Event(EventKind::ToolResult {
                    call_id: call_id.clone(),
                    output: cap(without_shell_note(output)),
                    exit_code,
                    is_error: d["success"] == false,
                    truncated: false,
                    blob: None,
                })];
                let edits = d["fileEdits"].as_array().map(Vec::as_slice).unwrap_or_default();
                // The diff covers the whole call, so it belongs to the file only when there is one.
                let diff = result["detailedContent"].as_str().filter(|t| edits.len() == 1 && t.contains("diff --git"));
                for e in edits {
                    let Some(path) = e["path"].as_str() else { continue };
                    out.push(Reading::Event(EventKind::FileChange {
                        path: self.rel(path),
                        diff: diff.map(|t| t.trim().to_string()),
                        change: match e["kind"].as_str() {
                            Some("create") => ChangeKind::Create,
                            Some("delete") => ChangeKind::Delete,
                            _ => ChangeKind::Modify,
                        },
                        call_id: Some(call_id.clone()),
                    }));
                }
                out
            }
            // No line says "the turn is over". A model call that asked for no tools is its last.
            "assistant.turn_end" if !self.calls_pending => vec![Reading::Idle],
            // `ctrl+c`, or `esc` twice: the call in flight gets no result.
            "abort" => {
                self.calls_pending = false;
                vec![Reading::Event(EventKind::Interrupt), Reading::Idle]
            }
            // Written once after every turn, however it ended, and when Copilot quits.
            "session.usage_checkpoint" | "session.shutdown" => {
                self.calls_pending = false;
                vec![Reading::Idle]
            }
            _ => Vec::new(),
        }
    }
}

/// Copilot's own questions at the bottom of its screen: a tool approval or the folder-trust
/// question (a numbered list under `enter to select`), or the form its `ask_user` tool opens.
pub fn waiting(tail: &str) -> bool {
    tail.contains("1. Yes") && tail.contains("enter to select")
        || tail.contains("Copilot needs information")
        || tail.contains("enter accept") && tail.contains("ctrl+d decline")
}

/// Copilot's own tool for a neutral call; None when it has none that replays safely.
fn native_tool(cwd: &Path, name: &str, args: &Value) -> Option<(&'static str, Value)> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let abs = |p: String| if p.starts_with('/') { p } else { cwd.join(p).to_string_lossy().into_owned() };
    match name {
        tool::BASH => Some(("bash", json!({ "command": s("command"), "description": desl_ir::tool_target(name, args) }))),
        tool::READ => Some(("view", json!({ "path": abs(s("path")) }))),
        tool::SEARCH if !s("pattern").is_empty() => Some(("rg", json!({ "pattern": s("pattern") }))),
        tool::FETCH => Some(("web_fetch", json!({ "url": s("url") }))),
        _ => None,
    }
}

/// A call Copilot has no tool for, told as the assistant's own words.
fn told(name: &str, args: &Value, output: &str, diff: Option<&str>) -> String {
    let target = desl_ir::tool_target(name, args);
    match (name, diff) {
        (tool::EDIT, Some(d)) => format!("I edited {target}:\n```diff\n{}\n```", d.trim_end()),
        (tool::EDIT, None) => format!("I edited {target}."),
        _ if output.trim().is_empty() => format!("[{name} {target}]"),
        _ => format!("[{name} {target}]\n{}", output.trim_end()),
    }
}

/// An id Copilot's models accept for a call they did not make: `call_` and letters and digits.
fn call_id(id: &str) -> String {
    let alnum: String = id.chars().filter(char::is_ascii_alphanumeric).collect();
    if alnum.is_empty() {
        format!("call_{}", &new_uuid().replace('-', "")[..24])
    } else {
        format!("call_{}", &alnum[alnum.len().saturating_sub(24)..])
    }
}

/// A new session holding `events`, written the way Copilot writes its own: a `session.start`
/// header, then each prompt and what followed it as model calls of one interaction, every
/// event naming the one before it. Copilot adds `session.resume` when it opens the session and
/// draws the history. Returns the id, the event log and its length.
pub fn write_in(home: &Path, cwd: &Path, events: &[EventKind]) -> Result<(String, PathBuf, u64), String> {
    let id = new_uuid();
    let dir = home.join("session-state").join(&id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create {} ({e})", dir.display()))?;
    let cwd_s = cwd.to_string_lossy().into_owned();
    let now = || chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let mut out = String::new();
    let mut parent = Value::Null;
    let mut push = |kind: &str, data: Value, out: &mut String| {
        let event_id = new_uuid();
        out.push_str(&json!({ "type": kind, "data": data, "id": event_id, "timestamp": now(), "parentId": parent }).to_string());
        out.push('\n');
        parent = Value::String(event_id);
    };
    push(
        "session.start",
        json!({ "sessionId": id, "version": 1, "producer": "copilot-agent", "copilotVersion": crate::FIXTURES_VERSION,
                "startTime": now(), "contextTier": null, "context": { "cwd": cwd_s }, "alreadyInUse": false, "remoteSteerable": false }),
        &mut out,
    );
    // The interaction a prompt opens, the prompt's own id, and the model call within it.
    let mut turn: (String, String, u32) = (new_uuid(), new_uuid(), 0);
    for item in copy_items(events) {
        if let CopyItem::User(text) = item {
            turn = (new_uuid(), new_uuid(), 0);
            push("user.message", json!({ "content": text, "messageId": turn.1, "interactionId": turn.0, "turnId": "0" }), &mut out);
            continue;
        }
        let (interaction, prompt, n) = (turn.0.clone(), turn.1.clone(), turn.2.to_string());
        let message = |content: &str, requests: Value| {
            json!({ "messageId": new_uuid(), "originatingMessageId": prompt, "content": content, "toolRequests": requests,
                    "interactionId": interaction, "turnId": n })
        };
        push("assistant.turn_start", json!({ "turnId": n, "interactionId": interaction, "agentMode": "interactive" }), &mut out);
        match item {
            CopyItem::User(_) => {}
            CopyItem::Assistant(text) => push("assistant.message", message(&text, json!([])), &mut out),
            CopyItem::Call { id: from, name, args, output, is_error, diff } => match native_tool(cwd, &name, &args) {
                Some((tool_name, arguments)) => {
                    let call = call_id(&from);
                    let request = json!([{ "toolCallId": call, "name": tool_name, "arguments": arguments, "type": "function" }]);
                    push("assistant.message", message("", request), &mut out);
                    push(
                        "tool.execution_start",
                        json!({ "toolCallId": call, "toolName": tool_name, "arguments": arguments, "turnId": n }),
                        &mut out,
                    );
                    push(
                        "tool.execution_complete",
                        json!({ "toolCallId": call, "interactionId": interaction, "turnId": n, "success": !is_error,
                                "result": { "content": output } }),
                        &mut out,
                    );
                }
                None => push("assistant.message", message(&told(&name, &args, &output, diff.as_deref()), json!([])), &mut out),
            },
        }
        push("assistant.turn_end", json!({ "turnId": n }), &mut out);
        turn.2 += 1;
    }
    let path = dir.join("events.jsonl");
    std::fs::write(&path, &out).map_err(|e| format!("Could not write {} ({e})", path.display()))?;
    let t = now();
    let meta = format!(
        "id: {id}\ncwd: {}\nclient_name: github/cli\nlast_client_name: github/cli\nuser_named: false\nsummary_count: 0\nfork_count: 0\ncreated_at: {t}\nupdated_at: {t}\n",
        yaml_scalar(&cwd_s)
    );
    std::fs::write(dir.join("workspace.yaml"), meta).map_err(|e| format!("Could not write {} ({e})", dir.display()))?;
    Ok((id, path, out.len() as u64))
}

/// A path as a YAML scalar: plain when it can be, single-quoted when a character would not
/// read back as part of it.
fn yaml_scalar(s: &str) -> String {
    let plain = s.starts_with('/') && !s.contains(": ") && !s.contains(" #") && !s.ends_with(':') && !s.contains(['\n', '\'', '"']);
    if plain { s.to_string() } else { format!("'{}'", s.replace('\'', "''")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(lines: &[&str]) -> Vec<Reading> {
        let mut r = Reader::new(Path::new("/tmp/demo"));
        lines.iter().flat_map(|l| r.line(l)).collect()
    }

    fn names(got: &[Reading]) -> Vec<String> {
        got.iter()
            .map(|r| match r {
                Reading::Event(e) => e.name().to_string(),
                Reading::Working => "working".into(),
                Reading::Idle => "idle".into(),
                Reading::Settings { .. } => "settings".into(),
                Reading::Title(_) => "title".into(),
            })
            .collect()
    }

    #[test]
    fn a_turn_with_a_tool_call_ends_after_the_reply_not_after_the_call() {
        let got = read(&[
            r#"{"type":"session.start","data":{"sessionId":"s1","context":{"cwd":"/tmp/demo"}},"id":"e1","parentId":null}"#,
            r#"{"type":"user.message","data":{"content":"","transformedContent":"<memories>\nNone.\n</memories>","source":"memory"},"id":"e2"}"#,
            r#"{"type":"user.message","data":{"content":"Run cat notes.txt","messageId":"m1","interactionId":"i1","turnId":"0"},"id":"e3"}"#,
            r#"{"type":"assistant.turn_start","data":{"turnId":"0","interactionId":"i1","agentMode":"interactive"},"id":"e4"}"#,
            r#"{"type":"assistant.message","data":{"content":"","model":"gpt-6-luna","toolRequests":[{"toolCallId":"call_1","name":"bash","arguments":{"command":"cat notes.txt"},"type":"function"}],"turnId":"0"},"id":"e5"}"#,
            r#"{"type":"tool.execution_start","data":{"toolCallId":"call_1","toolName":"bash","arguments":{"command":"cat notes.txt","description":"Read the notes"},"turnId":"0"},"id":"e6"}"#,
            r#"{"type":"tool.execution_complete","data":{"toolCallId":"call_1","turnId":"0","shellExecution":{"exitCode":0},"success":true,"result":{"content":"deadline: Friday\n<shellId: 0 completed with exit code 0>"}},"id":"e7"}"#,
            r#"{"type":"assistant.turn_end","data":{"turnId":"0"},"id":"e8"}"#,
            r#"{"type":"assistant.turn_start","data":{"turnId":"1","interactionId":"i1","agentMode":"interactive"},"id":"e9"}"#,
            r#"{"type":"assistant.message","data":{"content":"The deadline is Friday.","model":"gpt-6-luna","toolRequests":[],"turnId":"1","phase":"final_answer"},"id":"e10"}"#,
            r#"{"type":"assistant.turn_end","data":{"turnId":"1"},"id":"e11"}"#,
            r#"{"type":"session.usage_checkpoint","data":{"totalPremiumRequests":1},"id":"e12"}"#,
        ]);
        assert_eq!(
            names(&got),
            ["working", "user_message", "working", "tool_call", "tool_result", "working", "assistant_message", "idle", "idle"],
            "the memories Copilot recalls are not a prompt, and the first model call's end is not the turn's"
        );
        assert!(
            matches!(&got[3], Reading::Event(EventKind::ToolCall { name, args, .. }) if name == "bash" && args["command"] == "cat notes.txt")
        );
        assert!(
            matches!(&got[4], Reading::Event(EventKind::ToolResult { output, exit_code, is_error, .. }) if output == "deadline: Friday" && *exit_code == Some(0) && !is_error)
        );
        assert!(matches!(&got[6], Reading::Event(EventKind::AssistantMessage { model, .. }) if model.as_deref() == Some("gpt-6-luna")));
    }

    #[test]
    fn a_patch_is_an_edit_of_the_file_it_names_with_its_diff() {
        let got = read(&[
            r#"{"type":"tool.execution_start","data":{"toolCallId":"custom_call_2","toolName":"apply_patch","arguments":"*** Begin Patch\n*** Add File: hello.txt\n+hi\n*** End Patch\n","turnId":"0"}}"#,
            r#"{"type":"permission.requested","data":{"requestId":"r1","permissionRequest":{"kind":"write","toolCallId":"custom_call_2"}}}"#,
            r#"{"type":"permission.completed","data":{"requestId":"r1","toolCallId":"custom_call_2","result":{"kind":"approved"}}}"#,
            r#"{"type":"tool.execution_complete","data":{"toolCallId":"custom_call_2","fileEdits":[{"path":"/tmp/demo/hello.txt","kind":"create"}],"success":true,"result":{"content":"Added 1 file(s): /tmp/demo/hello.txt","detailedContent":"\ndiff --git a/tmp/demo/hello.txt b/tmp/demo/hello.txt\n--- /dev/null\n+++ b/tmp/demo/hello.txt\n@@ -0,0 +1 @@\n+hi\n"}}}"#,
            r#"{"type":"tool.execution_start","data":{"toolCallId":"call_3","toolName":"view","arguments":{"path":"/tmp/demo/src/main.rs"}}}"#,
        ]);
        assert_eq!(names(&got), ["tool_call", "tool_result", "file_change", "tool_call"]);
        assert!(matches!(&got[0], Reading::Event(EventKind::ToolCall { name, args, .. }) if name == "edit" && args["path"] == "hello.txt"));
        assert!(
            matches!(&got[2], Reading::Event(EventKind::FileChange { path, diff: Some(d), change: ChangeKind::Create, call_id: Some(c) })
            if path == "hello.txt" && d.starts_with("diff --git") && c == "custom_call_2")
        );
        assert!(
            matches!(&got[3], Reading::Event(EventKind::ToolCall { name, args, .. }) if name == "read" && args["path"] == "src/main.rs")
        );
    }

    #[test]
    fn an_interrupt_ends_the_turn_though_its_call_never_finished() {
        let got = read(&[
            r#"{"type":"user.message","data":{"content":"Run sleep 60","messageId":"m1","turnId":"0"}}"#,
            r#"{"type":"assistant.turn_start","data":{"turnId":"0"}}"#,
            r#"{"type":"assistant.message","data":{"content":"","toolRequests":[{"toolCallId":"call_1","name":"bash","arguments":{"command":"sleep 60"}}],"turnId":"0"}}"#,
            r#"{"type":"tool.execution_start","data":{"toolCallId":"call_1","toolName":"bash","arguments":{"command":"sleep 60"}}}"#,
            r#"{"type":"assistant.turn_end","data":{"turnId":"0"}}"#,
            r#"{"type":"abort","data":{"reason":"user_initiated"}}"#,
        ]);
        assert_eq!(names(&got), ["working", "user_message", "working", "tool_call", "interrupt", "idle"]);
    }

    #[test]
    fn a_model_picked_in_copilot_is_reported_once() {
        let got = read(&[
            r#"{"type":"session.model_change","data":{"cause":"initial_resolution","source":"automatic","newModel":"gpt-5.6-terra","reasoningEffort":"medium"}}"#,
            r#"{"type":"session.resume","data":{"eventCount":10}}"#,
            r#"{"type":"session.model_change","data":{"cause":"initial_resolution","source":"automatic","newModel":"gpt-5.6-terra","reasoningEffort":"medium"}}"#,
            r#"{"type":"session.model_change","data":{"newModel":"claude-haiku-4.5","reasoningEffort":null}}"#,
        ]);
        assert_eq!(
            got,
            vec![
                Reading::Settings { model: Some("gpt-5.6-terra".into()), effort: Some("medium".into()) },
                Reading::Settings { model: Some("claude-haiku-4.5".into()), effort: None },
            ]
        );
    }

    #[test]
    fn its_dialogs_and_its_form_are_questions_and_its_prompt_is_not() {
        let approval = "│ Do you want to run this command?\n│\n│ ❯ 1. Yes\n│   2. Yes, and don't ask again for `sleep` in this repo\n│   3. No, and tell Copilot what to do differently (Esc to stop)\n│\n│ ↑/↓ to navigate · enter to select · esc to cancel";
        let trust = "│ Do you trust the files in this folder?\n│\n│ ❯ 1. Yes\n│   2. Yes, and remember this folder for future sessions\n│   3. No (Esc)\n│\n│ ↑/↓ to navigate · enter to select · esc to cancel";
        let form = " Copilot needs information.\n Which color do you prefer?\n\n ❯ red\n   blue\n   Other (type your answer)\n\n ↑/↓ select · enter accept · ctrl+d decline · esc cancel";
        let idle = " ● 1. Yes, the deadline is Friday.\n\n❯\n ← open sidebar · Interactive · Manual Approval · / commands · ? help";
        assert!(waiting(approval) && waiting(trust) && waiting(form));
        assert!(!waiting(idle));
    }

    #[test]
    fn a_written_session_reads_back_as_the_same_conversation_and_is_found_by_its_id() {
        let home = std::env::temp_dir().join(format!("desl-copilot-write-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let events = vec![
            EventKind::UserMessage { text: "Read notes.txt".into(), attachments: vec![] },
            EventKind::ToolCall { id: "toolu_9".into(), name: "read".into(), args: json!({ "path": "notes.txt" }) },
            EventKind::ToolResult {
                call_id: "toolu_9".into(),
                output: "deadline: Friday".into(),
                exit_code: None,
                is_error: false,
                truncated: false,
                blob: None,
            },
            EventKind::ToolCall { id: "toolu_10".into(), name: "edit".into(), args: json!({ "path": "net.py" }) },
            EventKind::ToolResult {
                call_id: "toolu_10".into(),
                output: "ok".into(),
                exit_code: None,
                is_error: false,
                truncated: false,
                blob: None,
            },
            EventKind::AssistantMessage { text: "The deadline is Friday.".into(), model: None },
            EventKind::UserMessage { text: "Thanks".into(), attachments: vec![] },
        ];
        let before = SystemTime::now() - std::time::Duration::from_secs(5);
        let (id, path, len) = write_in(&home, Path::new("/tmp/demo"), &events).unwrap();
        assert!(path.ends_with(format!("session-state/{id}/events.jsonl")));
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.len() as u64, len);
        // Every event hangs off the one before it, as Copilot's own do.
        let lines: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines[0]["type"], "session.start");
        assert_eq!(lines[0]["data"]["sessionId"], id.as_str());
        for w in lines.windows(2) {
            assert_eq!(w[1]["parentId"], w[0]["id"]);
        }
        let mut r = Reader::new(Path::new("/tmp/demo"));
        let back: Vec<Reading> = text.lines().flat_map(|l| r.line(l)).collect();
        let kinds: Vec<&EventKind> = back
            .iter()
            .filter_map(|x| match x {
                Reading::Event(e) => Some(e),
                _ => None,
            })
            .collect();
        assert!(matches!(kinds[0], EventKind::UserMessage { text, .. } if text == "Read notes.txt"));
        assert!(matches!(kinds[1], EventKind::ToolCall { name, args, .. } if name == "read" && args["path"] == "notes.txt"));
        assert!(matches!(kinds[2], EventKind::ToolResult { output, .. } if output == "deadline: Friday"));
        assert!(matches!(kinds[3], EventKind::AssistantMessage { text, .. } if text == "I edited net.py."));
        assert!(matches!(kinds[4], EventKind::AssistantMessage { text, .. } if text == "The deadline is Friday."));
        assert!(matches!(kinds[5], EventKind::UserMessage { text, .. } if text == "Thanks"));
        // The copy ends on a prompt nobody answered: the thread reads as working until Copilot does.
        assert_eq!(
            back.iter().filter(|x| matches!(x, Reading::Idle)).count(),
            2,
            "the edit told in words and the reply each end a model call"
        );
        assert_eq!(file_in(&home, Path::new("/tmp/demo"), Some(&id), SystemTime::now()), Some((id.clone(), path.clone())));
        assert_eq!(file_in(&home, Path::new("/tmp/demo"), None, before).map(|(i, _)| i), Some(id.clone()), "found by its directory too");
        assert_eq!(file_in(&home, Path::new("/tmp/other"), None, before), None);
        assert_eq!(file_in(&home, Path::new("/tmp/demo"), Some("../x"), before), None);
        assert_eq!(title_in(&home, &id), None, "Copilot names it after its first turn");
        std::fs::write(
            home.join("session-state").join(&id).join("workspace.yaml"),
            "id: x\ncwd: /tmp/demo\nname: 'Read the team''s notes'\n",
        )
        .unwrap();
        assert_eq!(title_in(&home, &id).as_deref(), Some("Read the team's notes"));
        let _ = std::fs::remove_dir_all(&home);
    }
}
