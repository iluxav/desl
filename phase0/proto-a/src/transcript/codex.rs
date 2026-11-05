//! Codex rollout files: `~/.codex/sessions/YYYY/MM/DD/rollout-<timestamp>-<thread-id>.jsonl`.
//! Verified against Codex CLI 0.165.0; fixture `fixtures/transcripts/codex-0.165.0-*.jsonl`.

use super::{TranscriptEvent, home, newest_since};
use crate::agent::AgentEvent;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub fn sessions_dir() -> PathBuf {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".codex")).join("sessions")
}

/// Today's (and yesterday's, around midnight) rollout directories.
fn day_dirs() -> Vec<PathBuf> {
    let now = chrono::Local::now();
    let mut dirs = Vec::new();
    for d in [now, now - chrono::Duration::days(1)] {
        dirs.push(sessions_dir().join(d.format("%Y").to_string()).join(d.format("%m").to_string()).join(d.format("%d").to_string()));
    }
    dirs
}

/// The rollout a Codex started after `since` in `cwd` is writing, if any. Checks the file's own cwd.
pub fn find_session(cwd: &Path, since: SystemTime, taken: &[PathBuf]) -> Option<PathBuf> {
    let want = cwd.to_string_lossy().to_string();
    for dir in day_dirs() {
        let found = newest_since(&dir, since, taken, |p| {
            p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("rollout-"))
                && std::fs::read_to_string(p).ok().is_some_and(|s| s.lines().next().is_some_and(|l| l.contains(&want) && began_after(l, since)))
        });
        if found.is_some() {
            return found;
        }
    }
    None
}

/// True when the rollout's first line is stamped at or after `since`. An older Codex still working in the
/// same directory keeps touching its own rollout; it must not be taken for the one this pane started.
fn began_after(first_line: &str, since: SystemTime) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(first_line) else { return false };
    let Some(ts) = v.get("timestamp").and_then(Value::as_str) else { return true };
    match chrono::DateTime::parse_from_rfc3339(ts) {
        Ok(t) => SystemTime::from(t) >= since,
        Err(_) => true,
    }
}

#[derive(Default)]
pub struct State {
    started: bool,
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// `text(await tools.exec_command({cmd:"pwd; rg --files", ...}))` reads as `pwd; rg --files`.
fn extract_cmd(input: &str) -> Option<String> {
    let start = input.find("cmd:\"")? + 5;
    let rest = &input[start..];
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(n) = chars.next() {
                    out.push(match n {
                        'n' => '\n',
                        't' => '\t',
                        other => other,
                    });
                }
            }
            '"' => return Some(out),
            c => out.push(c),
        }
    }
    Some(out)
}

/// `*** Update File: path` lines inside an apply_patch call.
fn patch_files(input: &str) -> Vec<String> {
    let mut files = Vec::new();
    for marker in ["*** Update File: ", "*** Add File: ", "*** Delete File: "] {
        let mut rest = input;
        while let Some(pos) = rest.find(marker) {
            let after = &rest[pos + marker.len()..];
            let end = after.find("\\n").or_else(|| after.find('\n')).unwrap_or(after.len());
            let f = after[..end].trim().trim_end_matches('"').to_string();
            if !f.is_empty() && !files.contains(&f) {
                files.push(f);
            }
            rest = &after[end..];
        }
    }
    files
}

fn tool_use_for(call_id: &str, name: &str, input: &str) -> Vec<AgentEvent> {
    if input.contains("apply_patch(") {
        let files = patch_files(input);
        if !files.is_empty() {
            return files
                .iter()
                .enumerate()
                .map(|(n, f)| AgentEvent::ToolUse {
                    id: if n == 0 { call_id.to_string() } else { format!("{call_id}:{n}") },
                    name: "Edit".into(),
                    input: json!({ "file_path": f }),
                })
                .collect();
        }
    }
    if let Some(cmd) = extract_cmd(input) {
        return vec![AgentEvent::ToolUse { id: call_id.to_string(), name: "Bash".into(), input: json!({ "command": cmd }) }];
    }
    vec![AgentEvent::ToolUse { id: call_id.to_string(), name: name.to_string(), input: json!({ "input": input.chars().take(120).collect::<String>() }) }]
}

pub fn parse_line(state: &mut State, line: &str) -> Vec<TranscriptEvent> {
    let mut out = Vec::new();
    let Ok(v) = serde_json::from_str::<Value>(line) else { return out };
    let payload = v.get("payload").cloned().unwrap_or(Value::Null);
    let ptype = str_of(&payload, "type");
    match str_of(&v, "type").as_str() {
        "session_meta" => {
            if !state.started {
                state.started = true;
                let id = str_of(&payload, "id");
                out.push(TranscriptEvent::Agent(AgentEvent::Started { session_id: id, model: String::new() }));
            }
        }
        "event_msg" => match ptype.as_str() {
            "task_started" => out.push(TranscriptEvent::TurnStarted),
            "task_complete" => out.push(TranscriptEvent::Agent(AgentEvent::TurnDone {
                is_error: false,
                cost_usd: 0.0,
                duration_ms: 0,
                result: None,
                subtype: "success".into(),
            })),
            "turn_aborted" => out.push(TranscriptEvent::Agent(AgentEvent::TurnDone {
                is_error: true,
                cost_usd: 0.0,
                duration_ms: 0,
                result: None,
                subtype: "error_during_execution".into(),
            })),
            _ => {}
        },
        "response_item" => match ptype.as_str() {
            "message" => {
                let role = str_of(&payload, "role");
                let texts: Vec<String> = payload
                    .get("content")
                    .and_then(Value::as_array)
                    .map(|c| c.iter().filter_map(|b| b.get("text").and_then(Value::as_str).map(str::to_string)).collect())
                    .unwrap_or_default();
                match role.as_str() {
                    "user" => {
                        for t in texts {
                            // Environment context and other injected blocks start with a tag.
                            if !t.trim_start().starts_with('<') && !t.trim().is_empty() {
                                out.push(TranscriptEvent::User(t));
                            }
                        }
                    }
                    "assistant" => {
                        for t in texts {
                            if !t.trim().is_empty() {
                                out.push(TranscriptEvent::Agent(AgentEvent::TextStart));
                                out.push(TranscriptEvent::Agent(AgentEvent::TextFinal(t)));
                            }
                        }
                    }
                    _ => {}
                }
            }
            "custom_tool_call" => {
                let call_id = str_of(&payload, "call_id");
                let name = str_of(&payload, "name");
                let input = str_of(&payload, "input");
                out.extend(tool_use_for(&call_id, &name, &input).into_iter().map(TranscriptEvent::Agent));
            }
            "custom_tool_call_output" | "function_call_output" => {
                let output = match payload.get("output") {
                    Some(Value::String(s)) => s.clone(),
                    Some(other) => other.to_string(),
                    None => String::new(),
                };
                let is_error = output.contains("exit code: 1") || output.starts_with("Error");
                out.push(TranscriptEvent::Agent(AgentEvent::ToolResult { tool_use_id: str_of(&payload, "call_id"), is_error, content: output, diff: None }));
            }
            "function_call" => {
                let args: Value = serde_json::from_str(&str_of(&payload, "arguments")).unwrap_or(Value::Null);
                out.push(TranscriptEvent::Agent(AgentEvent::ToolUse { id: str_of(&payload, "call_id"), name: str_of(&payload, "name"), input: args }));
            }
            _ => {}
        },
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_input_yields_the_command() {
        let input = r#"text(await tools.exec_command({cmd:"pwd; rg --files -g '!node_modules'", workdir:"/x"}))"#;
        assert_eq!(extract_cmd(input).as_deref(), Some("pwd; rg --files -g '!node_modules'"));
    }

    #[test]
    fn apply_patch_input_yields_files() {
        let input = "text(await tools.apply_patch(\"*** Begin Patch\\n*** Update File: src/auth/tokens.ts\\n@@\\n*** End Patch\"))";
        assert_eq!(patch_files(input), vec!["src/auth/tokens.ts".to_string()]);
    }
}
