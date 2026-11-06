//! The daemon protocol (SPEC.md "Architecture"): length-prefixed JSON frames over a Unix socket.
//! Documented in docs/protocol.md; this file is the definition.
//!
//! Three families. A `command` goes from client to daemon with a request id and gets exactly one
//! typed reply. An `event` goes from daemon to client as it happens: every IR event and every
//! workspace change. A `stream` carries token deltas for the thread a client is viewing, and a
//! `pty` frame the raw terminal output of the thread a client shows (the native route).

pub mod b64;
pub mod paths;
pub mod term;

use desl_ir::adapter::{Capabilities, Detection};
use desl_ir::{Decision, Event, ModelInfo, PermissionMode, Project, Status, Thread};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read, Write};

/// The major version. The daemon refuses a client whose major differs and tells it to upgrade.
pub const PROTOCOL: u32 = 1;

/// No frame may be larger than this. A thread with thousands of events fits many times over.
pub const MAX_FRAME: usize = 256 * 1024 * 1024;

/// Writes one frame: a 4-byte big-endian length, then that many bytes of JSON.
pub fn write_frame<W: Write, T: Serialize>(w: &mut W, value: &T) -> io::Result<()> {
    w.write_all(&encode(value)?)?;
    w.flush()
}

/// One frame as bytes, for sending the same frame to many clients.
pub fn encode<T: Serialize>(value: &T) -> io::Result<Vec<u8>> {
    let body = serde_json::to_vec(value).map_err(io::Error::other)?;
    let len = u32::try_from(body.len()).map_err(|_| io::Error::other("frame too large"))?;
    let mut out = Vec::with_capacity(body.len() + 4);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Reads one frame. `Ok(None)` when the other side closed the connection between frames.
pub fn read_frame<R: Read, T: DeserializeOwned>(r: &mut R) -> io::Result<Option<T>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::other("frame too large"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    serde_json::from_slice(&body).map(Some).map_err(io::Error::other)
}

/// Client to daemon.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub command: Command,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    /// First frame on every connection.
    Hello {
        protocol: u32,
        client: String,
    },
    /// The workspace as it is now: projects, threads, agents. No subscription.
    Workspace,
    /// The workspace, and from now on every change to it. `cwd` registers that directory's
    /// repository as a project and makes sure it has a thread to open.
    Subscribe {
        cwd: Option<String>,
    },
    /// A thread's events, and its token stream from now on. One viewed thread per client.
    OpenThread {
        thread_id: String,
    },
    /// Moves this client's token stream to a thread whose events it already has.
    View {
        thread_id: String,
    },
    Prompt {
        thread_id: String,
        text: String,
    },
    Interrupt {
        thread_id: String,
    },
    /// Answers the thread's pending approval. `call_id` guards against answering a newer one.
    Answer {
        thread_id: String,
        call_id: Option<String>,
        decision: Decision,
    },
    Switch {
        thread_id: String,
        agent: String,
    },
    /// A new thread with this one's history up to event `at` (the head when absent).
    Fork {
        thread_id: String,
        agent: Option<String>,
        at: Option<u64>,
    },
    /// `agent` "shell" opens a shell instead of an agent.
    NewThread {
        cwd: String,
        agent: Option<String>,
        prompt: Option<String>,
        /// The thread the user is on. A shell opens in that thread's project, at `cwd`. When
        /// `beside` is itself a shell, a new agent opens where that shell is now, as a project
        /// of its own, and the shell moves into that project.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        beside: Option<String>,
    },
    Rename {
        thread_id: String,
        name: String,
    },
    RemoveThread {
        thread_id: String,
    },
    /// The brief desl would send if the thread switched to `to` now (the other agent when absent).
    Brief {
        thread_id: String,
        to: Option<String>,
    },
    /// A fresh native session on the same thread; history is kept.
    Clear {
        thread_id: String,
    },
    /// The models the thread's active agent offers, and which one this thread uses.
    Models {
        thread_id: String,
    },
    /// Picks a model for the thread's active agent, by `ModelInfo::id` or by name. `None` goes
    /// back to the agent's default. Takes effect with the next prompt.
    SetModel {
        thread_id: String,
        model: Option<String>,
    },
    /// Picks a reasoning effort for the thread's active agent, by `Effort::id`. `None` goes back
    /// to the agent's own setting. Takes effect with the next prompt.
    SetEffort {
        thread_id: String,
        effort: Option<String>,
    },
    /// What the thread's agents may do without asking. The broker follows it at once; an
    /// agent's own flags follow with its next turn.
    SetMode {
        thread_id: String,
        mode: PermissionMode,
    },
    /// Replies when the thread is waiting, idle or failed.
    Wait {
        thread_id: String,
    },
    /// Detected agents. `refresh` runs the detection again.
    Agents {
        refresh: bool,
    },
    /// Recent prompts across every thread, newest first.
    History {
        limit: u32,
    },
    /// Files of the thread's project, for the `@` picker.
    Files {
        thread_id: String,
    },
    /// A thread's events, without opening it.
    Events {
        thread_id: String,
    },
    /// The full text of a truncated tool output.
    Blob {
        hash: String,
    },
    /// What the client wants restored on its next attach: current thread, focus, scroll positions.
    SaveView {
        view: Value,
    },
    /// Store and process checks for `desl doctor`.
    Doctor,
    /// Shows a thread in this client's terminal: its agent's interface starts if it is not
    /// running, at this size. The reply carries what the agent has drawn so far; `pty` frames
    /// follow with everything it draws after. `answers` is what the client's terminal said to
    /// the questions agents ask it, for the daemon to give while no client shows the agent.
    Attach {
        thread_id: String,
        rows: u16,
        cols: u16,
        #[serde(default)]
        answers: term::Answers,
    },
    /// Keys for the agent of the thread this client shows, as the terminal sent them (base64).
    /// Never replied to.
    Input {
        thread_id: String,
        data: String,
    },
    /// The client's terminal changed size. Never replied to.
    Resize {
        thread_id: String,
        rows: u16,
        cols: u16,
    },
    /// The conversation of one of a thread's subagents, read-only. Replied with `events`.
    Subagent {
        thread_id: String,
        subagent_id: String,
    },
    /// Stops every agent and the daemon.
    Shutdown,
}

/// Daemon to client.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    /// Exactly one of `ok` and `error` is set. `error` is a complete sentence.
    Reply {
        id: u64,
        ok: Option<Reply>,
        error: Option<String>,
    },
    Event {
        change: Change,
    },
    Stream {
        thread_id: String,
        delta: StreamDelta,
    },
    /// What the shown thread's agent drew, as raw terminal bytes (base64).
    Pty {
        thread_id: String,
        data: String,
    },
    /// The shown thread's agent quit. `code` is its exit status when it had one.
    PtyEnded {
        thread_id: String,
        code: Option<u32>,
    },
    /// The thread moved to another agent, which has drawn the conversation: replace the whole
    /// screen with `data` (base64) in one frame. `pty` frames that follow are the new agent's.
    PtyReplace {
        thread_id: String,
        agent: String,
        data: String,
    },
    /// A sentence for the user about the shown thread, such as why a switch did not happen.
    Notice {
        thread_id: String,
        text: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Reply {
    Hello {
        protocol: u32,
        version: String,
        pid: u32,
    },
    Workspace(Box<Workspace>),
    /// `partial` is the text of a block still being generated, for a client that joins mid-turn.
    Thread {
        thread: Box<Thread>,
        events: Vec<Event>,
        partial: Option<String>,
    },
    Events {
        events: Vec<Event>,
    },
    /// `partial` as in `Thread`.
    Viewing {
        thread_id: String,
        partial: Option<String>,
    },
    Done,
    /// The answer to `attach`: everything the agent has drawn so far (base64), to put on the
    /// screen before the `pty` frames that follow; `agent` is who is drawing it.
    Attached {
        thread_id: String,
        agent: String,
        replay: String,
    },
    /// `queued` when the agent was busy and the prompt waits its turn.
    Sent {
        queued: bool,
    },
    Created {
        thread_id: String,
    },
    Brief(BriefPreview),
    /// `current` is the id of the model this thread uses: the user's pick, else the default.
    /// `effort` is the user's pick of a reasoning effort; None follows the agent's own setting.
    Models {
        agent: String,
        models: Vec<ModelInfo>,
        current: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
    },
    /// The answer to `set_model` and `set_effort`: what the thread's agent will use from its
    /// next turn. `model` is None on the agent's default model, `effort` is None on the agent's
    /// own effort setting.
    ModelSet {
        agent: String,
        model: Option<ModelInfo>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
    },
    /// The thread's permission mode, and in words what it means with the thread's active agent.
    ModeSet {
        mode: PermissionMode,
        meaning: String,
    },
    Waited {
        status: Status,
    },
    Agents {
        agents: Vec<AgentInfo>,
    },
    History {
        prompts: Vec<PastPrompt>,
    },
    Files {
        files: Vec<String>,
    },
    Blob {
        text: String,
    },
    Doctor {
        checks: Vec<Check>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct Workspace {
    pub projects: Vec<Project>,
    pub threads: Vec<Thread>,
    pub agents: Vec<AgentInfo>,
    pub view: Option<Value>,
    /// The project of the directory the client subscribed from.
    pub project_id: Option<String>,
    /// The thread to open first: the one the client last had open there, else the newest.
    pub focus: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct AgentInfo {
    pub id: String,
    pub detection: Detection,
    pub capabilities: Capabilities,
    /// Permission modes this agent cannot hold as written, each with what it does instead.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mode_notes: Vec<ModeNote>,
}

/// What an agent does in a permission mode it cannot enforce as written.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ModeNote {
    pub mode: PermissionMode,
    pub note: String,
}

impl AgentInfo {
    /// What `mode` means with this agent: its own note where it has one, else the mode's meaning.
    pub fn mode_meaning(&self, mode: PermissionMode) -> String {
        self.mode_notes.iter().find(|n| n.mode == mode).map_or_else(|| mode.meaning().to_string(), |n| n.note.clone())
    }
    /// Installed, and not known to be logged out.
    pub fn usable(&self) -> bool {
        self.detection.installed && self.detection.logged_in != Some(false)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BriefPreview {
    pub to: String,
    pub text: String,
    pub token_estimate: u64,
    /// User turns the brief covers.
    pub turns: u64,
    pub delta: bool,
    /// False when the receiving agent has already seen everything; nothing would be sent.
    pub needed: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PastPrompt {
    pub thread_id: String,
    pub text: String,
}

/// One line of `desl doctor`.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    /// What was found, and what to do when `ok` is false.
    pub detail: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "what", rename_all = "snake_case")]
pub enum Change {
    Project {
        project: Project,
    },
    /// A thread was created or its name, status, agent, bindings, queue or approval changed.
    Thread {
        thread: Box<Thread>,
    },
    ThreadRemoved {
        thread_id: String,
    },
    /// An event was appended to a thread's log.
    Appended {
        event: Event,
    },
    Agents {
        agents: Vec<AgentInfo>,
    },
    /// The daemon is stopping.
    Bye,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamDelta {
    /// A new block of prose begins.
    Start,
    Text {
        text: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_end_cleanly() {
        let mut buf = Vec::new();
        let req = Request { id: 7, command: Command::Prompt { thread_id: "t-7f3a".into(), text: "hi".into() } };
        write_frame(&mut buf, &req).unwrap();
        write_frame(&mut buf, &Request { id: 8, command: Command::Workspace }).unwrap();
        let mut r = &buf[..];
        let a: Request = read_frame(&mut r).unwrap().unwrap();
        assert!(matches!(a, Request { id: 7, command: Command::Prompt { .. } }));
        let b: Request = read_frame(&mut r).unwrap().unwrap();
        assert!(matches!(b, Request { id: 8, command: Command::Workspace }));
        assert!(read_frame::<_, Request>(&mut r).unwrap().is_none());
    }

    #[test]
    fn a_request_is_one_flat_object() {
        let v =
            serde_json::to_value(Request { id: 1, command: Command::Switch { thread_id: "t-1".into(), agent: "codex".into() } }).unwrap();
        assert_eq!(v, serde_json::json!({ "id": 1, "command": "switch", "thread_id": "t-1", "agent": "codex" }));
    }

    #[test]
    fn a_truncated_frame_is_an_error_not_a_clean_end() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &Request { id: 1, command: Command::Workspace }).unwrap();
        buf.truncate(buf.len() - 2);
        assert!(read_frame::<_, Request>(&mut &buf[..]).is_err());
    }
}
