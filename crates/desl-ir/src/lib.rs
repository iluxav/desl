//! The provider-neutral event IR and the Thread and Binding types (SPEC.md "Core concepts and
//! data model"), the adapter contract (SPEC.md "Agent adapters"), and unified diffs.
//!
//! Nothing here knows an agent's native format. Adapters translate into these types and
//! nothing else crosses the boundary.

pub mod adapter;
pub mod diff;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The `active_agent` of a thread that runs the user's shell instead of an agent.
pub const SHELL: &str = "shell";

/// The status words, used identically in the sidebar, the CLI and notifications.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[serde(rename_all = "lowercase")]
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
    pub fn parse(s: &str) -> Option<Status> {
        Some(match s {
            "working" => Status::Working,
            "waiting" => Status::Waiting,
            "idle" => Status::Idle,
            "failed" => Status::Failed,
            "detached" => Status::Detached,
            _ => return None,
        })
    }
    /// A turn is in flight: the agent is generating, running a tool, or waiting on the user.
    pub fn busy(self) -> bool {
        matches!(self, Status::Working | Status::Waiting)
    }
}

/// What an agent may do without asking (SPEC.md "Permission modes").
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    Ask,
    #[default]
    Edits,
    Worktree,
    Yolo,
}

impl PermissionMode {
    pub fn word(self) -> &'static str {
        match self {
            PermissionMode::Ask => "ask",
            PermissionMode::Edits => "edits",
            PermissionMode::Worktree => "worktree",
            PermissionMode::Yolo => "yolo",
        }
    }
    /// The modes a thread can be put in today, mildest first. `worktree` joins them with worktrees.
    pub const CHOICES: [PermissionMode; 3] = [PermissionMode::Ask, PermissionMode::Edits, PermissionMode::Yolo];

    /// What the mode means, in a few words, for an agent that can ask before each call.
    pub fn meaning(self) -> &'static str {
        match self {
            PermissionMode::Ask => "every edit and command asks first",
            PermissionMode::Edits => "file edits are allowed, commands ask",
            PermissionMode::Worktree => "everything is allowed inside the worktree",
            PermissionMode::Yolo => "everything is allowed, nothing asks",
        }
    }

    pub fn parse(s: &str) -> Option<PermissionMode> {
        Some(match s {
            "ask" => PermissionMode::Ask,
            "edits" => PermissionMode::Edits,
            "worktree" => PermissionMode::Worktree,
            "yolo" => PermissionMode::Yolo,
            _ => return None,
        })
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Create,
    Modify,
    Delete,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Read,
    Edit,
    Command,
    Network,
    Destructive,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allow,
    Deny,
    Always,
}

impl Decision {
    pub fn allows(self) -> bool {
        !matches!(self, Decision::Deny)
    }
}

/// Tool names every adapter maps onto when it can; anything else keeps the agent's own name.
pub mod tool {
    pub const READ: &str = "read";
    pub const EDIT: &str = "edit";
    pub const BASH: &str = "bash";
    pub const SEARCH: &str = "search";
    pub const FETCH: &str = "fetch";
    pub const AGENT: &str = "agent";
    pub const TODO: &str = "todo";
}

/// The one line a tool call is about, from its neutral arguments: a path, a command's first
/// line, a pattern, a query, an address. Adapters fill `args` with these keys.
pub fn tool_target(name: &str, args: &Value) -> String {
    let get = |key: &str| args.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    let first_line = |s: String| s.lines().next().unwrap_or("").trim().to_string();
    match name {
        tool::READ | tool::EDIT => get("path"),
        tool::BASH => first_line(get("command")),
        tool::SEARCH => {
            let p = get("pattern");
            if p.is_empty() { get("query") } else { p }
        }
        tool::FETCH => get("url"),
        tool::AGENT => get("description"),
        tool::TODO => String::new(),
        _ => first_line(args.as_object().and_then(|o| o.values().find_map(|v| v.as_str().map(str::to_string))).unwrap_or_default()),
    }
}

/// The word a tool call is shown with: a shell command runs, everything else keeps its name.
pub fn tool_verb(name: &str) -> &str {
    if name == tool::BASH { "run" } else { name }
}

/// One entry of a thread's append-only log. `index` starts at 1, is dense and is never reused,
/// so a fork point or a high-water mark is always an exact reference.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Event {
    pub index: u64,
    pub thread_id: String,
    /// Milliseconds since the Unix epoch.
    pub timestamp: i64,
    /// The thread's active agent when the event was appended.
    pub agent: String,
    #[serde(flatten)]
    pub kind: EventKind,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventKind {
    /// What the user sent.
    UserMessage {
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<String>,
    },
    /// Final prose of one block of a turn. Streamed deltas are not stored.
    AssistantMessage {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    /// `name` is one of [`tool`]'s names or the agent's own; `args` uses the neutral keys
    /// `path`, `command`, `pattern`, `query`, `url`, `description` where they apply.
    ToolCall { id: String, name: String, args: Value },
    /// `output` is cut to an inline limit; when `truncated`, the full text is the blob `blob`.
    ToolResult {
        call_id: String,
        output: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        #[serde(default)]
        is_error: bool,
        #[serde(default)]
        truncated: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blob: Option<String>,
    },
    /// What changed in a file, as a unified diff when the adapter can tell. `call_id` ties it to
    /// the tool call that made the change.
    FileChange {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diff: Option<String>,
        change: ChangeKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
    },
    /// The agent asks before running a tool. `tool` is the tool's name, the key of the
    /// thread's "always" list.
    ApprovalRequest { call_id: String, tool: String, summary: String, risk: Risk },
    /// `by` is `user` or `policy`.
    ApprovalResponse { call_id: String, decision: Decision, by: String },
    AgentSwitch {
        from: String,
        to: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        brief_event_id: Option<u64>,
    },
    /// First event of a forked thread after the copied history.
    Fork { parent_id: String, fork_point: u64 },
    /// The handoff text that was actually sent. `covers_range` is the first and last event index
    /// it summarises.
    Brief { text: String, token_estimate: u64, covers_range: (u64, u64), delta: bool },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cost_estimate: Option<f64>,
    },
    /// Process crashes and adapter failures, as a complete sentence.
    Error { message: String, recoverable: bool },
    /// The user stopped the turn.
    Interrupt,
}

impl EventKind {
    /// The `kind` tag as stored and sent.
    pub fn name(&self) -> &'static str {
        match self {
            EventKind::UserMessage { .. } => "user_message",
            EventKind::AssistantMessage { .. } => "assistant_message",
            EventKind::ToolCall { .. } => "tool_call",
            EventKind::ToolResult { .. } => "tool_result",
            EventKind::FileChange { .. } => "file_change",
            EventKind::ApprovalRequest { .. } => "approval_request",
            EventKind::ApprovalResponse { .. } => "approval_response",
            EventKind::AgentSwitch { .. } => "agent_switch",
            EventKind::Fork { .. } => "fork",
            EventKind::Brief { .. } => "brief",
            EventKind::Usage { .. } => "usage",
            EventKind::Error { .. } => "error",
            EventKind::Interrupt => "interrupt",
        }
    }

    /// Part of the conversation an agent has to know about to continue it. Everything else is
    /// bookkeeping a brief leaves out.
    pub fn conversational(&self) -> bool {
        matches!(
            self,
            EventKind::UserMessage { .. } | EventKind::AssistantMessage { .. } | EventKind::ToolCall { .. } | EventKind::FileChange { .. }
        )
    }
}

/// What an agent knows about a thread: its native session and how far it has read.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Binding {
    pub agent: String,
    /// The agent's own id, resumable. Shown only on request, never required from the user.
    pub session_id: Option<String>,
    /// Index of the last event this agent has in context; 0 when it has seen nothing.
    pub seen_up_to: u64,
    /// The model the agent reported for its session.
    pub model: Option<String>,
    /// The model the user picked with `/model`; None follows the agent's own default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_choice: Option<String>,
    /// The reasoning effort the user picked with `/model`; None follows the agent's own setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort_choice: Option<String>,
    pub last_used_at: i64,
    /// How far into the agent's session file desl has read, in bytes (the native route).
    #[serde(default)]
    pub file_offset: u64,
}

/// One model an agent offers, as its own picker would show it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ModelInfo {
    /// What the agent takes as the model's name on its command line.
    pub id: String,
    pub name: String,
    pub description: String,
    /// The one the agent uses when the user has not picked.
    pub default: bool,
    /// The reasoning effort levels this model takes, lightest first. Empty when it has none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub efforts: Vec<Effort>,
    /// The level the agent uses when the user has not picked, when the agent says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
}

impl ModelInfo {
    /// A model known by name only: the agent's list has not been read.
    pub fn named(id: &str) -> ModelInfo {
        ModelInfo {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            default: false,
            efforts: Vec::new(),
            default_effort: None,
        }
    }
}

/// One reasoning effort level of a model, as the agent names it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Effort {
    /// What the agent takes on its command line: `low`, `high`.
    pub id: String,
    /// The agent's own line about the level; empty when it has none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// Branch state for the status bar, read by the daemon's git service.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct GitState {
    pub branch: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// How many paths `git status` lists: changed, staged, or new and not ignored.
    #[serde(default)]
    pub dirty: u32,
}

/// An approval the user still has to answer.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct PendingApproval {
    pub call_id: String,
    pub tool: String,
    pub summary: String,
}

/// One conversation with an agent, owned by desl.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Thread {
    /// Short, typeable, unique per workspace: `t-7f3a`, or `t-7f3a/1` for a fork.
    pub id: String,
    pub project_id: String,
    /// The thread's title: the active agent's own title for the conversation once it has
    /// written one, before that the first line of the first prompt; empty until then. A name the
    /// user gives it wins for good (SPEC.md "Interface design", Sidebar).
    pub name: String,
    /// The user named the thread: the agents' own titles no longer change it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub named: bool,
    pub cwd: String,
    pub worktree: Option<String>,
    pub parent_id: Option<String>,
    pub fork_point: Option<u64>,
    pub active_agent: String,
    pub status: Status,
    pub permission_mode: PermissionMode,
    pub created_at: i64,
    pub updated_at: i64,
    /// Index of the newest event; 0 for an empty thread.
    pub head: u64,
    pub bindings: BTreeMap<String, Binding>,
    /// Prompts sent while the agent was busy, oldest first.
    #[serde(default)]
    pub queued: Vec<String>,
    #[serde(default)]
    pub pending_approval: Option<PendingApproval>,
    #[serde(default)]
    pub git: GitState,
    /// Helpers the thread's agents started, oldest first, across switches. Read from the
    /// agents' files while they run, and stored with the thread.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subagents: Vec<Subagent>,
}

/// A helper agent that a thread's agent started.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct Subagent {
    pub id: String,
    /// The agent's own word for its kind: `general-purpose`, `Explore`.
    pub kind: String,
    pub description: String,
    pub finished: bool,
    /// Milliseconds since the epoch.
    pub started_at: i64,
    /// The agent and native session that started it: a thread keeps its subagents across
    /// switches, and each is read from the session it belongs to.
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub session_id: String,
}

impl Thread {
    pub fn binding(&self, agent: &str) -> Option<&Binding> {
        self.bindings.get(agent)
    }
    /// The active agent's native session id, if it has one.
    pub fn session_id(&self) -> Option<&str> {
        self.binding(&self.active_agent).and_then(|b| b.session_id.as_deref())
    }
    /// The active agent's model, if known.
    pub fn model(&self) -> Option<&str> {
        self.binding(&self.active_agent).and_then(|b| b.model.as_deref())
    }
}

/// A git repository desl knows about.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub path: String,
    pub default_agent: Option<String>,
    pub created_at: i64,
    /// The branch checked out in the project's folder and what is uncommitted there, read by the
    /// daemon every few seconds while a client is attached; not stored.
    #[serde(default)]
    pub git: GitState,
}

const STOP: &[&str] = &[
    "a",
    "an",
    "the",
    "to",
    "in",
    "on",
    "for",
    "of",
    "and",
    "or",
    "with",
    "my",
    "our",
    "this",
    "that",
    "please",
    "can",
    "could",
    "you",
    "me",
    "i",
    "we",
    "is",
    "it",
    "be",
    "do",
    "add",
    "make",
    "fix",
    "write",
    "create",
    "update",
    "change",
    "help",
    "want",
    "need",
    "new",
    "some",
    "all",
    "into",
    "from",
    "at",
    "by",
    "up",
    "so",
    "if",
    "then",
    "now",
    "read",
    "explain",
    "show",
    "tell",
    "run",
    "check",
    "look",
    "find",
    "implement",
    "refactor",
    "test",
    "describe",
    "summarize",
    "summarise",
    "list",
    "what",
    "how",
    "why",
    "does",
    "where",
    "which",
    "about",
    "file",
    "files",
    "code",
    "two",
    "three",
    "one",
    "sentence",
    "sentences",
    "short",
    "briefly",
    "its",
    "it's",
    "did",
    "should",
    "would",
    "use",
];

/// Longest thread name, in characters. The sidebar is narrow.
pub const MAX_NAME: usize = 48;

/// `src/auth/middleware.ts` names a thread `middleware`, not `src/auth/mid`.
fn stem(word: &str) -> String {
    let last = word.rsplit('/').next().unwrap_or(word);
    let no_ext = last.rsplit_once('.').map(|(a, _)| a).filter(|a| !a.is_empty()).unwrap_or(last);
    no_ext.to_string()
}

/// A thread's title from its first prompt: the first line, its spaces collapsed, cut at a
/// word to at most [`MAX_NAME`] characters.
pub fn derive_name(prompt: &str) -> String {
    let line = prompt.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.is_empty() {
        return "untitled".to_string();
    }
    let mut out = String::new();
    for w in words {
        let next = if out.is_empty() { w.chars().count() } else { out.chars().count() + 1 + w.chars().count() };
        if next > MAX_NAME {
            if out.is_empty() {
                out = w.chars().take(MAX_NAME).collect();
            }
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(w);
    }
    out
}

/// The one-word names desl gave threads before titles (until 2026-11-06), so a name that is
/// still one of them can be replaced with a title.
pub fn derive_word(prompt: &str) -> String {
    let words: Vec<String> = prompt
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | '.' | ':' | ';' | '"' | '`'))
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    let pick = words
        .iter()
        .find(|w| w.len() > 2 && !STOP.contains(&w.as_str()))
        .or_else(|| words.first())
        .map(|w| stem(w))
        .unwrap_or_else(|| "untitled".to_string());
    pick.chars().take(12).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_event_is_one_flat_object_tagged_by_kind() {
        let e = Event {
            index: 3,
            thread_id: "t-7f3a".into(),
            timestamp: 1,
            agent: "claude".into(),
            kind: EventKind::ToolCall { id: "c1".into(), name: tool::READ.into(), args: json!({ "path": "src/a.ts" }) },
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(
            v,
            json!({ "index": 3, "thread_id": "t-7f3a", "timestamp": 1, "agent": "claude",
                    "kind": "tool_call", "id": "c1", "name": "read", "args": { "path": "src/a.ts" } })
        );
        assert_eq!(serde_json::from_value::<Event>(v).unwrap(), e);
    }

    #[test]
    fn every_kind_survives_a_round_trip() {
        let kinds = vec![
            EventKind::UserMessage { text: "hi".into(), attachments: vec![] },
            EventKind::AssistantMessage { text: "hello".into(), model: Some("m".into()) },
            EventKind::ToolResult {
                call_id: "c".into(),
                output: "o".into(),
                exit_code: Some(2),
                is_error: true,
                truncated: false,
                blob: None,
            },
            EventKind::FileChange {
                path: "a".into(),
                diff: Some("@@ -1,1 +1,1 @@\n-a\n+b\n".into()),
                change: ChangeKind::Modify,
                call_id: Some("c".into()),
            },
            EventKind::ApprovalRequest { call_id: "c".into(), tool: "bash".into(), summary: "rm -rf dist/".into(), risk: Risk::Command },
            EventKind::ApprovalResponse { call_id: "c".into(), decision: Decision::Always, by: "user".into() },
            EventKind::AgentSwitch { from: "claude".into(), to: "codex".into(), brief_event_id: None },
            EventKind::Fork { parent_id: "t-1".into(), fork_point: 12 },
            EventKind::Brief { text: "# Handoff brief".into(), token_estimate: 4, covers_range: (1, 9), delta: false },
            EventKind::Usage { input_tokens: 1, output_tokens: 2, cost_estimate: Some(0.5) },
            EventKind::Error { message: "It stopped.".into(), recoverable: true },
            EventKind::Interrupt,
        ];
        for kind in kinds {
            let name = kind.name();
            let v = serde_json::to_value(&kind).unwrap();
            assert_eq!(v["kind"], name);
            assert_eq!(serde_json::from_value::<EventKind>(v).unwrap(), kind);
        }
    }

    #[test]
    fn targets_come_from_neutral_argument_keys() {
        assert_eq!(tool_target("read", &json!({ "path": "src/a.ts" })), "src/a.ts");
        assert_eq!(tool_target("bash", &json!({ "command": "pnpm test\nsecond line" })), "pnpm test");
        assert_eq!(tool_target("search", &json!({ "query": "jwt" })), "jwt");
        assert_eq!(tool_target("mcp.thing", &json!({ "n": 1, "what": "first string" })), "first string");
    }

    #[test]
    fn titles_are_the_first_line_cut_at_a_word() {
        assert_eq!(derive_name("Add refresh-token rotation to the auth middleware"), "Add refresh-token rotation to the auth");
        assert_eq!(derive_name("\n  Fix   the login\nmore detail"), "Fix the login");
        assert_eq!(derive_name(&"x".repeat(60)), "x".repeat(48));
        assert_eq!(derive_name(""), "untitled");
        assert_eq!(derive_word("Read src/auth/middleware.ts and explain it"), "middleware");
    }
}
