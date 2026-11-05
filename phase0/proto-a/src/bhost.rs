//! Prototype B's background host: owns the agents' PTYs and session-file tails for one project,
//! decides status, performs switches, and replays each pane's bytes to whoever attaches.
//! A stand-in for the phase 1 daemon's PTY host, built only so detach and reattach can be felt.

use crate::b64;
use crate::model::{Agent, Item, Status, Thread, derive_name};
use crate::paths;
use crate::projector;
use crate::pty::{Pane, PaneMsg, keys, widget};
use crate::transcript::{self, Tail, TranscriptEvent};
use crate::workspace::apply_agent;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

pub const PROTO: &str = "b";
/// Bytes of pane output kept per agent for replay on attach.
const REPLAY_CAP: usize = 2 * 1024 * 1024;
/// How long an agent's screen may stand still before a "working" status is no longer believed.
const STILL_MEANS_IDLE: Duration = Duration::from_secs(5);

/// Text that means the agent's own interface is asking the user something.
pub const WAITING_PATTERNS: &[&str] = &[
    "Do you want to proceed",
    "Do you want to",
    "Do you trust",
    "Is this a project you",
    "Enter to confirm",
    "Yes, and don't ask again",
    "Allow once",
    "Allow for this session",
    "Approve",
    "[y/n",
    "(y/n",
    "Press Enter to continue",
];
/// Claude Code's permission dialog, whose options desl can answer with y, n and a. The question varies
/// ("Do you want to proceed?", "Do you want to create probe.txt?"); the first option does not.
pub const CLAUDE_PERMISSION: &str = "Do you want to";
const CLAUDE_FIRST_OPTION: &str = "1. Yes";

/// True while Claude Code's own permission dialog is on screen: its question and its first option.
/// The question alone is not enough, since an answer can end with "Do you want to proceed?".
pub fn claude_dialog(tail: &str) -> bool {
    tail.contains(CLAUDE_PERMISSION) && tail.contains(CLAUDE_FIRST_OPTION)
}

/// The text on screen that says an agent is asking the user something, if any.
fn asking(tail: &str) -> Option<&'static str> {
    WAITING_PATTERNS
        .iter()
        .copied()
        .find(|pat| tail.contains(pat) && (!pat.starts_with(CLAUDE_PERMISSION) || tail.contains(CLAUDE_FIRST_OPTION)))
}
/// Codex's approval prompt answers y and n itself.
pub const CODEX_APPROVAL: &[&str] = &["Allow", "Approve", "[y/n", "(y/n"];

#[derive(Serialize, Deserialize, Debug)]
pub enum BCommand {
    Attach { rows: u16, cols: u16 },
    Resize { rows: u16, cols: u16 },
    Input { idx: usize, agent: Agent, data: String },
    NewThread { agent: Agent },
    SwitchAsk { idx: usize, to: Agent },
    Switch { idx: usize, to: Agent },
    Slash { idx: usize, text: String },
    SaveView { view: Value },
    Shutdown,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ThreadSnap {
    pub id: String,
    pub name: String,
    pub agent: Agent,
    pub status: Status,
    /// Agents with a live interface for this thread, with an exit code once one has ended.
    pub panes: Vec<(Agent, Option<Option<u32>>)>,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum BFrame {
    Threads { threads: Vec<ThreadSnap>, view: Option<Value> },
    /// Everything a pane has printed so far: reset the screen, then feed.
    Replay { idx: usize, agent: Agent, data: String },
    Output { idx: usize, agent: Agent, data: String },
    Ask { idx: usize, to: Agent, text: String },
    Text { title: String, lines: Vec<String> },
    Notice { text: String, error: bool },
    Bell,
    Bye,
}

struct Source {
    /// The session file this pane will write, when the agent lets desl choose it up front.
    expect: Option<PathBuf>,
    tail: Option<Tail>,
    claude: transcript::claude::State,
    codex: transcript::codex::State,
    last_search: Instant,
}

impl Source {
    fn new() -> Self {
        Source { expect: None, tail: None, claude: Default::default(), codex: Default::default(), last_search: Instant::now() - Duration::from_secs(5) }
    }
}

struct Slot {
    pane: Pane,
    source: Source,
    raw: Vec<u8>,
}

enum HostMsg {
    Pane(usize, Agent, PaneMsg),
    Command(u64, BCommand),
    Connected(u64, Arc<Mutex<UnixStream>>),
    Disconnected(u64),
    Tick,
}

struct Host {
    cwd: PathBuf,
    threads: Vec<Thread>,
    slots: Vec<HashMap<Agent, Slot>>,
    clients: HashMap<u64, Arc<Mutex<UnixStream>>>,
    view: Option<Value>,
    size: (u16, u16),
    last_snapshot: String,
    tx: Sender<HostMsg>,
}

fn fmt_tokens(n: usize) -> String {
    if n >= 1000 { format!("~{:.1}k tokens", n as f64 / 1000.0) } else { format!("~{n} tokens") }
}

pub fn run(cwd: PathBuf) -> Result<()> {
    let sock = paths::socket_path_for(&cwd, PROTO);
    if let Some(dir) = sock.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if UnixStream::connect(&sock).is_ok() {
        eprintln!("a desl host is already serving {}", cwd.display());
        return Ok(());
    }
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).with_context(|| format!("binding {}", sock.display()))?;
    let (tx, rx) = channel::<HostMsg>();
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut next_id = 1u64;
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let id = next_id;
                next_id += 1;
                let Ok(reader) = stream.try_clone() else { continue };
                if tx.send(HostMsg::Connected(id, Arc::new(Mutex::new(stream)))).is_err() {
                    return;
                }
                let tx = tx.clone();
                std::thread::spawn(move || {
                    for line in BufReader::new(reader).lines() {
                        let Ok(line) = line else { break };
                        if let Ok(cmd) = serde_json::from_str::<BCommand>(&line)
                            && tx.send(HostMsg::Command(id, cmd)).is_err()
                        {
                            return;
                        }
                    }
                    let _ = tx.send(HostMsg::Disconnected(id));
                });
            }
        });
    }
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(150));
                if tx.send(HostMsg::Tick).is_err() {
                    break;
                }
            }
        });
    }
    let mut host = Host {
        cwd: cwd.clone(),
        threads: Vec::new(),
        slots: Vec::new(),
        clients: HashMap::new(),
        view: None,
        size: (24, 80),
        last_snapshot: String::new(),
        tx,
    };
    eprintln!("desl host (prototype B) serving {} on {}", cwd.display(), sock.display());
    for msg in rx {
        match msg {
            HostMsg::Connected(id, w) => {
                host.clients.insert(id, w);
            }
            HostMsg::Disconnected(id) => {
                host.clients.remove(&id);
            }
            HostMsg::Pane(idx, agent, PaneMsg::Output(_, bytes)) => host.on_output(idx, agent, bytes),
            HostMsg::Pane(idx, agent, PaneMsg::Exited(..)) => {
                if let Some(s) = host.slots.get_mut(idx).and_then(|m| m.get_mut(&agent)) {
                    s.pane.mark_exited();
                }
                host.refresh_status();
            }
            HostMsg::Tick => host.tick(),
            HostMsg::Command(id, cmd) => {
                if !host.on_command(id, cmd) {
                    break;
                }
            }
        }
        host.publish_threads();
    }
    for s in host.slots.iter_mut().flat_map(|m| m.values_mut()) {
        s.pane.kill();
    }
    let _ = std::fs::remove_file(&sock);
    eprintln!("desl host (prototype B) stopped");
    Ok(())
}

impl Host {
    fn send_to(&self, id: u64, frame: &BFrame) {
        if let Some(w) = self.clients.get(&id)
            && let Ok(mut s) = w.lock()
        {
            let mut line = serde_json::to_string(frame).unwrap_or_default();
            line.push('\n');
            let _ = s.write_all(line.as_bytes());
        }
    }

    fn broadcast(&self, frame: &BFrame) {
        let mut line = serde_json::to_string(frame).unwrap_or_default();
        line.push('\n');
        for w in self.clients.values() {
            if let Ok(mut s) = w.lock() {
                let _ = s.write_all(line.as_bytes());
            }
        }
    }

    fn notice(&self, text: impl Into<String>) {
        self.broadcast(&BFrame::Notice { text: text.into(), error: false });
    }

    fn snapshot(&self) -> Vec<ThreadSnap> {
        self.threads
            .iter()
            .enumerate()
            .map(|(i, t)| ThreadSnap {
                id: t.id.clone(),
                name: t.name.clone(),
                agent: t.agent,
                status: t.status,
                panes: {
                    let mut v: Vec<(Agent, Option<Option<u32>>)> = self.slots[i].iter().map(|(a, s)| (*a, s.pane.exited)).collect();
                    v.sort_by_key(|(a, _)| a.name());
                    v
                },
            })
            .collect()
    }

    /// Sends the thread list whenever it differs from what clients last saw.
    fn publish_threads(&mut self) {
        let threads = self.snapshot();
        let key = serde_json::to_string(&threads).unwrap_or_default();
        if key != self.last_snapshot {
            self.last_snapshot = key;
            self.broadcast(&BFrame::Threads { threads, view: None });
        }
    }

    fn ensure_pane(&mut self, idx: usize, agent: Agent) -> bool {
        if self.slots[idx].contains_key(&agent) {
            return true;
        }
        let (rows, cols) = self.size;
        let (ptx, prx) = channel::<PaneMsg>();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            for m in prx {
                if tx.send(HostMsg::Pane(idx, agent, m)).is_err() {
                    break;
                }
            }
        });
        let mut source = Source::new();
        let args: Vec<String> = match agent {
            Agent::Claude => {
                let id = transcript::claude::new_session_id();
                source.expect = Some(transcript::claude::session_path(&self.cwd, &id));
                transcript::claude::session_args(&id)
            }
            // Codex names its own rollout; it is found by directory and start time once it exists.
            Agent::Codex => Vec::new(),
        };
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        match Pane::spawn(idx, agent.name(), &args, &self.cwd, rows, cols, ptx) {
            Ok(pane) => {
                self.slots[idx].insert(agent, Slot { pane, source, raw: Vec::new() });
                true
            }
            Err(e) => {
                self.threads[idx].status = Status::Failed;
                self.broadcast(&BFrame::Notice {
                    text: format!("{e:#}. Install {} and log in, then try again.", agent.name()),
                    error: true,
                });
                false
            }
        }
    }

    fn new_thread(&mut self, agent: Agent) {
        let idx = self.threads.len();
        self.threads.push(Thread::new(agent));
        self.slots.push(HashMap::new());
        self.ensure_pane(idx, agent);
    }

    fn on_output(&mut self, idx: usize, agent: Agent, bytes: Vec<u8>) {
        let Some(s) = self.slots.get_mut(idx).and_then(|m| m.get_mut(&agent)) else { return };
        s.pane.feed(&bytes);
        s.raw.extend_from_slice(&bytes);
        if s.raw.len() > REPLAY_CAP {
            let cut = s.raw.len() - REPLAY_CAP;
            s.raw.drain(..cut);
        }
        self.broadcast(&BFrame::Output { idx, agent, data: b64::encode(&bytes) });
        self.refresh_status();
    }

    fn tick(&mut self) {
        self.poll_transcripts();
        self.flush_pastes();
        self.refresh_status();
    }

    fn poll_transcripts(&mut self) {
        let cwd = self.cwd.clone();
        for idx in 0..self.threads.len() {
            let agents: Vec<Agent> = self.slots[idx].keys().copied().collect();
            for agent in agents {
                let taken: Vec<PathBuf> = self
                    .slots
                    .iter()
                    .flat_map(|m| m.values())
                    .filter_map(|s| s.source.tail.as_ref().map(|t| t.path.clone()))
                    .collect();
                let Some(slot) = self.slots[idx].get_mut(&agent) else { continue };
                let src = &mut slot.source;
                if src.tail.is_none() {
                    if src.last_search.elapsed() < Duration::from_millis(1000) {
                        continue;
                    }
                    src.last_search = Instant::now();
                    let since = slot.pane.spawned_at.checked_sub(Duration::from_secs(2)).unwrap_or(SystemTime::UNIX_EPOCH);
                    let found = match (&src.expect, agent) {
                        (Some(path), _) => path.exists().then(|| path.clone()),
                        (None, Agent::Claude) => transcript::claude::find_session(&cwd, since, &taken),
                        (None, Agent::Codex) => transcript::codex::find_session(&cwd, since, &taken),
                    };
                    match found {
                        Some(path) => src.tail = Some(Tail::new(path)),
                        None => continue,
                    }
                }
                let lines = src.tail.as_mut().map(|t| t.read_new_lines()).unwrap_or_default();
                if lines.is_empty() {
                    continue;
                }
                let mut events = Vec::new();
                for line in &lines {
                    match agent {
                        Agent::Claude => events.extend(transcript::claude::parse_line(&mut src.claude, line)),
                        Agent::Codex => events.extend(transcript::codex::parse_line(&mut src.codex, line)),
                    }
                }
                let t = &mut self.threads[idx];
                let before = t.status;
                for ev in events {
                    match ev {
                        TranscriptEvent::User(text) => {
                            // A pasted brief arrives as part of the user's message; keep only what they typed.
                            let typed = match text.find("# Handoff brief") {
                                Some(0) => text.rsplit("\n\n").next().unwrap_or("").to_string(),
                                _ => text.clone(),
                            };
                            if t.name.is_empty() && !typed.trim_start().starts_with('/') && !typed.trim().is_empty() {
                                t.name = derive_name(&typed);
                            }
                            t.items.push(Item::User { text: typed, queued: false });
                            t.turns += 1;
                            let len = t.items.len();
                            t.binding_mut(agent).seen_up_to = len;
                        }
                        TranscriptEvent::TurnStarted => {
                            if t.agent == agent && t.status != Status::Waiting {
                                t.status = Status::Working;
                            }
                        }
                        TranscriptEvent::Agent(ev) => apply_agent(t, agent, &ev, &cwd),
                    }
                }
                if t.status != before {
                    eprintln!("{} {}: {} -> {} (session file)", chrono::Local::now().format("%H:%M:%S"), t.id, before.word(), t.status.word());
                }
            }
        }
    }

    /// Pastes a waiting brief once the target interface is quiet and not asking anything.
    fn flush_pastes(&mut self) {
        for idx in 0..self.threads.len() {
            let agents: Vec<Agent> = self.slots[idx].keys().copied().collect();
            for agent in agents {
                let Some(slot) = self.slots[idx].get_mut(&agent) else { continue };
                let p = &mut slot.pane;
                if p.pending_paste.is_none() || p.exited.is_some() {
                    continue;
                }
                let tail = widget::tail_text(p.parser.screen(), 14);
                let waiting = asking(&tail).is_some();
                if waiting || !p.saw_output || p.last_output.elapsed() < Duration::from_millis(900) {
                    continue;
                }
                let text = p.pending_paste.take().unwrap_or_default();
                let bracketed = p.parser.screen().bracketed_paste();
                p.write(&keys::paste(&text, bracketed));
                let t = &mut self.threads[idx];
                let len = t.items.len();
                t.binding_mut(agent).seen_up_to = len;
                self.notice(format!(
                    "The brief is in {}'s prompt as a paste. Type your question after it and press enter.",
                    agent.name()
                ));
            }
        }
    }

    fn refresh_status(&mut self) {
        let mut ring = false;
        for idx in 0..self.threads.len() {
            let agent = self.threads[idx].agent;
            let Some(slot) = self.slots[idx].get(&agent) else { continue };
            let p = &slot.pane;
            let has_transcript = slot.source.tail.is_some();
            let t = &mut self.threads[idx];
            let before = t.status;
            let tail = widget::tail_text(p.parser.screen(), 14);
            // An agent asks for permission only in the middle of a turn. Once its session file says the turn
            // is over, the same words on screen are just the end of its answer ("Do you want to proceed?").
            let asked = asking(&tail);
            let waiting = asked.is_some() && (!has_transcript || before != Status::Idle);
            let recent = p.last_output.elapsed() < Duration::from_millis(1500);
            let echo = p.last_output.saturating_duration_since(p.last_input) < Duration::from_millis(500);
            let booting = p.spawned_at.elapsed().map(|d| d < Duration::from_secs(6)).unwrap_or(false);
            let next = if let Some(code) = p.exited {
                if code == Some(0) { Status::Idle } else { Status::Failed }
            } else if waiting {
                Status::Waiting
            } else if has_transcript {
                // The session file says when turns start and end; the screen only adds "waiting".
                match before {
                    Status::Waiting => Status::Working,
                    Status::Failed => Status::Failed,
                    // Both interfaces animate while they work. One that has stood still this long is not
                    // working, whatever its file last said (a turn can end without a line desl recognises).
                    Status::Working if p.last_output.elapsed() > STILL_MEANS_IDLE => Status::Idle,
                    other => other,
                }
            } else if recent && !echo && !booting && p.pending_paste.is_none() {
                Status::Working
            } else {
                Status::Idle
            };
            if next != before {
                // One line per change in the host log, with the screen text that caused a "waiting".
                let why = asked.filter(|_| next == Status::Waiting).map(|pat| format!(" (screen shows \"{pat}\")")).unwrap_or_default();
                eprintln!("{} {}: {} -> {}{why}", chrono::Local::now().format("%H:%M:%S"), t.id, before.word(), next.word());
                t.status = next;
                ring |= next == Status::Waiting;
            }
        }
        if ring {
            self.broadcast(&BFrame::Bell);
        }
    }

    fn plan(&self, idx: usize, to: Agent) -> (usize, bool) {
        let t = &self.threads[idx];
        match t.binding(to) {
            Some(b) if b.session_id.is_some() => (b.seen_up_to.min(t.items.len()), true),
            _ => (0, false),
        }
    }

    fn switch_ask(&mut self, client: u64, idx: usize, to: Agent) {
        let Some(t) = self.threads.get(idx) else { return };
        if t.agent == to {
            self.send_to(client, &BFrame::Notice { text: format!("This thread is already on {}.", to.name()), error: false });
            return;
        }
        if t.busy() {
            self.send_to(
                client,
                &BFrame::Notice { text: format!("{} is still working. Wait for it to finish, then switch.", t.agent.name()), error: false },
            );
            return;
        }
        let (from, delta) = self.plan(idx, to);
        let unseen = &t.items[from..];
        let has_new = unseen.iter().any(|i| matches!(i, Item::User { .. } | Item::Assistant { .. } | Item::Tool { .. }));
        if delta && !has_new {
            self.perform_switch(idx, to, false);
            return;
        }
        if !has_new {
            self.send_to(
                client,
                &BFrame::Notice {
                    text: format!(
                        "Nothing to hand over yet: desl has not seen a turn in {}'s session file. Send one prompt first.",
                        t.agent.name()
                    ),
                    error: false,
                },
            );
            return;
        }
        let brief = projector::build(unseen, &self.cwd, None, delta);
        let turns = brief.turns.max(1);
        let s = if turns == 1 { "" } else { "s" };
        let text = if delta {
            format!("Handing {turns} new turn{s} to {} as a delta brief ({}). Continue?", to.name(), fmt_tokens(brief.token_estimate))
        } else {
            format!("Handing {turns} turn{s} to {} as a brief ({}). Continue?", to.name(), fmt_tokens(brief.token_estimate))
        };
        self.send_to(client, &BFrame::Ask { idx, to, text });
    }

    fn perform_switch(&mut self, idx: usize, to: Agent, with_brief: bool) {
        if idx >= self.threads.len() || !self.ensure_pane(idx, to) {
            return;
        }
        let (from, delta) = self.plan(idx, to);
        let t = &mut self.threads[idx];
        t.items.push(Item::Divider(format!("switched to {}", to.name())));
        t.agent = to;
        t.status = Status::Idle;
        if with_brief {
            let end = t.items.len().saturating_sub(1);
            let b = projector::build(&t.items[from.min(end)..end], &self.cwd, None, delta);
            let label = format!("{}brief pasted into {}", if delta { "delta " } else { "" }, to.name());
            t.items.push(Item::Brief { label, text: b.text.clone(), tokens: b.token_estimate, expanded: false });
            if let Some(slot) = self.slots[idx].get_mut(&to) {
                slot.pane.pending_paste = Some(b.text);
            }
            self.notice(format!("Switched to {}. The brief will be pasted into its prompt when it is ready.", to.name()));
        } else {
            self.notice(format!("Switched to {}; it has seen everything already.", to.name()));
        }
    }

    fn slash(&mut self, client: u64, idx: usize, text: &str) {
        let Some(t) = self.threads.get(idx) else { return };
        let mut parts = text.splitn(2, ' ');
        let cmd = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("").trim().to_string();
        match cmd {
            "/brief" => {
                let to = t.agent.other();
                let (from, delta) = self.plan(idx, to);
                let brief = projector::build(&t.items[from..], &self.cwd, None, delta);
                let title = format!("brief desl would send to {} now ({})", to.name(), fmt_tokens(brief.token_estimate));
                self.send_to(client, &BFrame::Text { title, lines: brief.text.lines().map(str::to_string).collect() });
            }
            "/info" => {
                let mut lines = vec![format!("{} · on {} · {} turns · {} items", t.id, t.agent.name(), t.turns, t.items.len())];
                for agent in [Agent::Claude, Agent::Codex] {
                    if let Some(b) = t.binding(agent) {
                        lines.push(format!(
                            "{}: session {} · seen {} of {} items",
                            agent.name(),
                            b.session_id.clone().unwrap_or_else(|| "not started".into()),
                            b.seen_up_to,
                            t.items.len()
                        ));
                    }
                    if let Some(s) = self.slots[idx].get(&agent) {
                        lines.push(format!(
                            "{} session file: {}",
                            agent.name(),
                            s.source.tail.as_ref().map(|t| t.path.display().to_string()).unwrap_or_else(|| "not found yet".into())
                        ));
                    }
                }
                self.send_to(client, &BFrame::Text { title: "info".into(), lines });
            }
            "/rename" => {
                if arg.is_empty() {
                    self.send_to(client, &BFrame::Notice { text: "Give the thread a name: /rename <name>.".into(), error: false });
                } else {
                    self.threads[idx].name = arg.chars().take(12).collect();
                }
            }
            other => self.send_to(client, &BFrame::Notice { text: format!("{other} is not built yet in prototype B."), error: false }),
        }
    }

    fn on_command(&mut self, client: u64, cmd: BCommand) -> bool {
        match cmd {
            BCommand::Attach { rows, cols } => {
                self.resize(rows, cols);
                if self.threads.is_empty() {
                    self.new_thread(Agent::Claude);
                }
                self.send_to(client, &BFrame::Threads { threads: self.snapshot(), view: self.view.clone() });
                for (idx, m) in self.slots.iter().enumerate() {
                    for (agent, slot) in m {
                        self.send_to(client, &BFrame::Replay { idx, agent: *agent, data: b64::encode(&slot.raw) });
                    }
                }
            }
            BCommand::Resize { rows, cols } => self.resize(rows, cols),
            BCommand::Input { idx, agent, data } => {
                if let Some(s) = self.slots.get_mut(idx).and_then(|m| m.get_mut(&agent)) {
                    s.pane.write(&b64::decode(&data));
                }
            }
            BCommand::NewThread { agent } => self.new_thread(agent),
            BCommand::SwitchAsk { idx, to } => self.switch_ask(client, idx, to),
            BCommand::Switch { idx, to } => self.perform_switch(idx, to, true),
            BCommand::Slash { idx, text } => self.slash(client, idx, &text),
            BCommand::SaveView { view } => self.view = Some(view),
            BCommand::Shutdown => {
                self.broadcast(&BFrame::Bye);
                return false;
            }
        }
        true
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        if rows == 0 || cols == 0 || (rows, cols) == self.size {
            return;
        }
        self.size = (rows, cols);
        for s in self.slots.iter_mut().flat_map(|m| m.values_mut()) {
            s.pane.resize(rows, cols);
        }
    }
}

/// Starts a prototype B host for `cwd` in its own session so closing the terminal never reaches it.
pub fn spawn_detached(cwd: &Path) -> Result<()> {
    let exe = std::env::current_exe()?;
    let log = paths::host_stderr_path_for(cwd, PROTO);
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let out = std::fs::OpenOptions::new().create(true).append(true).open(&log)?;
    let err = out.try_clone()?;
    let has_setsid = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d.join("setsid").is_file())).unwrap_or(false);
    let mut cmd = if has_setsid {
        let mut c = std::process::Command::new("setsid");
        c.arg(&exe);
        c
    } else {
        std::process::Command::new(&exe)
    };
    cmd.arg("--host").arg(cwd).stdin(std::process::Stdio::null()).stdout(out).stderr(err);
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn().context("starting the desl host")?;
    Ok(())
}

/// Connects to the project's B host, starting one when `start` is set.
pub fn connect(cwd: &Path, start: bool) -> Result<Option<UnixStream>> {
    let sock = paths::socket_path_for(cwd, PROTO);
    if let Ok(s) = UnixStream::connect(&sock) {
        return Ok(Some(s));
    }
    if !start {
        return Ok(None);
    }
    spawn_detached(cwd)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(s) = UnixStream::connect(&sock) {
            return Ok(Some(s));
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    anyhow::bail!("the desl host did not start within five seconds; see {}", paths::host_stderr_path_for(cwd, PROTO).display())
}
