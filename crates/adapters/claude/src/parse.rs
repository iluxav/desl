//! Claude Code's stream-json output, one JSON object per line, turned into adapter events.
//! Shapes verified against Claude Code 2.1.312; see docs/adapters.md.

use desl_ir::adapter::AdapterEvent;
use desl_ir::{ChangeKind, EventKind, Risk, diff, tool};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A permission request as Claude Code sent it, kept until the user answers.
#[derive(Clone, Debug)]
pub struct NativeRequest {
    pub request_id: String,
    pub input: Value,
}

pub struct Parser {
    cwd: PathBuf,
    /// Tool calls seen, by id: the neutral name and target path, for results that follow.
    calls: HashMap<String, (String, String)>,
    /// Permission requests waiting for an answer, by call id.
    pub requests: HashMap<String, NativeRequest>,
    /// `total_cost_usd` is a running total for the process; a turn's cost is the difference.
    cost_so_far: f64,
    model: Option<String>,
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

fn event(kind: EventKind) -> AdapterEvent {
    AdapterEvent::Event { event: kind }
}

/// The change an edit or a write made, as a unified diff. Claude Code attaches it to the result
/// as `structuredPatch` (hunks with line numbers); a new file has no patch, only its `content`.
fn result_diff(r: &Value) -> Option<String> {
    let hunks = r.get("structuredPatch")?.as_array()?;
    if hunks.is_empty() {
        let text = diff::unified("", r.get("content").and_then(Value::as_str).filter(|_| created(r))?, 3);
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

fn created(r: &Value) -> bool {
    str_of(r, "type") == "create" || r.get("originalFile").is_some_and(Value::is_null)
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

impl Parser {
    pub fn new(cwd: &Path) -> Parser {
        Parser { cwd: cwd.to_path_buf(), calls: HashMap::new(), requests: HashMap::new(), cost_so_far: 0.0, model: None }
    }

    /// A path as the user would type it: relative to the thread's directory when inside it.
    fn rel(&self, path: &str) -> String {
        let cwd = self.cwd.to_string_lossy();
        match path.strip_prefix(&*cwd) {
            Some(r) if r.starts_with('/') => r.trim_start_matches('/').to_string(),
            _ => path.to_string(),
        }
    }

    /// Claude Code's tool name and input as the neutral name and arguments of the IR.
    fn neutral(&self, name: &str, input: &Value) -> (String, Value) {
        match name {
            "Read" | "NotebookRead" => (tool::READ.into(), json!({ "path": self.rel(&str_of(input, "file_path")) })),
            "Edit" | "MultiEdit" | "Write" => (tool::EDIT.into(), json!({ "path": self.rel(&str_of(input, "file_path")) })),
            "NotebookEdit" => {
                let path = input.get("notebook_path").or_else(|| input.get("file_path")).and_then(Value::as_str).unwrap_or("");
                (tool::EDIT.into(), json!({ "path": self.rel(path) }))
            }
            "Bash" | "PowerShell" => (tool::BASH.into(), json!({ "command": str_of(input, "command") })),
            "Grep" | "Glob" => (tool::SEARCH.into(), json!({ "pattern": str_of(input, "pattern") })),
            "WebSearch" => (tool::SEARCH.into(), json!({ "query": str_of(input, "query") })),
            "WebFetch" => (tool::FETCH.into(), json!({ "url": str_of(input, "url") })),
            "Task" | "Agent" => (tool::AGENT.into(), json!({ "description": str_of(input, "description") })),
            "TodoWrite" => (tool::TODO.into(), json!({})),
            other => (other.to_string(), input.clone()),
        }
    }

    fn risk(name: &str, native: &str) -> Risk {
        match name {
            tool::READ => Risk::Read,
            tool::EDIT => Risk::Edit,
            tool::FETCH => Risk::Network,
            tool::SEARCH if native == "WebSearch" => Risk::Network,
            tool::SEARCH => Risk::Read,
            _ => Risk::Command,
        }
    }

    /// One stdout line can carry several events (an assistant message with many blocks).
    pub fn line(&mut self, v: &Value) -> Vec<AdapterEvent> {
        let mut out = Vec::new();
        // Subagent traffic carries parent_tool_use_id; a thread shows its main conversation only.
        let is_subagent = v.get("parent_tool_use_id").is_some_and(|p| !p.is_null());
        match v.get("type").and_then(Value::as_str) {
            Some("system") if v.get("subtype").and_then(Value::as_str) == Some("init") => {
                let model = Some(str_of(v, "model")).filter(|m| !m.is_empty());
                self.model = model.clone();
                out.push(AdapterEvent::Session { session_id: str_of(v, "session_id"), model });
            }
            Some("stream_event") if !is_subagent => {
                let e = &v["event"];
                match e.get("type").and_then(Value::as_str) {
                    Some("content_block_start") if e["content_block"]["type"] == "text" => out.push(AdapterEvent::TextStart),
                    Some("content_block_delta") if e["delta"]["type"] == "text_delta" => {
                        out.push(AdapterEvent::TextDelta { text: str_of(&e["delta"], "text") });
                    }
                    _ => {}
                }
            }
            Some("assistant") if !is_subagent => {
                for b in v["message"]["content"].as_array().into_iter().flatten() {
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            let text = str_of(b, "text");
                            if !text.is_empty() {
                                out.push(event(EventKind::AssistantMessage { text, model: self.model.clone() }));
                            }
                        }
                        Some("tool_use") => {
                            let id = str_of(b, "id");
                            let (name, args) = self.neutral(&str_of(b, "name"), b.get("input").unwrap_or(&Value::Null));
                            self.calls.insert(id.clone(), (name.clone(), str_of(&args, "path")));
                            out.push(event(EventKind::ToolCall { id, name, args }));
                        }
                        _ => {}
                    }
                }
            }
            Some("user") if !is_subagent => {
                // The structured twin of the tool result: a patch for an edit, the file for a read.
                let structured = &v["tool_use_result"];
                for b in v["message"]["content"].as_array().into_iter().flatten() {
                    if b.get("type").and_then(Value::as_str) != Some("tool_result") {
                        continue;
                    }
                    let call_id = str_of(b, "tool_use_id");
                    let is_error = b.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                    let read = if is_error { None } else { result_file(structured) };
                    out.push(event(EventKind::ToolResult {
                        call_id: call_id.clone(),
                        output: read.unwrap_or_else(|| text_of(&b["content"])),
                        exit_code: None,
                        is_error,
                        truncated: false,
                        blob: None,
                    }));
                    self.requests.remove(&call_id);
                    if let Some((name, path)) = self.calls.get(&call_id)
                        && name == tool::EDIT
                        && !is_error
                    {
                        out.push(event(EventKind::FileChange {
                            path: path.clone(),
                            diff: result_diff(structured),
                            change: if created(structured) { ChangeKind::Create } else { ChangeKind::Modify },
                            call_id: Some(call_id),
                        }));
                    }
                }
            }
            Some("result") => {
                let subtype = str_of(v, "subtype");
                let is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(subtype != "success");
                let u = &v["usage"];
                let n = |key: &str| u.get(key).and_then(Value::as_u64).unwrap_or(0);
                let input_tokens = n("input_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens");
                let total = v.get("total_cost_usd").and_then(Value::as_f64).unwrap_or(0.0);
                let cost = (total - self.cost_so_far).max(0.0);
                self.cost_so_far = self.cost_so_far.max(total);
                if input_tokens + n("output_tokens") > 0 || cost > 0.0 {
                    out.push(event(EventKind::Usage {
                        input_tokens,
                        output_tokens: n("output_tokens"),
                        cost_estimate: (cost > 0.0).then_some(cost),
                    }));
                }
                self.requests.clear();
                // `error_during_execution` is how an interrupted turn ends; the user did that.
                let error = (is_error && subtype != "error_during_execution").then(|| {
                    let said = v.get("result").and_then(Value::as_str).filter(|s| !s.trim().is_empty()).unwrap_or(&subtype);
                    format!("claude stopped: {}. Send your prompt again to retry.", said.trim().trim_end_matches('.'))
                });
                out.push(AdapterEvent::TurnEnd { error });
            }
            Some("control_request") => {
                let req = &v["request"];
                if req.get("subtype").and_then(Value::as_str) == Some("can_use_tool") {
                    let request_id = str_of(v, "request_id");
                    let native = str_of(req, "tool_name");
                    let input = req.get("input").cloned().unwrap_or(Value::Null);
                    let (name, args) = self.neutral(&native, &input);
                    let call_id = req.get("tool_use_id").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| request_id.clone());
                    let target = desl_ir::tool_target(&name, &args);
                    // A shell command reads as itself; other tools as their name and target.
                    let summary = if name == tool::BASH { target } else { format!("{name} {target}").trim().to_string() };
                    let risk = Self::risk(&name, &native);
                    self.requests.insert(call_id.clone(), NativeRequest { request_id, input });
                    out.push(event(EventKind::ApprovalRequest { call_id, tool: name, summary, risk }));
                }
            }
            _ => {}
        }
        out
    }
}
