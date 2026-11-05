//! Drives `codex exec --json` (and `codex exec resume <id> --json`) one turn per process.
//! Flags verified against Codex CLI 0.165.0 on 2026-11-04; see docs/adapters.md.
//!
//! Findings that shape this adapter: agent messages arrive whole (no token deltas), there is no
//! approval channel in exec mode (the sandbox policy decides), and the process exits after each turn.

use super::{AgentEvent, AgentMsg};
use crate::diff;
use crate::model::Agent;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct CodexProcess {
    cwd: PathBuf,
    tx: Sender<AgentMsg>,
    thread_idx: usize,
    pub session_id: Option<String>,
    child: Option<Arc<Mutex<Child>>>,
}

/// The model name from the user's own config, for the status line only.
fn configured_model() -> String {
    let path = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex")))
        .map(|d| d.join("config.toml"));
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| {
            s.lines().find_map(|l| {
                let l = l.trim();
                l.strip_prefix("model")
                    .map(|r| r.trim_start())
                    .and_then(|r| r.strip_prefix('='))
                    .map(|r| r.trim().trim_matches('"').to_string())
            })
        })
        .unwrap_or_else(|| "default".into())
}

impl CodexProcess {
    pub fn new(cwd: &Path, session_id: Option<String>, tx: Sender<AgentMsg>, thread_idx: usize) -> Self {
        CodexProcess { cwd: cwd.to_path_buf(), tx, thread_idx, session_id, child: None }
    }

    pub fn send_user(&mut self, text: &str) -> std::io::Result<()> {
        if let Some(c) = &self.child
            && c.lock().map(|mut c| c.try_wait().ok().flatten().is_none()).unwrap_or(false)
        {
            return Err(std::io::Error::other("codex is still running the previous turn"));
        }
        let mut cmd = Command::new("codex");
        // `-s` is an option of `exec`. After `resume` it is refused ("unexpected argument '-s'", exit 2),
        // so it goes first; verified on 0.165.0 that a resumed turn can then still write in the workspace.
        cmd.args(["exec", "-s", "workspace-write"]);
        if let Some(id) = &self.session_id {
            cmd.args(["resume", id]);
        }
        cmd.args(["--json", "--skip-git-repo-check", "-"]);
        cmd.current_dir(&self.cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn()?;
        let mut stdin = child.stdin.take().ok_or_else(|| std::io::Error::other("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| std::io::Error::other("no stdout"))?;
        let stderr = child.stderr.take().ok_or_else(|| std::io::Error::other("no stderr"))?;
        stdin.write_all(text.as_bytes())?;
        stdin.write_all(b"\n")?;
        drop(stdin);
        let child = Arc::new(Mutex::new(child));
        self.child = Some(child.clone());

        let stderr_tail: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        {
            let tail = stderr_tail.clone();
            std::thread::spawn(move || {
                let mut buf = String::new();
                let _ = BufReader::new(stderr).read_to_string(&mut buf);
                let keep = buf.len().saturating_sub(2000);
                if let Ok(mut t) = tail.lock() {
                    *t = buf[keep..].to_string();
                }
            });
        }
        let tx = self.tx.clone();
        let idx = self.thread_idx;
        let model = configured_model();
        // One process is one turn; its start time names the turn, also across host restarts.
        let turn = format!("{:x}", SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0));
        std::thread::spawn(move || {
            let mut before = Before::new();
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if line.trim().is_empty() {
                    continue;
                }
                let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
                for ev in parse(&value, &model, &turn, &mut before) {
                    if tx.send(AgentMsg { idx, agent: Agent::Codex, ev }).is_err() {
                        return;
                    }
                }
            }
            let code = child.lock().ok().and_then(|mut c| c.wait().ok()).and_then(|s| s.code());
            let tail = stderr_tail.lock().map(|t| t.clone()).unwrap_or_default();
            // "Reading additional input from stdin..." is Codex's normal note when the prompt is piped.
            let tail = tail.lines().filter(|l| !l.contains("Reading additional input")).collect::<Vec<_>>().join("\n");
            let _ = tx.send(AgentMsg { idx, agent: Agent::Codex, ev: AgentEvent::Exited { code, stderr_tail: tail } });
        });
        Ok(())
    }

    /// Exec mode has no interrupt message; stopping the process is the only way.
    pub fn interrupt(&mut self) {
        if let Some(c) = self.child.take()
            && let Ok(mut c) = c.lock()
        {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// `/usr/bin/bash -lc 'cat src/x.ts'` reads as `cat src/x.ts`.
fn unwrap_shell(command: &str) -> String {
    let c = command.trim();
    for prefix in ["/usr/bin/bash -lc ", "/bin/bash -lc ", "bash -lc ", "/bin/sh -lc ", "sh -lc ", "/usr/bin/bash -c ", "bash -c "] {
        if let Some(rest) = c.strip_prefix(prefix) {
            let rest = rest.trim();
            if (rest.starts_with('\'') && rest.ends_with('\'')) || (rest.starts_with('"') && rest.ends_with('"')) {
                return rest[1..rest.len() - 1].to_string();
            }
            return rest.to_string();
        }
    }
    c.to_string()
}

/// A file's text at one moment, for diffing a change Codex only names.
enum Snapshot {
    Missing,
    Text(String),
    /// Too large, unreadable or not text: no diff is shown.
    Opaque,
}

/// Files above this size are not diffed.
const MAX_SNAPSHOT_BYTES: u64 = 512 * 1024;

impl Snapshot {
    fn take(path: &str) -> Snapshot {
        match std::fs::metadata(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Snapshot::Missing,
            Ok(m) if m.is_file() && m.len() <= MAX_SNAPSHOT_BYTES => {
                std::fs::read_to_string(path).map(Snapshot::Text).unwrap_or(Snapshot::Opaque)
            }
            _ => Snapshot::Opaque,
        }
    }

    fn text(&self) -> Option<&str> {
        match self {
            Snapshot::Missing => Some(""),
            Snapshot::Text(s) => Some(s),
            Snapshot::Opaque => None,
        }
    }

    fn diff_to(&self, now: &Snapshot) -> Option<String> {
        let text = diff::unified(self.text()?, now.text()?, 3);
        (!text.is_empty()).then_some(text)
    }
}

/// File snapshots taken when a change started, by tool id.
type Before = HashMap<String, Snapshot>;

/// `turn` makes tool ids unique: Codex numbers its items from `item_0` again in every turn, so
/// without it a later turn's tool call would be taken for an earlier one and never shown.
fn item_events(item: &Value, completed: bool, turn: &str, before: &mut Before, out: &mut Vec<AgentEvent>) {
    let id = format!("{turn}:{}", str_of(item, "id"));
    match str_of(item, "type").as_str() {
        "agent_message" if completed => {
            out.push(AgentEvent::TextStart);
            out.push(AgentEvent::TextFinal(str_of(item, "text")));
        }
        "command_execution" => {
            let command = unwrap_shell(&str_of(item, "command"));
            out.push(AgentEvent::ToolUse { id: id.clone(), name: "Bash".into(), input: json!({ "command": command }) });
            if completed {
                let exit = item.get("exit_code").and_then(Value::as_i64);
                let output = str_of(item, "aggregated_output");
                let failed = exit.is_some_and(|c| c != 0) || str_of(item, "status") == "failed";
                // A failed command leads with its exit code: that line is what the transcript shows.
                let content = if failed {
                    let code = exit.map(|c| format!("exit {c}")).unwrap_or_else(|| "failed".into());
                    if output.trim().is_empty() { code } else { format!("{code}\n{output}") }
                } else {
                    output
                };
                out.push(AgentEvent::ToolResult { tool_use_id: id, is_error: failed, content, diff: None });
            }
        }
        "file_change" => {
            let changes = item.get("changes").and_then(Value::as_array).cloned().unwrap_or_default();
            for (n, ch) in changes.iter().enumerate() {
                let path = str_of(ch, "path");
                let cid = format!("{id}:{n}");
                out.push(AgentEvent::ToolUse { id: cid.clone(), name: "Edit".into(), input: json!({ "file_path": path }) });
                if completed {
                    let failed = str_of(item, "status") == "failed";
                    // Exec mode reports the path and the kind of a change, never its text. The file as it
                    // was when the change started, against the file now, gives the diff.
                    let diff = before.remove(&cid).filter(|_| !failed).and_then(|was| was.diff_to(&Snapshot::take(&path)));
                    out.push(AgentEvent::ToolResult { tool_use_id: cid, is_error: failed, content: str_of(ch, "kind"), diff });
                } else {
                    before.insert(cid, Snapshot::take(&path));
                }
            }
        }
        "mcp_tool_call" => {
            let name = format!("{}.{}", str_of(item, "server"), str_of(item, "tool"));
            out.push(AgentEvent::ToolUse { id: id.clone(), name, input: item.get("arguments").cloned().unwrap_or(Value::Null) });
            if completed {
                let failed = str_of(item, "status") == "failed";
                out.push(AgentEvent::ToolResult { tool_use_id: id, is_error: failed, content: item.get("result").map(|r| r.to_string()).unwrap_or_default(), diff: None });
            }
        }
        "web_search" => {
            out.push(AgentEvent::ToolUse { id: id.clone(), name: "WebSearch".into(), input: json!({ "query": str_of(item, "query") }) });
            if completed {
                out.push(AgentEvent::ToolResult { tool_use_id: id, is_error: false, content: String::new(), diff: None });
            }
        }
        "error" if completed => {
            out.push(AgentEvent::TextStart);
            out.push(AgentEvent::TextFinal(format!("✕ {}", str_of(item, "message"))));
        }
        // reasoning, todo_list and anything new are not shown.
        _ => {}
    }
}

fn parse(v: &Value, model: &str, turn: &str, before: &mut Before) -> Vec<AgentEvent> {
    let mut out = Vec::new();
    match str_of(v, "type").as_str() {
        "thread.started" => {
            out.push(AgentEvent::Started { session_id: str_of(v, "thread_id"), model: model.to_string() });
        }
        "item.started" => {
            if let Some(item) = v.get("item") {
                item_events(item, false, turn, before, &mut out);
            }
        }
        "item.updated" => {}
        "item.completed" => {
            if let Some(item) = v.get("item") {
                item_events(item, true, turn, before, &mut out);
            }
        }
        "turn.completed" => {
            out.push(AgentEvent::TurnDone { is_error: false, cost_usd: 0.0, duration_ms: 0, result: None, subtype: "success".into() });
        }
        "turn.failed" => {
            let msg = v.get("error").map(|e| str_of(e, "message")).filter(|m| !m.is_empty());
            out.push(AgentEvent::TurnDone { is_error: true, cost_usd: 0.0, duration_ms: 0, result: msg, subtype: "turn.failed".into() });
        }
        "error" => {
            out.push(AgentEvent::TurnDone { is_error: true, cost_usd: 0.0, duration_ms: 0, result: Some(str_of(v, "message")), subtype: "error".into() });
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool_ids(turn: &str) -> Vec<String> {
        let line = json!({ "type": "item.completed", "item": { "id": "item_1", "type": "command_execution", "command": "bash -lc 'true'", "aggregated_output": "", "exit_code": 0, "status": "completed" } });
        parse(&line, "m", turn, &mut Before::new())
            .into_iter()
            .filter_map(|e| match e {
                AgentEvent::ToolUse { id, .. } => Some(id),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_same_item_id_in_two_turns_is_two_tool_calls() {
        assert_ne!(tool_ids("a"), tool_ids("b"));
        assert_eq!(tool_ids("a"), vec!["a:item_1".to_string()]);
    }

    #[test]
    fn a_failed_command_leads_with_its_exit_code() {
        let line = json!({ "type": "item.completed", "item": { "id": "item_1", "type": "command_execution", "command": "bash -lc 'ls /nope'", "aggregated_output": "ls: cannot access\n", "exit_code": 2, "status": "failed" } });
        let result = parse(&line, "m", "t", &mut Before::new()).into_iter().find_map(|e| match e {
            AgentEvent::ToolResult { is_error, content, .. } => Some((is_error, content)),
            _ => None,
        });
        assert_eq!(result, Some((true, "exit 2\nls: cannot access\n".to_string())));
    }

    #[test]
    fn a_file_change_is_diffed_against_the_file_as_it_was_when_the_change_started() {
        let dir = std::env::temp_dir().join(format!("desl-codex-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.txt");
        std::fs::write(&path, "one\ntwo\n").unwrap();
        let item = |status: &str| json!({ "type": if status == "completed" { "item.completed" } else { "item.started" },
            "item": { "id": "item_3", "type": "file_change", "changes": [{ "path": path.to_string_lossy(), "kind": "update" }], "status": status } });
        let mut before = Before::new();
        parse(&item("in_progress"), "m", "t", &mut before);
        std::fs::write(&path, "one\n2\n").unwrap();
        let diff = parse(&item("completed"), "m", "t", &mut before).into_iter().find_map(|e| match e {
            AgentEvent::ToolResult { diff, .. } => diff,
            _ => None,
        });
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(diff.as_deref(), Some("@@ -1,2 +1,2 @@\n one\n-two\n+2\n"));
    }
}
