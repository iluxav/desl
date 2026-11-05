//! `codex exec --json` output, one JSON object per line, turned into adapter events.
//! Shapes verified against Codex CLI 0.165.0; see docs/adapters.md.
//!
//! What shapes this parser: agent messages arrive whole (no token deltas), a file change names
//! the file but never says what changed, and item ids start again at `item_0` in every turn.

use desl_ir::adapter::AdapterEvent;
use desl_ir::{ChangeKind, Effort, EventKind, ModelInfo, diff, tool};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

pub struct Parser {
    cwd: PathBuf,
    /// Names this turn; prefixed to item ids so the same `item_1` in two turns is two tool calls.
    turn: String,
    model: Option<String>,
    /// File snapshots taken when a change started, by tool id.
    before: HashMap<String, Snapshot>,
    /// Tool calls already announced, so `item.started` and `item.completed` announce one call.
    announced: std::collections::HashSet<String>,
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn event(kind: EventKind) -> AdapterEvent {
    AdapterEvent::Event { event: kind }
}

/// `/usr/bin/bash -lc 'cat src/x.ts'` reads as `cat src/x.ts`.
fn unwrap_shell(command: &str) -> String {
    let c = command.trim();
    for prefix in [
        "/usr/bin/bash -lc ",
        "/bin/bash -lc ",
        "bash -lc ",
        "/bin/sh -lc ",
        "sh -lc ",
        "/usr/bin/bash -c ",
        "bash -c ",
        "/bin/zsh -lc ",
        "zsh -lc ",
    ] {
        if let Some(rest) = c.strip_prefix(prefix) {
            let rest = rest.trim();
            if rest.len() >= 2 && ((rest.starts_with('\'') && rest.ends_with('\'')) || (rest.starts_with('"') && rest.ends_with('"'))) {
                return rest[1..rest.len() - 1].to_string();
            }
            return rest.to_string();
        }
    }
    c.to_string()
}

/// `codex debug models`: the catalog behind Codex's own `/model` picker. Entries Codex hides
/// are left out; the rest keep its order. `configured` is the model in the user's config, which
/// is the one a turn uses when nothing else is said; without it, Codex's first entry is.
pub fn models(v: &Value, configured: Option<&str>, configured_effort: Option<&str>) -> Vec<ModelInfo> {
    let mut listed: Vec<&Value> =
        v.get("models").and_then(Value::as_array).into_iter().flatten().filter(|m| str_of(m, "visibility") != "hide").collect();
    listed.sort_by_key(|m| m.get("priority").and_then(Value::as_i64).unwrap_or(i64::MAX));
    let default = configured
        .filter(|c| listed.iter().any(|m| str_of(m, "slug") == *c))
        .map(str::to_string)
        .or_else(|| listed.first().map(|m| str_of(m, "slug")));
    listed
        .into_iter()
        .filter_map(|m| {
            let id = str_of(m, "slug");
            let efforts: Vec<Effort> = m
                .get("supported_reasoning_levels")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|l| Effort { id: str_of(l, "effort"), description: str_of(l, "description") })
                .filter(|e| !e.id.is_empty())
                .collect();
            // The user's own config wins over the model's default, as it does in Codex itself.
            let default_effort = configured_effort
                .filter(|c| efforts.iter().any(|e| e.id == *c))
                .map(str::to_string)
                .or_else(|| Some(str_of(m, "default_reasoning_level")).filter(|l| !l.is_empty()));
            (!id.is_empty()).then(|| ModelInfo {
                name: Some(str_of(m, "display_name")).filter(|n| !n.is_empty()).unwrap_or_else(|| id.clone()),
                description: str_of(m, "description"),
                default: Some(&id) == default.as_ref(),
                efforts,
                default_effort,
                id,
            })
        })
        .collect()
}

impl Parser {
    pub fn new(cwd: &Path, turn: &str, model: Option<String>) -> Parser {
        Parser { cwd: cwd.to_path_buf(), turn: turn.to_string(), model, before: HashMap::new(), announced: Default::default() }
    }

    fn rel(&self, path: &str) -> String {
        let cwd = self.cwd.to_string_lossy();
        match path.strip_prefix(&*cwd) {
            Some(r) if r.starts_with('/') => r.trim_start_matches('/').to_string(),
            _ => path.to_string(),
        }
    }

    fn call(&mut self, id: &str, name: &str, args: Value, out: &mut Vec<AdapterEvent>) {
        if self.announced.insert(id.to_string()) {
            out.push(event(EventKind::ToolCall { id: id.to_string(), name: name.to_string(), args }));
        }
    }

    fn item(&mut self, item: &Value, completed: bool, out: &mut Vec<AdapterEvent>) {
        let id = format!("{}:{}", self.turn, str_of(item, "id"));
        let failed = str_of(item, "status") == "failed";
        let result = |call_id: String, output: String, exit_code: Option<i32>, is_error: bool| {
            event(EventKind::ToolResult { call_id, output, exit_code, is_error, truncated: false, blob: None })
        };
        match str_of(item, "type").as_str() {
            "agent_message" if completed => {
                let text = str_of(item, "text");
                if !text.trim().is_empty() {
                    out.push(AdapterEvent::TextStart);
                    out.push(event(EventKind::AssistantMessage { text, model: self.model.clone() }));
                }
            }
            "command_execution" => {
                let command = unwrap_shell(&str_of(item, "command"));
                self.call(&id, tool::BASH, json!({ "command": command }), out);
                if completed {
                    let exit = item.get("exit_code").and_then(Value::as_i64).map(|c| c as i32);
                    let output = str_of(item, "aggregated_output");
                    let failed = failed || exit.is_some_and(|c| c != 0);
                    // A failed command leads with its exit code: that line is what the transcript shows.
                    let output = if failed {
                        let code = exit.map(|c| format!("exit {c}")).unwrap_or_else(|| "failed".into());
                        if output.trim().is_empty() { code } else { format!("{code}\n{output}") }
                    } else {
                        output
                    };
                    out.push(result(id, output, exit, failed));
                }
            }
            "file_change" => {
                for (n, ch) in item.get("changes").and_then(Value::as_array).into_iter().flatten().enumerate() {
                    let path = str_of(ch, "path");
                    let cid = format!("{id}:{n}");
                    self.call(&cid, tool::EDIT, json!({ "path": self.rel(&path) }), out);
                    if !completed {
                        self.before.insert(cid, Snapshot::take(&path));
                        continue;
                    }
                    let kind = str_of(ch, "kind");
                    out.push(result(cid.clone(), kind.clone(), None, failed));
                    if !failed {
                        // Exec mode reports the path and the kind of a change, never its text. The file as it
                        // was when the change started, against the file now, gives the diff.
                        let diff = self.before.remove(&cid).and_then(|was| was.diff_to(&Snapshot::take(&path)));
                        let change = match kind.as_str() {
                            "add" => ChangeKind::Create,
                            "delete" => ChangeKind::Delete,
                            _ => ChangeKind::Modify,
                        };
                        out.push(event(EventKind::FileChange { path: self.rel(&path), diff, change, call_id: Some(cid) }));
                    }
                }
            }
            "mcp_tool_call" => {
                let name = format!("{}.{}", str_of(item, "server"), str_of(item, "tool"));
                self.call(&id, &name, item.get("arguments").cloned().unwrap_or(Value::Null), out);
                if completed {
                    out.push(result(id, item.get("result").map(Value::to_string).unwrap_or_default(), None, failed));
                }
            }
            "web_search" => {
                self.call(&id, tool::SEARCH, json!({ "query": str_of(item, "query") }), out);
                if completed {
                    out.push(result(id, String::new(), None, false));
                }
            }
            "error" if completed => {
                let said = str_of(item, "message");
                // Codex notes a model change on resume as an error item. The user picked the model
                // with /model; the turn runs, and nothing went wrong.
                if said.contains("is resuming with") {
                    return;
                }
                out.push(event(EventKind::Error {
                    message: format!("codex reported: {}.", said.trim().trim_end_matches('.')),
                    recoverable: true,
                }));
            }
            // reasoning, todo_list and anything new are not shown.
            _ => {}
        }
    }

    pub fn line(&mut self, v: &Value) -> Vec<AdapterEvent> {
        let mut out = Vec::new();
        let failed = |said: Option<String>| {
            let said = said.filter(|m| !m.trim().is_empty()).unwrap_or_else(|| "the turn failed".into());
            AdapterEvent::TurnEnd {
                error: Some(format!("codex stopped: {}. Send your prompt again to retry.", said.trim().trim_end_matches('.'))),
            }
        };
        match str_of(v, "type").as_str() {
            "thread.started" => out.push(AdapterEvent::Session { session_id: str_of(v, "thread_id"), model: self.model.clone() }),
            "item.started" => {
                if let Some(item) = v.get("item") {
                    self.item(item, false, &mut out);
                }
            }
            "item.completed" => {
                if let Some(item) = v.get("item") {
                    self.item(item, true, &mut out);
                }
            }
            "turn.completed" => {
                let u = &v["usage"];
                let n = |key: &str| u.get(key).and_then(Value::as_u64).unwrap_or(0);
                if n("input_tokens") + n("output_tokens") > 0 {
                    out.push(event(EventKind::Usage {
                        input_tokens: n("input_tokens"),
                        output_tokens: n("output_tokens"),
                        cost_estimate: None,
                    }));
                }
                out.push(AdapterEvent::TurnEnd { error: None });
            }
            "turn.failed" => out.push(failed(v.get("error").map(|e| str_of(e, "message")))),
            "error" => out.push(failed(Some(str_of(v, "message")))),
            _ => {}
        }
        out
    }
}
