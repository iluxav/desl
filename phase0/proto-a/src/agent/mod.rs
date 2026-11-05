//! The only code that knows the agents' native formats (AGENTS.md rule 4).
//! Prototype-local: this is not the phase 1 adapter contract.

pub mod claude;
pub mod codex;

use crate::model::Agent;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a process reports back, tagged with the thread and the agent that produced it.
pub struct AgentMsg {
    pub idx: usize,
    pub agent: Agent,
    pub ev: AgentEvent,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum AgentEvent {
    Started { session_id: String, model: String },
    /// A new text block begins: open a new assistant item.
    TextStart,
    TextDelta(String),
    /// The text block is complete, with its final text.
    TextFinal(String),
    ToolUse { id: String, name: String, input: Value },
    /// `diff` is the change an edit made, as a unified diff, when the adapter can tell.
    ToolResult {
        tool_use_id: String,
        is_error: bool,
        content: String,
        #[serde(default)]
        diff: Option<String>,
    },
    ApprovalRequest { request_id: String, tool_name: String, input: Value, tool_use_id: Option<String> },
    TurnDone { is_error: bool, cost_usd: f64, duration_ms: u64, result: Option<String>, subtype: String },
    Exited { code: Option<i32>, stderr_tail: String },
}

/// One live agent behind a thread.
pub enum Process {
    Claude(claude::ClaudeProcess),
    Codex(codex::CodexProcess),
}

impl Process {
    pub fn send_user(&mut self, text: &str) -> std::io::Result<()> {
        match self {
            Process::Claude(p) => p.send_user(text),
            Process::Codex(p) => p.send_user(text),
        }
    }
    pub fn interrupt(&mut self) -> std::io::Result<()> {
        match self {
            Process::Claude(p) => p.interrupt(),
            Process::Codex(p) => {
                p.interrupt();
                Ok(())
            }
        }
    }
    pub fn answer(&mut self, request_id: &str, allow: bool, input: &Value) -> std::io::Result<()> {
        match self {
            Process::Claude(p) => p.answer(request_id, allow, input),
            Process::Codex(_) => Ok(()),
        }
    }
    pub fn kill(&mut self) {
        match self {
            Process::Claude(p) => p.kill(),
            Process::Codex(p) => p.interrupt(),
        }
    }
    /// True when the process ending after a turn is normal, not a failure.
    pub fn exits_per_turn(&self) -> bool {
        matches!(self, Process::Codex(_))
    }
}
