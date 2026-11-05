//! The background host: owns the agent processes and the append-only log for one project,
//! serves clients over a Unix socket, and keeps running when every client has gone.
//! A stand-in for the phase 1 daemon, built only so detach and reattach can be felt.

use crate::agent::claude::ClaudeProcess;
use crate::agent::codex::CodexProcess;
use crate::agent::{AgentEvent, AgentMsg, Process};
use crate::log::{BriefRecord, Command, Entry, Frame};
use crate::model::{Agent, Item, Status, new_thread_id};
use crate::paths;
use crate::projector;
use crate::workspace::Workspace;
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

enum HostMsg {
    Agent(AgentMsg),
    Command(u64, Command),
    Connected(u64, Arc<Mutex<UnixStream>>),
    Disconnected(u64),
}

struct Host {
    ws: Workspace,
    log: Vec<Entry>,
    log_file: File,
    procs: Vec<HashMap<Agent, Process>>,
    clients: HashMap<u64, Arc<Mutex<UnixStream>>>,
    view: Option<Value>,
    agent_tx: Sender<AgentMsg>,
}

pub fn load_log(path: &Path) -> Vec<Entry> {
    let Ok(f) = File::open(path) else { return Vec::new() };
    BufReader::new(f).lines().map_while(Result::ok).filter_map(|l| serde_json::from_str(&l).ok()).collect()
}

pub fn run(cwd: PathBuf) -> Result<()> {
    let sock = paths::socket_path(&cwd);
    if let Some(dir) = sock.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let log_path = paths::log_path(&cwd);
    if let Some(dir) = log_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Another host for this project already running? Then there is nothing to do.
    if UnixStream::connect(&sock).is_ok() {
        eprintln!("a desl host is already serving {}", cwd.display());
        return Ok(());
    }
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock).with_context(|| format!("binding {}", sock.display()))?;

    let (tx, rx) = channel::<HostMsg>();
    let (agent_tx, agent_rx) = channel::<AgentMsg>();
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for m in agent_rx {
                if tx.send(HostMsg::Agent(m)).is_err() {
                    break;
                }
            }
        });
    }
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut next_id = 1u64;
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let id = next_id;
                next_id += 1;
                let reader = match stream.try_clone() {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                let writer = Arc::new(Mutex::new(stream));
                if tx.send(HostMsg::Connected(id, writer)).is_err() {
                    return;
                }
                let tx = tx.clone();
                std::thread::spawn(move || {
                    for line in BufReader::new(reader).lines() {
                        let Ok(line) = line else { break };
                        if let Ok(cmd) = serde_json::from_str::<Command>(&line)
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

    let mut host = Host {
        ws: Workspace::new(cwd.clone()),
        log: Vec::new(),
        log_file: OpenOptions::new().create(true).append(true).open(&log_path)?,
        procs: Vec::new(),
        clients: HashMap::new(),
        view: None,
        agent_tx,
    };
    // Replay what an earlier host left behind, then tell the truth about agents that are gone.
    for e in load_log(&log_path) {
        host.ws.apply(&e);
        host.log.push(e);
    }
    host.procs.resize_with(host.ws.threads.len(), HashMap::new);
    let busy: Vec<usize> = host.ws.threads.iter().enumerate().filter(|(_, t)| t.busy()).map(|(i, _)| i).collect();
    for idx in busy {
        host.append(Entry::HostRestarted { idx });
    }
    if host.ws.threads.is_empty() {
        host.append(Entry::ThreadCreated { idx: 0, id: new_thread_id(), agent: Agent::Claude });
    }
    eprintln!("desl host serving {} on {}", cwd.display(), sock.display());

    let keep_going = host.serve(rx);
    for procs in host.procs.iter_mut() {
        for p in procs.values_mut() {
            p.kill();
        }
    }
    let _ = std::fs::remove_file(&sock);
    eprintln!("desl host stopped ({})", if keep_going { "channel closed" } else { "shutdown" });
    Ok(())
}

impl Host {
    fn serve(&mut self, rx: Receiver<HostMsg>) -> bool {
        for msg in rx {
            match msg {
                HostMsg::Connected(id, w) => {
                    self.clients.insert(id, w);
                }
                HostMsg::Disconnected(id) => {
                    self.clients.remove(&id);
                }
                HostMsg::Agent(AgentMsg { idx, agent, ev }) => self.on_agent(idx, agent, ev),
                HostMsg::Command(id, cmd) => {
                    if !self.on_command(id, cmd) {
                        return false;
                    }
                }
            }
        }
        true
    }

    fn send_to(&self, id: u64, frame: &Frame) {
        if let Some(w) = self.clients.get(&id)
            && let Ok(mut s) = w.lock()
        {
            let mut line = serde_json::to_string(frame).unwrap_or_default();
            line.push('\n');
            let _ = s.write_all(line.as_bytes());
        }
    }

    fn broadcast(&self, frame: &Frame) {
        let mut line = serde_json::to_string(frame).unwrap_or_default();
        line.push('\n');
        for w in self.clients.values() {
            if let Ok(mut s) = w.lock() {
                let _ = s.write_all(line.as_bytes());
            }
        }
    }

    /// The one write path: file, projection, clients, in that order.
    fn append(&mut self, e: Entry) -> Option<String> {
        if let Ok(mut line) = serde_json::to_string(&e) {
            line.push('\n');
            let _ = self.log_file.write_all(line.as_bytes());
        }
        let out = self.ws.apply(&e);
        if self.procs.len() < self.ws.threads.len() {
            self.procs.resize_with(self.ws.threads.len(), HashMap::new);
        }
        self.broadcast(&Frame::Entry(e.clone()));
        self.log.push(e);
        out
    }

    fn on_command(&mut self, client: u64, cmd: Command) -> bool {
        match cmd {
            Command::Attach => {
                let frame = Frame::Snapshot { entries: self.log.clone(), view: self.view.clone() };
                self.send_to(client, &frame);
            }
            Command::Prompt { idx, text } => {
                let Some(t) = self.ws.threads.get(idx) else { return true };
                if t.busy() {
                    self.append(Entry::Prompt { idx, text, queued: true });
                } else {
                    self.append(Entry::Prompt { idx, text: text.clone(), queued: false });
                    self.dispatch(idx, &text);
                }
            }
            Command::Interrupt { idx } => {
                let Some(t) = self.ws.threads.get(idx) else { return true };
                if !t.busy() {
                    return true;
                }
                let agent = t.agent;
                let mut ended = false;
                if let Some(p) = self.procs[idx].get_mut(&agent) {
                    let _ = p.interrupt();
                    ended = p.exits_per_turn();
                }
                if ended {
                    self.procs[idx].remove(&agent);
                }
                self.append(Entry::Interrupted { idx, ended });
            }
            Command::Answer { idx, request_id, allow, always } => {
                let Some(t) = self.ws.threads.get(idx) else { return true };
                let Some(a) = t.approval.as_ref().filter(|a| a.request_id == request_id) else { return true };
                let input = a.input.clone();
                let agent = t.agent;
                if let Some(p) = self.procs[idx].get_mut(&agent) {
                    let _ = p.answer(&request_id, allow, &input);
                }
                self.append(Entry::Decision { idx, request_id, allow, always });
            }
            Command::Switch { idx, to } => {
                let Some(t) = self.ws.threads.get(idx) else { return true };
                if t.agent == to || t.busy() {
                    return true;
                }
                let len = t.items.len();
                let (from, delta) = match t.binding(to) {
                    Some(b) if b.session_id.is_some() => (b.seen_up_to.min(len), true),
                    _ => (0, false),
                };
                let unseen = &t.items[from..];
                let nothing_new = delta
                    && !unseen.iter().any(|i| matches!(i, Item::User { .. } | Item::Assistant { .. } | Item::Tool { .. }));
                let brief_from = if nothing_new { None } else { Some(from) };
                self.append(Entry::Switched { idx, to, brief_from, delta });
            }
            Command::NewThread => {
                let idx = self.ws.threads.len();
                self.append(Entry::ThreadCreated { idx, id: new_thread_id(), agent: Agent::Claude });
            }
            Command::Rename { idx, name } => {
                if idx < self.ws.threads.len() {
                    self.append(Entry::Renamed { idx, name });
                }
            }
            Command::ShowBrief { idx } => {
                let Some(t) = self.ws.threads.get(idx) else { return true };
                let to = t.agent.other();
                let (from, delta) = match t.binding(to) {
                    Some(b) if b.session_id.is_some() => (b.seen_up_to.min(t.items.len()), true),
                    _ => (0, false),
                };
                let brief = projector::build(&t.items[from..], &self.ws.cwd, None, delta);
                let text = format!("brief desl would send to {} now", to.name());
                let record = BriefRecord { label: text.clone(), text: brief.text, tokens: brief.token_estimate };
                self.append(Entry::Note { idx, text, brief: Some(record) });
            }
            Command::Note { idx, text } => {
                if idx < self.ws.threads.len() {
                    self.append(Entry::Note { idx, text, brief: None });
                }
            }
            Command::SaveView { view } => self.view = Some(view),
            Command::Shutdown => {
                self.broadcast(&Frame::Bye);
                return false;
            }
        }
        true
    }

    fn dispatch(&mut self, idx: usize, text: &str) {
        let agent = self.ws.threads[idx].agent;
        // A confirmed switch rides along with this prompt as the brief's "Next step".
        let (payload, brief) = match self.ws.threads[idx].pending_brief.as_ref() {
            Some(pb) => {
                let items = &self.ws.threads[idx].items;
                let end = items.len().saturating_sub(1); // the prompt itself is the next step, not history
                let b = projector::build(&items[pb.from.min(end)..end], &self.ws.cwd, Some(text), pb.delta);
                let label = format!("{}brief sent to {}", if pb.delta { "delta " } else { "" }, agent.name());
                (b.text.clone(), Some(BriefRecord { label, text: b.text, tokens: b.token_estimate }))
            }
            None => (text.to_string(), None),
        };
        if !self.procs[idx].contains_key(&agent) {
            let session = self.ws.threads[idx].binding(agent).and_then(|b| b.session_id.clone());
            let spawned = match agent {
                Agent::Claude => {
                    ClaudeProcess::spawn(&self.ws.cwd, session.as_deref(), self.agent_tx.clone(), idx).map(Process::Claude)
                }
                Agent::Codex => Ok(Process::Codex(CodexProcess::new(&self.ws.cwd, session, self.agent_tx.clone(), idx))),
            };
            match spawned {
                Ok(p) => {
                    self.procs[idx].insert(agent, p);
                }
                Err(e) => {
                    let message = format!("{e:#}. Install {} and log in, then send your prompt again.", agent.name());
                    self.append(Entry::Failed { idx, message });
                    return;
                }
            }
        }
        if let Some(p) = self.procs[idx].get_mut(&agent)
            && let Err(e) = p.send_user(&payload)
        {
            self.procs[idx].remove(&agent);
            let message = format!("Could not send the prompt to {} ({e}). Send it again to start a new process.", agent.name());
            self.append(Entry::Failed { idx, message });
            return;
        }
        self.append(Entry::Dispatched { idx, agent, brief });
    }

    fn on_agent(&mut self, idx: usize, agent: Agent, ev: AgentEvent) {
        if idx >= self.ws.threads.len() {
            return;
        }
        let was_busy = self.ws.threads[idx].busy();
        let active = self.ws.threads[idx].agent == agent;
        // Side effects that need the event before it is applied.
        let auto_allow = match &ev {
            AgentEvent::ApprovalRequest { request_id, tool_name, input, .. }
                if self.ws.threads[idx].always.contains(tool_name) =>
            {
                Some((request_id.clone(), input.clone()))
            }
            _ => None,
        };
        let exited = matches!(ev, AgentEvent::Exited { .. });
        let exit_info = match &ev {
            AgentEvent::Exited { code, stderr_tail } => Some((*code, stderr_tail.clone())),
            _ => None,
        };
        let turn_done = matches!(ev, AgentEvent::TurnDone { .. });

        self.append(Entry::Agent { idx, agent, ev });

        if let Some((request_id, input)) = auto_allow
            && let Some(p) = self.procs[idx].get_mut(&agent)
        {
            let _ = p.answer(&request_id, true, &input);
        }
        if exited {
            let per_turn = self.procs[idx].get(&agent).is_some_and(Process::exits_per_turn);
            self.procs[idx].remove(&agent);
            if active && was_busy {
                let (code, tail) = exit_info.unwrap_or((None, String::new()));
                // The agent's own error line if it printed one, else its last words; never its "try --help" hint.
                let lines: Vec<&str> = tail.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
                let why = lines
                    .iter()
                    .find(|l| l.to_lowercase().starts_with("error"))
                    .or_else(|| lines.iter().rev().find(|l| !l.contains("--help")))
                    .map(|l| l.trim_start_matches("error:").trim_start_matches("Error:").trim().trim_end_matches('.').to_string())
                    .unwrap_or_default();
                let code = code.map(|c| c.to_string()).unwrap_or_else(|| "a signal".into());
                let mut message = format!("{} exited with {code} mid-turn.", agent.name());
                if !why.is_empty() {
                    message.push_str(&format!(" It said: {why}."));
                }
                message.push_str(if per_turn { " Send your prompt again to retry." } else { " Send your prompt again to resume the session." });
                self.append(Entry::Failed { idx, message });
            }
        }
        if turn_done && active && self.ws.threads[idx].has_queued() {
            if let Some(text) = self.append(Entry::Dequeued { idx }) {
                self.dispatch(idx, &text);
            }
        }
        let _ = Status::Idle;
    }
}

/// Starts a host for `cwd` in its own session so closing the terminal never reaches it.
pub fn spawn_detached(cwd: &Path) -> Result<()> {
    let exe = std::env::current_exe()?;
    let log = paths::host_stderr_path(cwd);
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let out = OpenOptions::new().create(true).append(true).open(&log)?;
    let err = out.try_clone()?;
    let mut cmd = if which("setsid") {
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

fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
        .unwrap_or(false)
}
