//! The adapter contract (SPEC.md "Agent adapters"). An adapter is the only code allowed to know
//! an agent's native format: it runs the agent headless, turns its stream into [`AdapterEvent`]s
//! and answers approvals. The daemon talks to every agent through these two traits.

use crate::{Decision, EventKind, ModelInfo, PermissionMode};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

/// What `detect` found on this machine.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct Detection {
    pub installed: bool,
    pub version: Option<String>,
    /// None when the agent has no way to say.
    pub logged_in: Option<bool>,
    pub binary_path: Option<String>,
    /// One sentence: how to install the agent, shown when it is missing.
    pub install_hint: String,
    /// One sentence: how to log in, shown when it is installed but logged out.
    pub login_hint: String,
    /// The newest agent version this adapter has fixtures for.
    pub fixtures_version: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Capabilities {
    pub resume: bool,
    pub fork: bool,
    pub model_switch: bool,
    pub compact: bool,
    pub interrupt: bool,
    /// Per-call approval requests reach desl. Without it the agent's own sandbox decides.
    pub approvals: bool,
    /// Replies arrive as token deltas, not whole.
    pub streaming: bool,
}

pub struct StartOpts {
    pub thread_id: String,
    pub thread_name: String,
    pub cwd: PathBuf,
    pub resume_session_id: Option<String>,
    /// The user's pick, by `ModelInfo::id`; None lets the agent use its default.
    pub model: Option<String>,
    /// The user's reasoning effort, by `Effort::id`; None lets the agent use its own setting.
    pub effort: Option<String>,
    pub permission_mode: PermissionMode,
}

/// How the agent's own interface opens a native session (the native route, SPEC.md "Agent
/// adapters").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Open {
    /// A new session. `session_id` is the id to give it, for an agent that takes one.
    New { session_id: Option<String> },
    /// An existing or desl-written session.
    Resume { session_id: String },
}

pub struct LaunchOpts {
    pub cwd: PathBuf,
    pub open: Open,
    /// As in [`StartOpts`].
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: PermissionMode,
}

/// The command that starts the agent's interface in a PTY.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
    /// Variables to leave out of the agent's environment, besides the secrets denylist.
    pub env_remove: Vec<String>,
    /// Secret-looking variables the agent logs in through, kept despite the denylist.
    pub keep: Vec<String>,
    /// The id the session will have, when the agent takes one from desl.
    pub session_id: Option<String>,
}

/// What one line of an agent's session file says (the native route).
#[derive(Clone, Debug, PartialEq)]
pub enum Reading {
    Event(EventKind),
    /// A turn began: the user sent something and the agent is on it.
    Working,
    /// The turn is over, finished or stopped.
    Idle,
}

/// Turns an agent's session file into events, one line at a time, as the agent appends them.
pub trait SessionReader: Send {
    fn line(&mut self, line: &str) -> Vec<Reading>;
}

pub struct Turn {
    pub text: String,
    pub attachments: Vec<String>,
    /// Text to put before `text` in the same user turn: a brief after a switch.
    pub prefix: Option<String>,
}

impl Turn {
    pub fn text(text: impl Into<String>) -> Turn {
        Turn { text: text.into(), attachments: Vec::new(), prefix: None }
    }
    /// The whole user turn as the agent receives it.
    pub fn payload(&self) -> String {
        match &self.prefix {
            Some(p) if !p.is_empty() => format!("{p}\n\n{}", self.text),
            _ => self.text.clone(),
        }
    }
}

/// What a session reports while it runs.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AdapterEvent {
    /// The native session exists: its id, to resume later, and the model behind it.
    Session { session_id: String, model: Option<String> },
    /// A new block of prose begins.
    TextStart,
    /// More of the current block, as it is generated. Never stored.
    TextDelta { text: String },
    /// An event for the thread's log.
    Event { event: EventKind },
    /// The turn is over. `error` is a sentence when it ended badly.
    TurnEnd { error: Option<String> },
    /// The agent's process is gone. `detail` is what it said last, if anything.
    Exit { code: Option<i32>, detail: String },
}

/// Where a session sends what it reports. Called from the adapter's own threads.
pub type Sink = Arc<dyn Fn(AdapterEvent) + Send + Sync>;

/// A failure the user will read: a complete sentence saying what happened and what to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterError(pub String);

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for AdapterError {}

pub trait Adapter: Send + Sync {
    /// `claude`, `codex`: the word the user types after `/switch`.
    fn id(&self) -> &'static str;
    fn detect(&self) -> Detection;
    fn capabilities(&self) -> Capabilities;
    /// What the agent does in `mode` when it cannot do what the mode says (SPEC.md "Permission
    /// modes": an adapter reports what it could not enforce). A few words that take the place
    /// of the mode's own meaning in the permissions list. None when the mode holds as written.
    fn permission_note(&self, _mode: PermissionMode) -> Option<String> {
        None
    }
    /// The models the agent offers, in the order its own picker shows them. May start the
    /// agent to ask, so it can take a second or two: never call it on the engine's thread.
    fn models(&self, cwd: &std::path::Path) -> Result<Vec<ModelInfo>, AdapterError>;
    /// Prepares a native session. Nothing is sent to the agent until the first `send`.
    fn start(&self, opts: StartOpts, sink: Sink) -> Result<Box<dyn Session>, AdapterError>;
    /// The session's file, and its id: by `session_id` when desl knows it, else the newest
    /// session in `cwd` begun after `since` (an agent that names its own sessions).
    fn session_file(&self, cwd: &std::path::Path, session_id: Option<&str>, since: std::time::SystemTime) -> Option<(String, PathBuf)> {
        let _ = (cwd, session_id, since);
        None
    }
    /// A reader for this agent's session files; None when the agent has none.
    fn session_reader(&self, cwd: &std::path::Path) -> Option<Box<dyn SessionReader>> {
        let _ = cwd;
        None
    }
    /// Whether the bottom of the agent's screen shows it asking the user something: its
    /// approval dialog or a question it waits on.
    fn waiting(&self, screen_tail: &str) -> bool {
        let _ = screen_tail;
        false
    }
    /// The command line of the agent's own interface for `opts` (the native route).
    fn launch(&self, opts: &LaunchOpts) -> Launch {
        let _ = opts;
        Launch { program: self.id().to_string(), args: Vec::new(), env_remove: Vec::new(), keep: Vec::new(), session_id: None }
    }
}

/// Whether a variable is kept out of every agent's environment (SPEC.md "Secrets").
pub fn is_secret(name: &str) -> bool {
    name == "AWS_SECRET_ACCESS_KEY" || name.ends_with("_TOKEN") || name.ends_with("_SECRET")
}

pub trait Session: Send {
    /// Sends one user turn. What follows arrives through the sink, ending with `TurnEnd` or `Exit`.
    fn send(&mut self, turn: Turn) -> Result<(), AdapterError>;
    /// Stops the current turn. True when the turn is over at once and no `TurnEnd` will follow.
    fn interrupt(&mut self) -> bool;
    fn answer(&mut self, call_id: &str, decision: Decision) -> Result<(), AdapterError>;
    /// Ends the native process, politely first. The native session stays resumable.
    fn close(&mut self);
}

/// Finds a binary on PATH.
pub fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(bin)).find(|p| p.is_file())
}

/// Sends SIGTERM to a process group, waits up to `grace`, then SIGKILL (SPEC.md "Process
/// hygiene"). Agent processes are started in their own group, so nothing they spawned survives.
pub fn terminate_group(child: &mut std::process::Child, grace: std::time::Duration) {
    let pgid = format!("-{}", child.id());
    let signal = |sig: &str| {
        let _ = std::process::Command::new("kill")
            .args([sig, "--", &pgid])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    };
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }
    signal("-TERM");
    let deadline = std::time::Instant::now() + grace;
    while std::time::Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    signal("-KILL");
    let _ = child.kill();
    let _ = child.wait();
}

/// Variables never handed to an agent process (SPEC.md "Secrets"): `AWS_SECRET_ACCESS_KEY` and
/// anything ending in `_TOKEN` or `_SECRET`, except the names in `keep`, which an adapter lists
/// when its own agent logs in through them.
pub fn scrub_env(cmd: &mut std::process::Command, keep: &[&str]) {
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if is_secret(&name) && !keep.contains(&name.as_ref()) {
            cmd.env_remove(&key);
        }
    }
}

/// What a dead process said, in a few words: its own error line if it printed one, else its
/// last words; never its "try --help" hint.
pub fn last_words(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    lines
        .iter()
        .find(|l| l.to_lowercase().starts_with("error"))
        .or_else(|| lines.iter().rev().find(|l| !l.contains("--help")))
        .map(|l| l.trim_start_matches("error:").trim_start_matches("Error:").trim().trim_end_matches('.').to_string())
        .unwrap_or_default()
}
