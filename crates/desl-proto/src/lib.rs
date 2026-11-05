//! The daemon protocol (SPEC.md "Architecture"): length-prefixed JSON frames over a Unix socket.
//! Documented in docs/protocol.md; this file is the definition.
//!
//! Three families. A `command` goes from client to daemon with a request id and gets exactly one
//! typed reply. An `event` goes from daemon to client as it happens: every IR event and every
//! workspace change. A `stream` carries token deltas for the thread a client is viewing.

pub mod paths;

use desl_ir::adapter::{Capabilities, Detection};
use desl_ir::{Decision, Event, Project, Status, Thread};
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
    NewThread {
        cwd: String,
        agent: Option<String>,
        prompt: Option<String>,
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
    /// `queued` when the agent was busy and the prompt waits its turn.
    Sent {
        queued: bool,
    },
    Created {
        thread_id: String,
    },
    Brief(BriefPreview),
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
}

impl AgentInfo {
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
