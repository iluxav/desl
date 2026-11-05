//! Drives `claude -p --output-format stream-json --input-format stream-json`.
//! Flags verified against Claude Code 2.1.312 on 2026-11-04; see docs/adapters.md.

use super::{AgentEvent, AgentMsg};
use crate::diff;
use crate::model::Agent;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

pub struct ClaudeProcess {
    child: Arc<Mutex<Child>>,
    stdin: ChildStdin,
    request_seq: u64,
}

impl ClaudeProcess {
    pub fn spawn(cwd: &Path, resume: Option<&str>, tx: Sender<AgentMsg>, thread_idx: usize) -> Result<Self> {
        let mut cmd = Command::new("claude");
        cmd.args([
            "-p",
            "--output-format",
            "stream-json",
            "--input-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-prompts",
            "host",
            "--permission-prompt-tool",
            "stdio",
            "--permission-mode",
            "acceptEdits",
        ]);
        if let Some(id) = resume {
            cmd.args(["--resume", id]);
        }
        // A nested Claude Code session must not inherit the parent's session plumbing.
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("CLAUDE") {
                cmd.env_remove(key);
            }
        }
        cmd.current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = cmd.spawn().context("could not start `claude`")?;
        let stdin = child.stdin.take().context("no stdin")?;
        let stdout = child.stdout.take().context("no stdout")?;
        let stderr = child.stderr.take().context("no stderr")?;
        let child = Arc::new(Mutex::new(child));

        let stderr_tail: Arc<Mutex<String>> = Arc::new(Mutex::new(String::new()));
        {
            let tail = stderr_tail.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stderr);
                let mut buf = String::new();
                let _ = reader.read_to_string(&mut buf);
                let keep = buf.len().saturating_sub(2000);
                if let Ok(mut t) = tail.lock() {
                    *t = buf[keep..].to_string();
                }
            });
        }
        {
            let child = child.clone();
            std::thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    let Ok(value) = serde_json::from_str::<Value>(&line) else { continue };
                    for ev in parse(&value) {
                        if tx.send(AgentMsg { idx: thread_idx, agent: Agent::Claude, ev }).is_err() {
                            return;
                        }
                    }
                }
                let code = child.lock().ok().and_then(|mut c| c.wait().ok()).and_then(|s| s.code());
                let tail = stderr_tail.lock().map(|t| t.clone()).unwrap_or_default();
                let _ = tx.send(AgentMsg { idx: thread_idx, agent: Agent::Claude, ev: AgentEvent::Exited { code, stderr_tail: tail } });
            });
        }
        Ok(ClaudeProcess { child, stdin, request_seq: 0 })
    }

    fn write(&mut self, value: Value) -> std::io::Result<()> {
        let mut line = serde_json::to_string(&value).unwrap_or_default();
        line.push('\n');
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.flush()
    }

    pub fn send_user(&mut self, text: &str) -> std::io::Result<()> {
        self.write(json!({
            "type": "user",
            "message": { "role": "user", "content": [{ "type": "text", "text": text }] }
        }))
    }

    pub fn interrupt(&mut self) -> std::io::Result<()> {
        self.request_seq += 1;
        self.write(json!({
            "type": "control_request",
            "request_id": format!("desl-{}", self.request_seq),
            "request": { "subtype": "interrupt" }
        }))
    }

    pub fn answer(&mut self, request_id: &str, allow: bool, input: &Value) -> std::io::Result<()> {
        let response = if allow {
            json!({ "behavior": "allow", "updatedInput": input })
        } else {
            json!({ "behavior": "deny", "message": "The user denied this in desl." })
        };
        self.write(json!({
            "type": "control_response",
            "response": { "subtype": "success", "request_id": request_id, "response": response }
        }))
    }

    pub fn kill(&mut self) {
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// One stdout line can carry several events (an assistant message with many blocks).
fn parse(v: &Value) -> Vec<AgentEvent> {
    let mut out = Vec::new();
    // Subagent traffic carries parent_tool_use_id; this prototype shows the main thread only.
    let is_subagent = v.get("parent_tool_use_id").is_some_and(|p| !p.is_null());
    match v.get("type").and_then(Value::as_str) {
        Some("system") if v.get("subtype").and_then(Value::as_str) == Some("init") => {
            out.push(AgentEvent::Started { session_id: str_of(v, "session_id"), model: str_of(v, "model") });
        }
        Some("stream_event") if !is_subagent => {
            let e = &v["event"];
            match e.get("type").and_then(Value::as_str) {
                Some("content_block_start") if e["content_block"]["type"] == "text" => out.push(AgentEvent::TextStart),
                Some("content_block_delta") if e["delta"]["type"] == "text_delta" => {
                    out.push(AgentEvent::TextDelta(str_of(&e["delta"], "text")));
                }
                _ => {}
            }
        }
        Some("assistant") if !is_subagent => {
            if let Some(blocks) = v["message"]["content"].as_array() {
                for b in blocks {
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") => out.push(AgentEvent::TextFinal(str_of(b, "text"))),
                        Some("tool_use") => out.push(AgentEvent::ToolUse {
                            id: str_of(b, "id"),
                            name: str_of(b, "name"),
                            input: b.get("input").cloned().unwrap_or(Value::Null),
                        }),
                        _ => {}
                    }
                }
            }
        }
        Some("user") if !is_subagent => {
            // The structured twin of the tool result: a patch for an edit, the file for a read.
            let structured = &v["tool_use_result"];
            if let Some(blocks) = v["message"]["content"].as_array() {
                for b in blocks {
                    if b.get("type").and_then(Value::as_str) == Some("tool_result") {
                        let is_error = b.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                        let read = if is_error { None } else { result_file(structured) };
                        out.push(AgentEvent::ToolResult {
                            tool_use_id: str_of(b, "tool_use_id"),
                            is_error,
                            content: read.unwrap_or_else(|| text_of(&b["content"])),
                            diff: if is_error { None } else { result_diff(structured) },
                        });
                    }
                }
            }
        }
        Some("result") => {
            let subtype = str_of(v, "subtype");
            out.push(AgentEvent::TurnDone {
                is_error: v.get("is_error").and_then(Value::as_bool).unwrap_or(subtype != "success"),
                cost_usd: v.get("total_cost_usd").and_then(Value::as_f64).unwrap_or(0.0),
                duration_ms: v.get("duration_ms").and_then(Value::as_u64).unwrap_or(0),
                result: v.get("result").and_then(Value::as_str).map(str::to_string),
                subtype,
            });
        }
        Some("control_request") => {
            let req = &v["request"];
            if req.get("subtype").and_then(Value::as_str) == Some("can_use_tool") {
                out.push(AgentEvent::ApprovalRequest {
                    request_id: str_of(v, "request_id"),
                    tool_name: str_of(req, "tool_name"),
                    input: req.get("input").cloned().unwrap_or(Value::Null),
                    tool_use_id: req.get("tool_use_id").and_then(Value::as_str).map(str::to_string),
                });
            }
        }
        _ => {}
    }
    out
}

/// The change an edit or a write made, as a unified diff. Claude Code attaches it to the result as
/// `structuredPatch` (hunks with line numbers); a new file has no patch, only its `content`.
fn result_diff(r: &Value) -> Option<String> {
    let hunks = r.get("structuredPatch")?.as_array()?;
    if hunks.is_empty() {
        let created = str_of(r, "type") == "create" || r.get("originalFile").is_some_and(Value::is_null);
        let text = diff::unified("", r.get("content").and_then(Value::as_str).filter(|_| created)?, 3);
        return (!text.is_empty()).then_some(text);
    }
    let mut out = String::new();
    for h in hunks {
        let n = |key: &str| h.get(key).and_then(Value::as_u64).unwrap_or(0);
        out.push_str(&format!("@@ -{},{} +{},{} @@\n", n("oldStart"), n("oldLines"), n("newStart"), n("newLines")));
        for l in h.get("lines").and_then(Value::as_array).into_iter().flatten() {
            out.push_str(l.as_str().unwrap_or(""));
            out.push('\n');
        }
    }
    Some(out)
}

/// A read comes back as `cat -n` text. The structured result has the file's own text and its
/// first line number, which gives clean numbered lines.
fn result_file(r: &Value) -> Option<String> {
    let file = r.get("file")?;
    let content = file.get("content")?.as_str()?;
    let start = file.get("startLine").and_then(Value::as_u64).unwrap_or(1) as usize;
    let lines: Vec<&str> = content.lines().collect();
    let w = (start + lines.len().saturating_sub(1)).to_string().len();
    Some(lines.iter().enumerate().map(|(i, l)| format!("{:>w$}  {l}", start + i)).collect::<Vec<_>>().join("\n"))
}

/// Collapse a tool call to a verb and a target, as in the spec's `read src/auth.ts`.
pub fn describe_tool(name: &str, input: &Value, cwd: &Path) -> (String, String) {
    let rel = |key: &str| -> String {
        let p = str_of(input, key);
        let cwd = cwd.to_string_lossy();
        p.strip_prefix(&*cwd).map(|r| r.trim_start_matches('/').to_string()).unwrap_or(p)
    };
    let first_line = |s: String| s.lines().next().unwrap_or("").trim().to_string();
    match name {
        "Read" | "NotebookRead" => ("read".into(), rel("file_path")),
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => ("edit".into(), rel("file_path")),
        "Bash" | "PowerShell" => ("run".into(), first_line(str_of(input, "command"))),
        "Grep" | "Glob" => ("search".into(), str_of(input, "pattern")),
        "WebSearch" => ("search".into(), str_of(input, "query")),
        "WebFetch" => ("fetch".into(), str_of(input, "url")),
        "Task" | "Agent" => ("agent".into(), str_of(input, "description")),
        "TodoWrite" => ("todo".into(), String::new()),
        other => {
            let target = input
                .as_object()
                .and_then(|o| o.values().find_map(|v| v.as_str().map(str::to_string)))
                .unwrap_or_default();
            (other.to_lowercase(), first_line(target))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result_of(v: Value) -> (String, Option<String>) {
        match parse(&v).into_iter().next() {
            Some(AgentEvent::ToolResult { content, diff, .. }) => (content, diff),
            other => panic!("expected a tool result, got {other:?}"),
        }
    }

    fn user(structured: Value) -> Value {
        json!({
            "type": "user",
            "message": { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": "raw text" }] },
            "tool_use_result": structured,
        })
    }

    #[test]
    fn an_edit_result_carries_its_patch_as_a_unified_diff() {
        let (_, diff) = result_of(user(json!({
            "filePath": "/p/a.ts",
            "originalFile": "x",
            "structuredPatch": [
                { "oldStart": 2, "oldLines": 2, "newStart": 2, "newLines": 3, "lines": [" a", "+b", " c"] },
                { "oldStart": 9, "oldLines": 1, "newStart": 10, "newLines": 1, "lines": ["-d", "+e"] }
            ]
        })));
        assert_eq!(diff.as_deref(), Some("@@ -2,2 +2,3 @@\n a\n+b\n c\n@@ -9,1 +10,1 @@\n-d\n+e\n"));
    }

    #[test]
    fn a_new_file_is_shown_as_added_lines() {
        let (_, diff) = result_of(user(json!({ "type": "create", "filePath": "/p/n.txt", "content": "one\ntwo\n", "structuredPatch": [], "originalFile": null })));
        assert_eq!(diff.as_deref(), Some("@@ -0,0 +1,2 @@\n+one\n+two\n"));
    }

    #[test]
    fn a_read_result_is_numbered_from_its_first_line() {
        let (content, diff) = result_of(user(json!({ "type": "text", "file": { "filePath": "/p/a.ts", "content": "x\ny\nz", "startLine": 9, "numLines": 3, "totalLines": 40 } })));
        assert_eq!(content, " 9  x\n10  y\n11  z");
        assert_eq!(diff, None);
    }

    #[test]
    fn a_plain_result_keeps_its_text() {
        let (content, diff) = result_of(user(json!({ "stdout": "hi", "stderr": "" })));
        assert_eq!(content, "raw text");
        assert_eq!(diff, None);
    }
}
