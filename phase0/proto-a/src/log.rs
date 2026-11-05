//! The append-only log every thread is made of, and the client/host protocol around it.
//! Nothing is ever lost: the host appends, the clients replay.

use crate::agent::AgentEvent;
use crate::model::Agent;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BriefRecord {
    pub label: String,
    pub text: String,
    pub tokens: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Entry {
    ThreadCreated { idx: usize, id: String, agent: Agent },
    Renamed { idx: usize, name: String },
    /// The user sent a prompt; queued when the agent was busy.
    Prompt { idx: usize, text: String, queued: bool },
    /// The first queued prompt is about to go out.
    Dequeued { idx: usize },
    /// A prompt (with a brief, after a switch) went to the agent.
    Dispatched { idx: usize, agent: Agent, brief: Option<BriefRecord> },
    Agent { idx: usize, agent: Agent, ev: AgentEvent },
    Decision { idx: usize, request_id: String, allow: bool, always: bool },
    Interrupted { idx: usize, ended: bool },
    Switched { idx: usize, to: Agent, brief_from: Option<usize>, delta: bool },
    /// Something desl itself could not do; `message` is a complete sentence.
    Failed { idx: usize, message: String },
    /// A note from desl, or the brief it would send right now.
    Note { idx: usize, text: String, brief: Option<BriefRecord> },
    HostRestarted { idx: usize },
}

impl Entry {
    pub fn idx(&self) -> usize {
        match self {
            Entry::ThreadCreated { idx, .. }
            | Entry::Renamed { idx, .. }
            | Entry::Prompt { idx, .. }
            | Entry::Dequeued { idx }
            | Entry::Dispatched { idx, .. }
            | Entry::Agent { idx, .. }
            | Entry::Decision { idx, .. }
            | Entry::Interrupted { idx, .. }
            | Entry::Switched { idx, .. }
            | Entry::Failed { idx, .. }
            | Entry::Note { idx, .. }
            | Entry::HostRestarted { idx } => *idx,
        }
    }
}

/// Client → host.
#[derive(Serialize, Deserialize, Debug)]
pub enum Command {
    Attach,
    Prompt { idx: usize, text: String },
    Interrupt { idx: usize },
    Answer { idx: usize, request_id: String, allow: bool, always: bool },
    Switch { idx: usize, to: Agent },
    NewThread,
    Rename { idx: usize, name: String },
    ShowBrief { idx: usize },
    Note { idx: usize, text: String },
    SaveView { view: Value },
    Shutdown,
}

/// Host → client.
#[derive(Serialize, Deserialize, Debug)]
pub enum Frame {
    Snapshot { entries: Vec<Entry>, view: Option<Value> },
    Entry(Entry),
    Bye,
}
