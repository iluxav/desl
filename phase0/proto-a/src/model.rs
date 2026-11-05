//! Thread, items and status words. Vocabulary is fixed by SPEC.md "Naming and vocabulary".

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum Agent {
    Claude,
    Codex,
}

impl Agent {
    pub fn name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }
    pub fn parse(s: &str) -> Option<Agent> {
        match s.trim().to_lowercase().as_str() {
            "claude" | "claude-code" => Some(Agent::Claude),
            "codex" => Some(Agent::Codex),
            _ => None,
        }
    }
    pub fn other(self) -> Agent {
        match self {
            Agent::Claude => Agent::Codex,
            Agent::Codex => Agent::Claude,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Status {
    Working,
    Waiting,
    Idle,
    Failed,
    Detached,
}

impl Status {
    pub fn glyph(self) -> &'static str {
        match self {
            Status::Working => "●",
            Status::Waiting => "◐",
            Status::Idle => "○",
            Status::Failed => "✕",
            Status::Detached => "⊝",
        }
    }
    pub fn word(self) -> &'static str {
        match self {
            Status::Working => "working",
            Status::Waiting => "waiting",
            Status::Idle => "idle",
            Status::Failed => "failed",
            Status::Detached => "detached",
        }
    }
}

pub struct Outcome {
    pub ok: bool,
    pub summary: String,
}

/// How much of a tool call is on screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Open {
    /// The one line.
    Closed,
    /// An edit as it lands: the first `PREVIEW_LINES` rows of its diff.
    Preview,
    Full,
}

/// Rows of a diff shown under an edit before the user asks for all of it.
pub const PREVIEW_LINES: usize = 12;

pub enum Item {
    User { text: String, queued: bool },
    Assistant { text: String, streaming: bool },
    Tool {
        id: String,
        verb: String,
        target: String,
        outcome: Option<Outcome>,
        /// "allowed", "denied" or "always": the user's approval decision, shown on the line.
        decision: Option<&'static str>,
        detail: String,
        /// The change an edit made, as a unified diff. Drawn in place of `detail`.
        diff: Option<String>,
        open: Open,
    },
    /// A thin labelled line: `── switched to codex ──`.
    Divider(String),
    /// The handoff text that was (or would be) sent, collapsed to one line.
    Brief { label: String, text: String, tokens: usize, expanded: bool },
    Note(String),
    Error(String),
}

impl Item {
    pub fn expandable(&self) -> bool {
        matches!(self, Item::Tool { .. } | Item::Brief { .. })
    }
    pub fn toggle(&mut self) {
        match self {
            // enter shows more while there is more, then closes.
            Item::Tool { open, diff, .. } => {
                let hidden = diff.as_deref().is_some_and(|d| crate::diff::parse(d).len() > PREVIEW_LINES);
                *open = match *open {
                    Open::Closed => Open::Full,
                    Open::Preview if hidden => Open::Full,
                    _ => Open::Closed,
                };
            }
            Item::Brief { expanded, .. } => *expanded = !*expanded,
            _ => {}
        }
    }
}

pub struct Approval {
    pub request_id: String,
    pub tool: String,
    pub summary: String,
    pub input: serde_json::Value,
    pub tool_use_id: Option<String>,
}

/// A one-line yes/no question above the prompt, never a modal.
pub struct Ask {
    pub text: String,
    pub kind: AskKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AskKind {
    Switch(Agent),
}

/// What an agent knows about this thread: its native session and how far it has read.
#[derive(Default, Clone)]
pub struct Binding {
    pub session_id: Option<String>,
    pub model: Option<String>,
    /// Index of the first item this agent has not seen.
    pub seen_up_to: usize,
}

/// A switch was confirmed; the brief goes out with the next prompt.
pub struct PendingBrief {
    pub from: usize,
    pub delta: bool,
}

pub struct Thread {
    pub id: String,
    pub name: String,
    pub agent: Agent,
    pub status: Status,
    pub items: Vec<Item>,
    pub bindings: HashMap<Agent, Binding>,
    pub cost_usd: f64,
    pub turns: u32,
    /// Auto-follow the newest output. Turns off the moment the user scrolls up.
    pub follow: bool,
    /// First visible transcript line when not following.
    pub scroll: usize,
    /// Selected item while the transcript is focused.
    pub cursor: Option<usize>,
    pub approval: Option<Approval>,
    pub ask: Option<Ask>,
    pub pending_brief: Option<PendingBrief>,
    pub always: HashSet<String>,
    pub created: Instant,
    /// When this client first saw the current turn working; kept across a wait for approval.
    pub working_since: Option<Instant>,
}

impl Thread {
    pub fn new(agent: Agent) -> Self {
        Thread {
            id: new_thread_id(),
            name: String::new(),
            agent,
            status: Status::Idle,
            items: Vec::new(),
            bindings: HashMap::new(),
            cost_usd: 0.0,
            turns: 0,
            follow: true,
            scroll: 0,
            cursor: None,
            approval: None,
            ask: None,
            pending_brief: None,
            always: HashSet::new(),
            created: Instant::now(),
            working_since: None,
        }
    }

    pub fn binding(&self, agent: Agent) -> Option<&Binding> {
        self.bindings.get(&agent)
    }
    pub fn binding_mut(&mut self, agent: Agent) -> &mut Binding {
        self.bindings.entry(agent).or_default()
    }
    pub fn session_id(&self) -> Option<&str> {
        self.binding(self.agent).and_then(|b| b.session_id.as_deref())
    }
    pub fn model(&self) -> Option<&str> {
        self.binding(self.agent).and_then(|b| b.model.as_deref())
    }

    /// A new text block starts: open a fresh assistant item unless an empty one is already open.
    pub fn start_text(&mut self) {
        if let Some(Item::Assistant { text, streaming: true }) = self.items.last()
            && text.is_empty()
        {
            return;
        }
        self.items.push(Item::Assistant { text: String::new(), streaming: true });
    }

    pub fn push_text_delta(&mut self, delta: &str) {
        match self.items.last_mut() {
            Some(Item::Assistant { text, streaming: true }) => text.push_str(delta),
            _ => self.items.push(Item::Assistant { text: delta.to_string(), streaming: true }),
        }
    }

    /// The block is complete: replace the streamed text with the final text so the two never drift.
    pub fn finish_text(&mut self, full: &str) {
        // The last assistant item, looking past notes such as "interrupted".
        let last = self.items.iter_mut().rev().find(|i| !matches!(i, Item::Note(_)));
        match last {
            Some(Item::Assistant { text, streaming }) if *streaming => {
                *text = full.to_string();
                *streaming = false;
            }
            // An interrupted block arrives again as a final event; it is already on screen.
            Some(Item::Assistant { text, streaming: false }) if text.as_str() == full => {}
            _ if !full.is_empty() => {
                self.items.push(Item::Assistant { text: full.to_string(), streaming: false })
            }
            _ => {}
        }
    }

    pub fn close_streaming(&mut self) {
        if let Some(Item::Assistant { streaming, .. }) = self.items.last_mut() {
            *streaming = false;
        }
    }

    pub fn has_tool(&self, tool_id: &str) -> bool {
        self.items.iter().any(|i| matches!(i, Item::Tool { id, .. } if id == tool_id))
    }

    pub fn tool_mut(&mut self, tool_id: &str) -> Option<&mut Item> {
        self.items.iter_mut().rev().find(|i| matches!(i, Item::Tool { id, .. } if id == tool_id))
    }

    /// The tool line an approval refers to: by id when the request carries one, else the newest
    /// line still waiting for a result with the same verb and target.
    pub fn approval_tool_mut(&mut self, tool_use_id: Option<&str>, summary: &str) -> Option<&mut Item> {
        if let Some(id) = tool_use_id
            && self.has_tool(id)
        {
            return self.tool_mut(id);
        }
        self.items.iter_mut().rev().find(|i| {
            matches!(i, Item::Tool { verb, target, outcome: None, .. } if format!("{verb} {target}") == summary)
        })
    }

    pub fn first_queued(&mut self) -> Option<String> {
        for item in self.items.iter_mut() {
            if let Item::User { text, queued } = item
                && *queued
            {
                *queued = false;
                return Some(text.clone());
            }
        }
        None
    }

    pub fn has_queued(&self) -> bool {
        self.items.iter().any(|i| matches!(i, Item::User { queued: true, .. }))
    }

    pub fn busy(&self) -> bool {
        matches!(self.status, Status::Working | Status::Waiting)
    }
}

/// Short, typeable, git-style: `t-7f3a`.
pub fn new_thread_id() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    nanos.hash(&mut h);
    std::process::id().hash(&mut h);
    format!("t-{:04x}", h.finish() & 0xffff)
}

const STOP: &[&str] = &[
    "a", "an", "the", "to", "in", "on", "for", "of", "and", "or", "with", "my", "our", "this", "that",
    "please", "can", "could", "you", "me", "i", "we", "is", "it", "be", "do", "add", "make", "fix",
    "write", "create", "update", "change", "help", "want", "need", "new", "some", "all", "into",
    "from", "at", "by", "up", "so", "if", "then", "now", "read", "explain", "show", "tell", "run", "check",
    "look", "find", "implement", "refactor", "test", "describe", "summarize", "summarise", "list", "what",
    "how", "why", "does", "where", "which", "about", "file", "files", "code", "two", "three", "one",
    "sentence", "sentences", "short", "briefly", "its", "it's", "did", "should", "would", "use",
];

/// `src/auth/middleware.ts` names a thread `middleware`, not `src/auth/mid`.
fn stem(word: &str) -> String {
    let last = word.rsplit('/').next().unwrap_or(word);
    let no_ext = last.rsplit_once('.').map(|(a, _)| a).filter(|a| !a.is_empty()).unwrap_or(last);
    no_ext.to_string()
}

/// A short name from the first prompt: the first meaningful word, lowercased, at most 12 chars.
pub fn derive_name(prompt: &str) -> String {
    let words: Vec<String> = prompt
        .split(|c: char| c.is_whitespace() || c == ',' || c == '.' || c == ':' || c == ';' || c == '"' || c == '`')
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect();
    let pick = words
        .iter()
        .find(|w| w.len() > 2 && !STOP.contains(&w.as_str()))
        .or_else(|| words.first())
        .map(|w| stem(w))
        .unwrap_or_else(|| "untitled".to_string());
    pick.chars().take(12).collect()
}
