//! The thread engine (SPEC.md "Architecture"): the core state machine. It takes a prompt,
//! appends the event, resolves the thread's adapter, checks the high-water mark, asks the
//! projector for a brief when the agent is behind, sends the turn, consumes the adapter's
//! events, appends them, updates the status and bumps the mark.
//!
//! One engine owns all state and runs on one thread, so every write to a thread is serialised.
//! Agents run in their own processes and report through a channel. The approval broker and the
//! event bus live here too: every store write is published to the subscribed clients.

use crate::handover::{Given, Giving, Kept};
use crate::pty::Pty;
use crate::store::{Store, StoredForm};
use crate::{git, projector};
use desl_ir::adapter::{
    Adapter, AdapterEvent, Launch, LaunchOpts, McpServer, Open, Reading, Session, SessionReader, Sink, StartOpts, Turn, which,
};
use desl_ir::{
    Binding, Decision, Event, EventKind, FILES, GitState, MAX_NAME, ModelInfo, OpenForm, PendingApproval, PermissionMode, Project, SHELL,
    Status, TASK_LIST, TASKS, Thread, derive_name, is_agent, program_word,
};
use desl_proto::{
    AgentInfo, BriefPreview, Change, Check, Command, ModeNote, PROTOCOL, Reply, Request, ServerFrame, StreamDelta, Workspace, b64, config,
    form, term,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub type ClientId = u64;
/// Encoded frames on their way to one client. Unbounded: a slow client never blocks the engine.
pub type Outbox = Sender<Arc<Vec<u8>>>;

pub enum Msg {
    Connected {
        client: ClientId,
        out: Outbox,
    },
    Disconnected {
        client: ClientId,
    },
    Request {
        client: ClientId,
        request: Request,
    },
    Adapter {
        thread_id: String,
        agent: String,
        generation: u64,
        event: AdapterEvent,
    },
    /// An agent's model list arrived; `client` and `id` are the request waiting for it.
    Models {
        client: ClientId,
        id: u64,
        thread_id: String,
        agent: String,
        result: Result<Vec<ModelInfo>, String>,
    },
    /// An agent's interface drew something. `generation` tells a restarted agent from the old one.
    PtyOutput {
        thread_id: String,
        generation: u64,
        data: Vec<u8>,
    },
    /// An agent's interface closed its terminal.
    PtyExit {
        thread_id: String,
        generation: u64,
    },
    /// The git worker read these directories' branch and uncommitted count.
    Git {
        states: Vec<(String, GitState)>,
    },
    /// Every [`TICK_MS`]: switches check their hidden agent; every second, session files.
    Tick,
}

struct Client {
    out: Outbox,
    subscribed: bool,
    /// The thread whose token stream this client receives.
    viewing: Option<String>,
    /// The threads whose agents' interfaces this client shows (the native route), one per pane.
    shows: HashSet<String>,
}

/// The turn in flight on a thread.
struct TurnState {
    agent: String,
    /// The turn resumed a native session; if nothing comes back, the session is gone.
    resumed: bool,
    /// The agent has said or done something this turn.
    heard: bool,
    /// This is already the second try after a failed resume.
    retried: bool,
    prompt: String,
    /// Index of the prompt's `user_message` event.
    user_index: u64,
}

/// Following one running agent's session file (the native route).
struct Tail {
    agent: String,
    reader: Box<dyn SessionReader>,
    /// None until the file is found: Codex writes nothing before its first turn.
    path: Option<PathBuf>,
    /// When the agent started, for an agent that names its own session.
    since: SystemTime,
    /// The thread's log already holds this session's history (from before desl read files):
    /// start reading at the end of the file.
    skip_existing: bool,
    /// The start of a line the agent has not finished writing.
    partial: Vec<u8>,
    last_poll: Instant,
    /// When the agent's own title was last asked for, for an agent that keeps it outside the file.
    title_asked: Option<Instant>,
}

/// How often an agent that keeps its titles outside the session file is asked for one.
const TITLE_EVERY: Duration = Duration::from_secs(5);

/// How often the engine ticks, in milliseconds. A switch is ready within one tick of its
/// hidden agent going quiet.
pub const TICK_MS: u64 = 100;
const TICKS_PER_SECOND: u64 = 1000 / TICK_MS;

/// A switch in progress: the receiving agent drawing out of sight until it is ready.
struct Pending {
    to: String,
    generation: u64,
    pty: Pty,
    started: Instant,
    started_at: SystemTime,
    last_output: Instant,
    saw_output: bool,
    /// The copied session, or None when the copy failed and the brief goes instead.
    copied: Option<(String, u64)>,
    brief: Option<String>,
}

impl Pending {
    /// Drawn and then quiet; never longer than 15 seconds.
    fn ready(&self) -> bool {
        (self.saw_output
            && self.last_output.elapsed() >= Duration::from_millis(600)
            && self.started.elapsed() >= Duration::from_millis(1200))
            || self.started.elapsed() >= Duration::from_secs(15)
    }
}

/// A prompt being typed into an agent's own interface: the text first, enter a tick later, so
/// the agent never takes the enter as part of a paste.
struct Typing {
    text: String,
    typed: Option<Instant>,
}

/// A prompt typed into an agent that has not started a turn by then is given up on.
const TURN_START_LIMIT: Duration = Duration::from_secs(30);

/// The lines of the screen's bottom that are checked for a question from the agent.
const ASKING_ROWS: usize = 14;

/// What exists only while the daemon runs.
#[derive(Default)]
struct Live {
    /// Native sessions by agent, each with the generation its events are tagged with.
    sessions: HashMap<String, (u64, Box<dyn Session>)>,
    /// The text of a block still being generated, for a client that opens the thread mid-turn.
    partial: Option<String>,
    /// Tools the user answered "always" for. Read from the log on first use.
    always: Option<HashSet<String>>,
    turn: Option<TurnState>,
    /// Agents whose session was started under settings that have since changed. It is replaced
    /// before its next turn, never in the middle of one.
    stale: HashSet<String>,
}

pub struct Engine {
    store: Store,
    adapters: Vec<Arc<dyn Adapter>>,
    agents: Vec<AgentInfo>,
    projects: Vec<Project>,
    threads: Vec<Thread>,
    live: HashMap<String, Live>,
    clients: HashMap<ClientId, Client>,
    /// `desl wait`: (client, request id, thread).
    waiters: Vec<(ClientId, u64, String)>,
    /// Each agent's model list, asked for once and kept while the daemon runs.
    models: HashMap<String, Vec<ModelInfo>>,
    tx: Sender<Msg>,
    generation: u64,
    /// Each thread's running agent interface, with the generation its output is tagged with.
    ptys: HashMap<String, (u64, Pty)>,
    /// What the user's terminal answers to the questions agents ask, from the last attach.
    answers: term::Answers,
    /// The session file of each thread's running agent.
    tails: HashMap<String, Tail>,
    /// Switches in progress, by thread.
    pending: HashMap<String, Pending>,
    /// When the user last pressed enter in each thread's agent: a turn it starts shows in the
    /// agent's files only a moment later.
    entered: HashMap<String, Instant>,
    /// A prompt from the CLI on its way into an agent's own interface, by thread.
    typing: HashMap<String, Typing>,
    /// Prompts typed into an agent whose turn has not shown in its files yet, by thread.
    awaiting_turn: HashMap<String, Instant>,
    /// Threads whose agent was ended to start again on the same conversation when its exit arrives.
    restarting: HashSet<String>,
    /// Threads whose `waiting` began with a dialog on their agent's screen; one waiting on a form
    /// is not here, and stays waiting while its screen asks nothing.
    asked_on_screen: HashSet<String>,
    /// The socket this daemon serves; agents' `desl mcp` connect to it.
    socket: Option<PathBuf>,
    /// A git worker is reading; the next refresh waits for its answer.
    git_busy: bool,
    /// The user's settings file; none in tests, which run on the defaults.
    config_file: Option<PathBuf>,
    /// The program a files thread runs: `yazi`. Tests name one every machine has.
    file_manager: String,
    /// What a tasks thread runs, before `--project <root>`: this desl's own `task ui`. Tests
    /// name a program every machine has.
    tasks_program: Vec<String>,
    ticks: u64,
    pub stopping: bool,
    /// A handover asked for and not yet made.
    handover: Option<Handover>,
}

/// A client asked the daemon to start again from another binary and keep every agent running.
struct Handover {
    client: ClientId,
    id: u64,
    exe: PathBuf,
    asked: Instant,
}

/// How long a handover waits for what is half done to finish: a prompt on its way into an
/// agent, a switch whose agent is starting, an agent starting again.
const HANDOVER_WAIT: Duration = Duration::from_secs(20);

/// A path with the home directory written `~`, for sentences.
fn tilde(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.to_string(),
    }
}

/// The user's own shell, with their whole environment: it is theirs, not an agent's.
fn shell_launch() -> Launch {
    Launch {
        program: std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "bash".into()),
        args: Vec::new(),
        env_remove: Vec::new(),
        keep: std::env::vars_os().map(|(k, _)| k.to_string_lossy().into_owned()).collect(),
        session_id: None,
    }
}

/// The user's own file manager, with their whole environment and their own configuration, as
/// their shell: it opens on the directory it starts in.
fn files_launch(program: &str) -> Launch {
    Launch { program: program.to_string(), ..shell_launch() }
}

/// What a tasks thread runs: `desl task ui` from this daemon's own binary, by the path it was
/// started from, so after an install the new one starts.
fn task_list_program() -> Vec<String> {
    let exe =
        std::env::current_exe().map(|p| p.to_string_lossy().trim_end_matches(" (deleted)").to_string()).unwrap_or_else(|_| "desl".into());
    vec![exe, "task".into(), "ui".into()]
}

/// desl's task list for the project at `root`, with the user's environment, as their shell.
fn tasks_launch(program: &[String], root: &str) -> Launch {
    let mut args: Vec<String> = program.iter().skip(1).cloned().collect();
    args.extend(["--project".to_string(), root.to_string()]);
    Launch { program: program.first().cloned().unwrap_or_else(|| "desl".into()), args, ..shell_launch() }
}

/// The shell's name for the sidebar: `bash`, `zsh`.
fn shell_name() -> String {
    let program = shell_launch().program;
    Path::new(&program).file_name().map_or(program.clone(), |n| n.to_string_lossy().into_owned())
}

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn detect(adapters: &[Arc<dyn Adapter>]) -> Vec<AgentInfo> {
    adapters
        .iter()
        .map(|a| AgentInfo {
            id: a.id().to_string(),
            detection: a.detect(),
            capabilities: a.capabilities(),
            mode_notes: PermissionMode::CHOICES
                .into_iter()
                .filter_map(|mode| Some(ModeNote { mode, note: a.permission_note(mode)? }))
                .collect(),
        })
        .collect()
}

/// Where the store keeps the order the user gave the projects: a JSON list of their ids.
const PROJECT_ORDER: &str = "project_order";

type Outcome = Result<Reply, String>;

impl Engine {
    pub fn new(store: Store, adapters: Vec<Arc<dyn Adapter>>, tx: Sender<Msg>) -> anyhow::Result<Engine> {
        let mut engine = Engine::build(store, adapters, tx)?;
        engine.settle();
        Ok(engine)
    }

    /// The engine of a daemon that takes over from the one before it: every program that one
    /// handed over is running in its thread already. Nothing is read from their terminals
    /// until [`Engine::read_terminals`].
    pub fn taking_over(
        store: Store,
        adapters: Vec<Arc<dyn Adapter>>,
        tx: Sender<Msg>,
        answers: term::Answers,
        given: Vec<Given>,
    ) -> anyhow::Result<Engine> {
        let mut engine = Engine::build(store, adapters, tx)?;
        engine.answers = answers;
        for g in given {
            engine.adopt(g);
        }
        engine.settle();
        Ok(engine)
    }

    fn build(store: Store, adapters: Vec<Arc<dyn Adapter>>, tx: Sender<Msg>) -> anyhow::Result<Engine> {
        let open_forms = store.open_forms()?;
        let mut projects = store.projects()?;
        // The order the user gave the projects; one registered since comes after them, oldest first.
        let order: Vec<String> =
            store.meta_get(PROJECT_ORDER).ok().flatten().and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default();
        projects.sort_by_key(|p| order.iter().position(|id| *id == p.id).unwrap_or(usize::MAX));
        let mut threads = store.threads()?;
        // Threads named before titles carry one word from their first prompt: give them the title.
        for t in threads.iter_mut().filter(|t| t.head > 0 && !t.name.contains(' ') && is_agent(&t.active_agent)) {
            let first = store.events(&t.id, 1, t.head.min(50)).unwrap_or_default().into_iter().find_map(|e| match e.kind {
                EventKind::UserMessage { text, .. } => Some(text),
                _ => None,
            });
            if let Some(text) = first
                && t.name == desl_ir::derive_word(&text)
            {
                t.name = derive_name(&text);
                if let Err(e) = store.put_thread(t) {
                    tracing::error!(thread = %t.id, "could not retitle the thread: {e:#}");
                }
            }
        }
        for (thread_id, form) in open_forms {
            if let Some(t) = threads.iter_mut().find(|t| t.id == thread_id) {
                t.open_forms.push(form);
            }
        }
        let agents = detect(&adapters);
        let mut engine = Engine {
            store,
            adapters,
            agents,
            projects,
            threads,
            live: HashMap::new(),
            clients: HashMap::new(),
            waiters: Vec::new(),
            models: HashMap::new(),
            tx,
            generation: 0,
            ptys: HashMap::new(),
            answers: term::Answers::default(),
            tails: HashMap::new(),
            pending: HashMap::new(),
            entered: HashMap::new(),
            typing: HashMap::new(),
            awaiting_turn: HashMap::new(),
            restarting: HashSet::new(),
            asked_on_screen: HashSet::new(),
            socket: None,
            git_busy: false,
            config_file: None,
            file_manager: desl_ir::FILE_MANAGER.to_string(),
            tasks_program: task_list_program(),
            ticks: 0,
            stopping: false,
            handover: None,
        };
        for t in engine.threads.iter_mut() {
            t.git = git::state(Path::new(&t.cwd));
        }
        for p in engine.projects.iter_mut() {
            p.git = git::state(Path::new(&p.path));
        }
        Ok(engine)
    }

    /// Tells the truth about turns an earlier daemon left unfinished: an agent that is not
    /// running here is gone, and its turn with it.
    fn settle(&mut self) {
        // A thread waiting on a form was not working: it waits on the user still.
        let interrupted: Vec<String> = self
            .threads
            .iter()
            .filter(|t| t.status.busy() && !self.ptys.contains_key(&t.id))
            .filter(|t| !(t.status == Status::Waiting && !t.open_forms.is_empty()))
            .map(|t| t.id.clone())
            .collect();
        for id in interrupted {
            self.append(
                &id,
                EventKind::Error {
                    message: "The desl daemon restarted while this agent was working. Send your prompt again to resume.".into(),
                    recoverable: true,
                },
            );
            if let Ok(i) = self.idx(&id) {
                self.threads[i].status = Status::Idle;
            }
            self.persist(&id);
        }
    }

    // ---- small helpers -----------------------------------------------------------------------

    fn idx(&self, thread_id: &str) -> Result<usize, String> {
        self.threads
            .iter()
            .position(|t| t.id == thread_id)
            .ok_or_else(|| format!("There is no thread {thread_id}. `desl status` lists the threads."))
    }

    fn adapter(&self, agent: &str) -> Option<Arc<dyn Adapter>> {
        self.adapters.iter().find(|a| a.id() == agent).cloned()
    }

    /// The file manager's name while it is on the daemon's PATH: looked up each time, so one
    /// installed while the daemon runs is found.
    fn file_manager(&self) -> Option<String> {
        which(&self.file_manager).map(|_| self.file_manager.clone())
    }

    fn no_file_manager(&self) -> String {
        format!(
            "{0} is not installed, or not on the PATH the desl daemon started with. Install {0} (https://yazi-rs.github.io/docs/installation) and open it again.",
            self.file_manager
        )
    }

    fn agent_info(&self, agent: &str) -> Option<&AgentInfo> {
        self.agents.iter().find(|a| a.id == agent)
    }

    /// The agents desl has adapters for, as a sentence names them: `claude, codex and copilot`.
    fn agent_names(&self) -> String {
        let names: Vec<&str> = self.agents.iter().map(|a| a.id.as_str()).collect();
        match names.split_last() {
            Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
            _ => names.join(""),
        }
    }

    /// An agent desl can start a turn on, or the sentence that says why not.
    fn usable(&self, agent: &str) -> Result<(), String> {
        let Some(info) = self.agent_info(agent) else {
            return Err(format!("desl drives {}, not {agent}.", self.agent_names()));
        };
        if !info.detection.installed {
            return Err(format!("{agent} is not installed. {}", info.detection.install_hint));
        }
        if info.detection.logged_in == Some(false) {
            return Err(format!("{agent} is not logged in. {}", info.detection.login_hint));
        }
        Ok(())
    }

    fn no_agent(&self) -> String {
        let hints: Vec<String> = self
            .agents
            .iter()
            .map(|a| if a.detection.installed { a.detection.login_hint.clone() } else { a.detection.install_hint.clone() })
            .collect();
        format!("desl drives {}, and none of them is ready on this machine. {}", self.agent_names(), hints.join(" "))
    }

    /// Reads desl's settings from `file` (SPEC.md "Configuration") from now on.
    /// The socket this daemon serves, which each agent's `desl mcp` connects to.
    pub fn set_socket(&mut self, socket: PathBuf) {
        self.socket = Some(socket);
    }

    /// `desl mcp` for a thread's agent: this daemon's own binary, on this daemon's socket. None
    /// until the socket is known, as in tests.
    fn mcp_for(&self, thread_id: &str) -> Option<McpServer> {
        let socket = self.socket.as_ref()?;
        let command = self.tasks_program.first()?.clone();
        let args = ["mcp", "--thread", thread_id, "--socket", &socket.to_string_lossy()].map(String::from).to_vec();
        Some(McpServer { command, args })
    }

    pub fn set_config_file(&mut self, file: PathBuf) {
        for problem in config::load(&file, None).problems {
            tracing::warn!("{problem}");
        }
        self.config_file = Some(file);
    }

    /// The settings in effect for `project` (its own file over the user's), read afresh.
    fn settings(&self, project: Option<&Path>) -> config::Config {
        match &self.config_file {
            Some(f) => config::load(f, project),
            None => config::Config::default(),
        }
    }

    /// How sentences name desl's prefix: `ctrl+space`.
    fn prefix(&self) -> String {
        self.settings(None).prefix.label()
    }

    fn default_agent(&self, project_id: &str) -> Result<String, String> {
        let project = self.projects.iter().find(|p| p.id == project_id);
        let configured = self.settings(project.map(|p| Path::new(&p.path))).default_agent;
        let preferred = project.and_then(|p| p.default_agent.clone()).or(configured);
        preferred
            .into_iter()
            .chain(self.agents.iter().map(|a| a.id.clone()))
            .find(|a| self.usable(a).is_ok())
            .ok_or_else(|| self.no_agent())
    }

    /// The agent `/switch` means when none is named: the only other usable one.
    fn other_agent(&self, active: &str) -> Result<String, String> {
        let others: Vec<&str> = self.agents.iter().map(|a| a.id.as_str()).filter(|a| *a != active && self.usable(a).is_ok()).collect();
        match others.as_slice() {
            [one] => Ok((*one).to_string()),
            [] => Err("There is no other agent to hand this thread to. `desl agents` shows what desl found.".to_string()),
            many => Err(format!("Say which agent: {}.", many.join(" or "))),
        }
    }

    fn send(&self, client: ClientId, frame: &ServerFrame) {
        if let Some(c) = self.clients.get(&client)
            && let Ok(bytes) = desl_proto::encode(frame)
        {
            let _ = c.out.send(Arc::new(bytes));
        }
    }

    fn reply(&self, client: ClientId, id: u64, outcome: Outcome) {
        let frame = match outcome {
            Ok(reply) => ServerFrame::Reply { id, ok: Some(reply), error: None },
            Err(error) => ServerFrame::Reply { id, ok: None, error: Some(error) },
        };
        self.send(client, &frame);
    }

    /// The event bus: every subscribed client hears every change, as it happens.
    fn broadcast(&self, change: Change) {
        let Ok(bytes) = desl_proto::encode(&ServerFrame::Event { change }) else { return };
        let bytes = Arc::new(bytes);
        for c in self.clients.values().filter(|c| c.subscribed) {
            let _ = c.out.send(bytes.clone());
        }
    }

    fn stream(&self, thread_id: &str, delta: StreamDelta) {
        let Ok(bytes) = desl_proto::encode(&ServerFrame::Stream { thread_id: thread_id.to_string(), delta }) else { return };
        let bytes = Arc::new(bytes);
        for c in self.clients.values().filter(|c| c.viewing.as_deref() == Some(thread_id)) {
            let _ = c.out.send(bytes.clone());
        }
    }

    /// Writes a thread's row, tells the clients, and answers anyone waiting on it.
    fn persist(&mut self, thread_id: &str) {
        let Ok(i) = self.idx(thread_id) else { return };
        self.threads[i].updated_at = now();
        // An idle agent with a form open is waiting on the user (web-forms design, "Lifecycle").
        if self.threads[i].status == Status::Idle && !self.threads[i].open_forms.is_empty() {
            self.threads[i].status = Status::Waiting;
        }
        if self.threads[i].status != Status::Waiting {
            self.asked_on_screen.remove(thread_id);
        }
        if let Err(e) = self.store.put_thread(&self.threads[i]) {
            tracing::error!(thread = thread_id, "could not write the thread: {e:#}");
        }
        let thread = self.threads[i].clone();
        let status = thread.status;
        self.broadcast(Change::Thread { thread: Box::new(thread) });
        if !status.busy() || status == Status::Waiting {
            let (ready, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.waiters).into_iter().partition(|(_, _, t)| t == thread_id);
            self.waiters = rest;
            for (client, id, _) in ready {
                self.reply(client, id, Ok(Reply::Waited { status }));
            }
        }
    }

    /// The one write path for events: store, then head, then clients.
    fn append(&mut self, thread_id: &str, kind: EventKind) -> Option<Event> {
        let i = self.idx(thread_id).ok()?;
        let kind = self.store.shrink(kind);
        let t = &self.threads[i];
        let event = Event { index: t.head + 1, thread_id: thread_id.to_string(), timestamp: now(), agent: t.active_agent.clone(), kind };
        if let Err(e) = self.store.append(&event) {
            tracing::error!(thread = thread_id, index = event.index, "could not write an event: {e:#}");
        }
        self.threads[i].head = event.index;
        self.broadcast(Change::Appended { event: event.clone() });
        Some(event)
    }

    fn sink(&self, thread_id: &str, agent: &str, generation: u64) -> Sink {
        let tx = self.tx.clone();
        let (thread_id, agent) = (thread_id.to_string(), agent.to_string());
        Arc::new(move |event| {
            let _ = tx.send(Msg::Adapter { thread_id: thread_id.clone(), agent: agent.clone(), generation, event });
        })
    }

    fn live(&mut self, thread_id: &str) -> &mut Live {
        self.live.entry(thread_id.to_string()).or_default()
    }

    /// The thread's "always" list, rebuilt from its log the first time it is needed.
    fn always(&mut self, thread_id: &str) -> &mut HashSet<String> {
        if self.live(thread_id).always.is_none() {
            let mut tools: HashMap<String, String> = HashMap::new();
            let mut set = HashSet::new();
            for e in self.store.all_events(thread_id).unwrap_or_default() {
                match e.kind {
                    EventKind::ApprovalRequest { call_id, tool, .. } => {
                        tools.insert(call_id, tool);
                    }
                    EventKind::ApprovalResponse { call_id, decision: Decision::Always, by } if by == "user" => {
                        if let Some(tool) = tools.get(&call_id) {
                            set.insert(tool.clone());
                        }
                    }
                    _ => {}
                }
            }
            self.live(thread_id).always = Some(set);
        }
        self.live(thread_id).always.get_or_insert_default()
    }

    fn mark_seen(&mut self, thread_id: &str, agent: &str) {
        let Ok(i) = self.idx(thread_id) else { return };
        let head = self.threads[i].head;
        let b =
            self.threads[i].bindings.entry(agent.to_string()).or_insert_with(|| Binding { agent: agent.to_string(), ..Binding::default() });
        b.seen_up_to = head;
        b.last_used_at = now();
    }

    // ---- projects and threads ----------------------------------------------------------------

    /// The project a directory belongs to, registered on first sight.
    fn project_for(&mut self, cwd: &Path) -> Result<String, String> {
        let root = git::toplevel(cwd);
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        if !root.is_dir() {
            return Err(format!("{} is not a directory desl can open. Run desl inside a repository.", root.display()));
        }
        let path = root.to_string_lossy().to_string();
        if let Some(p) = self.projects.iter().find(|p| p.path == path) {
            return Ok(p.id.clone());
        }
        let base = root.file_name().map(|s| s.to_string_lossy().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "project".into());
        let mut id = base.clone();
        let mut n = 2;
        while self.projects.iter().any(|p| p.id == id) {
            id = format!("{base}-{n}");
            n += 1;
        }
        let git = git::state(Path::new(&path));
        let project = Project { id: id.clone(), path, default_agent: None, created_at: now(), git };
        self.store.put_project(&project).map_err(|e| format!("Could not save the project ({e:#}). `desl doctor` checks the store."))?;
        self.projects.push(project.clone());
        self.broadcast(Change::Project { project });
        Ok(id)
    }

    /// Short, typeable, git-style: `t-7f3a`.
    fn new_thread_id(&self) -> String {
        use std::hash::{Hash, Hasher};
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        for salt in 0u32.. {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (nanos, salt, std::process::id()).hash(&mut h);
            let id = format!("t-{:04x}", h.finish() & 0xffff);
            if !self.threads.iter().any(|t| t.id == id || t.id.starts_with(&format!("{id}/"))) {
                return id;
            }
        }
        unreachable!("a free thread id always exists")
    }

    fn create_thread(&mut self, project_id: &str, agent: &str) -> Result<String, String> {
        let project =
            self.projects.iter().find(|p| p.id == project_id).cloned().ok_or_else(|| format!("There is no project {project_id}."))?;
        let ts = now();
        let thread = Thread {
            id: self.new_thread_id(),
            project_id: project.id,
            name: String::new(),
            named: false,
            git: git::state(Path::new(&project.path)),
            cwd: project.path.clone(),
            worktree: None,
            parent_id: None,
            fork_point: None,
            active_agent: agent.to_string(),
            status: Status::Idle,
            permission_mode: self
                .settings(Some(Path::new(&project.path)))
                .default_mode
                .and_then(|m| PermissionMode::parse(&m))
                .filter(|m| *m != PermissionMode::Worktree)
                .unwrap_or_default(),
            created_at: ts,
            updated_at: ts,
            head: 0,
            bindings: Default::default(),
            queued: Vec::new(),
            pending_approval: None,
            subagents: Vec::new(),
            open_forms: Vec::new(),
        };
        let id = thread.id.clone();
        self.threads.push(thread);
        self.persist(&id);
        Ok(id)
    }

    // ---- turns -------------------------------------------------------------------------------

    /// Sends a prompt to the thread's active agent. `retry` is the index of the prompt's event
    /// when this is the second try after a failed resume; the prompt is not appended again.
    fn dispatch(&mut self, thread_id: &str, prompt: &str, retry: Option<u64>) {
        let Ok(i) = self.idx(thread_id) else { return };
        let agent = self.threads[i].active_agent.clone();
        if self.threads[i].name.is_empty() {
            self.threads[i].name = derive_name(prompt);
        }
        let user_index = match retry {
            Some(index) => index,
            None => {
                self.append(thread_id, EventKind::UserMessage { text: prompt.to_string(), attachments: Vec::new() });
                self.threads[i].head
            }
        };

        // How far behind is this agent? Everything between its mark and the prompt is news to it.
        let upto = user_index - 1;
        let (seen, session_id) = match self.threads[i].bindings.get(&agent) {
            Some(b) => (b.seen_up_to.min(upto), b.session_id.clone()),
            None => (0, None),
        };
        let resumed = session_id.is_some();
        let unseen = if upto > seen { self.store.events(thread_id, seen + 1, upto).unwrap_or_default() } else { Vec::new() };
        let payload = if unseen.iter().any(|e| e.kind.conversational()) {
            let cwd = self.threads[i].cwd.clone();
            let diff = if resumed { String::new() } else { git::diff_head(Path::new(&cwd)) };
            let brief = projector::build(&unseen, Some(prompt), resumed, &diff, projector::BUDGET_TOKENS);
            self.append(
                thread_id,
                EventKind::Brief {
                    text: brief.text.clone(),
                    token_estimate: brief.token_estimate,
                    covers_range: brief.covers,
                    delta: resumed,
                },
            );
            brief.text
        } else {
            prompt.to_string()
        };

        let live = self.live(thread_id);
        if live.stale.remove(&agent) {
            live.sessions.remove(&agent);
        }
        if !self.live(thread_id).sessions.contains_key(&agent) {
            let Some(adapter) = self.adapter(&agent) else {
                return self.fail(thread_id, format!("desl has no adapter for {agent}. Switch this thread to {}.", self.agent_names()));
            };
            self.generation += 1;
            let generation = self.generation;
            let t = &self.threads[i];
            let opts = StartOpts {
                thread_id: t.id.clone(),
                thread_name: t.name.clone(),
                cwd: t.cwd.clone().into(),
                resume_session_id: session_id,
                model: t.bindings.get(&agent).and_then(|b| b.model_choice.clone()),
                effort: t.bindings.get(&agent).and_then(|b| b.effort_choice.clone()),
                permission_mode: t.permission_mode,
            };
            match adapter.start(opts, self.sink(thread_id, &agent, generation)) {
                Ok(session) => {
                    self.live(thread_id).sessions.insert(agent.clone(), (generation, session));
                }
                Err(e) => return self.fail(thread_id, e.0),
            }
        }
        let sent = match self.live(thread_id).sessions.get_mut(&agent) {
            Some((_, session)) => session.send(Turn::text(payload)),
            None => return,
        };
        if let Err(e) = sent {
            self.live(thread_id).sessions.remove(&agent);
            return self.fail(thread_id, e.0);
        }
        tracing::info!(thread = thread_id, agent = %agent, resumed, "turn started");
        let live = self.live(thread_id);
        live.turn = Some(TurnState {
            agent: agent.clone(),
            resumed,
            heard: false,
            retried: retry.is_some(),
            prompt: prompt.to_string(),
            user_index,
        });
        live.partial = None;
        self.threads[i].status = Status::Working;
        self.threads[i].pending_approval = None;
        self.mark_seen(thread_id, &agent);
        self.persist(thread_id);
    }

    /// desl itself could not do something: say so in the transcript and mark the thread failed.
    fn fail(&mut self, thread_id: &str, message: String) {
        tracing::warn!(thread = thread_id, "{message}");
        self.append(thread_id, EventKind::Error { message, recoverable: true });
        if let Ok(i) = self.idx(thread_id) {
            self.threads[i].status = Status::Failed;
            self.threads[i].pending_approval = None;
        }
        let live = self.live(thread_id);
        live.turn = None;
        live.partial = None;
        self.persist(thread_id);
    }

    /// The next queued prompt, if any, goes out now.
    fn dequeue(&mut self, thread_id: &str) {
        let Ok(i) = self.idx(thread_id) else { return };
        if self.threads[i].queued.is_empty() {
            return;
        }
        match self.store.queue_pop(thread_id) {
            Ok(Some(text)) => {
                self.threads[i].queued.remove(0);
                self.dispatch(thread_id, &text, None);
            }
            Ok(None) => self.threads[i].queued.clear(),
            Err(e) => tracing::error!(thread = thread_id, "could not read the queue: {e:#}"),
        }
    }

    fn on_adapter(&mut self, thread_id: &str, agent: &str, generation: u64, event: AdapterEvent) {
        let Ok(i) = self.idx(thread_id) else { return };
        // Only the current session of this agent speaks for the thread.
        match self.live(thread_id).sessions.get(agent) {
            Some((g, _)) if *g == generation => {}
            _ => return,
        }
        let active = self.threads[i].active_agent == agent;
        match event {
            AdapterEvent::Session { session_id, model } => {
                let b = self.threads[i]
                    .bindings
                    .entry(agent.to_string())
                    .or_insert_with(|| Binding { agent: agent.to_string(), ..Binding::default() });
                b.session_id = Some(session_id);
                if model.is_some() {
                    b.model = model;
                }
                b.last_used_at = now();
                self.persist(thread_id);
            }
            AdapterEvent::Exit { code, detail } => self.session_exited(thread_id, agent, code, &detail),
            // A process still talking after its thread moved to another agent is not the conversation.
            _ if !active => {}
            AdapterEvent::TextStart => {
                let live = self.live(thread_id);
                live.partial = Some(String::new());
                if let Some(t) = live.turn.as_mut() {
                    t.heard = true;
                }
                self.stream(thread_id, StreamDelta::Start);
            }
            AdapterEvent::TextDelta { text } => {
                self.live(thread_id).partial.get_or_insert_default().push_str(&text);
                self.stream(thread_id, StreamDelta::Text { text });
            }
            AdapterEvent::Event { event } => self.on_agent_event(thread_id, agent, event),
            AdapterEvent::TurnEnd { error } => self.turn_ended(thread_id, agent, error),
        }
    }

    fn on_agent_event(&mut self, thread_id: &str, agent: &str, kind: EventKind) {
        let Ok(i) = self.idx(thread_id) else { return };
        if let Some(t) = self.live(thread_id).turn.as_mut() {
            t.heard = true;
        }
        match &kind {
            EventKind::AssistantMessage { .. } => self.live(thread_id).partial = None,
            // The approval broker: policy answers what it can, the human answers the rest.
            EventKind::ApprovalRequest { call_id, tool, summary, .. } => {
                let (call_id, tool, summary) = (call_id.clone(), tool.clone(), summary.clone());
                self.append(thread_id, kind);
                // In `yolo` everything is allowed; otherwise what the user answered "always" for.
                let policy = match self.threads[i].permission_mode {
                    PermissionMode::Yolo => Some(Decision::Allow),
                    _ => self.always(thread_id).contains(&tool).then_some(Decision::Always),
                };
                if let Some(decision) = policy {
                    if let Some((_, session)) = self.live(thread_id).sessions.get_mut(agent)
                        && let Err(e) = session.answer(&call_id, Decision::Allow)
                    {
                        tracing::warn!(thread = thread_id, "could not answer an approval: {e}");
                    }
                    self.append(thread_id, EventKind::ApprovalResponse { call_id, decision, by: "policy".into() });
                } else {
                    self.threads[i].pending_approval = Some(PendingApproval { call_id, tool, summary });
                    self.threads[i].status = Status::Waiting;
                    self.persist(thread_id);
                }
                self.mark_seen(thread_id, agent);
                return;
            }
            // A result for the call an approval was about: the question is moot.
            EventKind::ToolResult { call_id, .. } if self.threads[i].pending_approval.as_ref().is_some_and(|p| p.call_id == *call_id) => {
                self.threads[i].pending_approval = None;
                self.threads[i].status = Status::Working;
                self.append(thread_id, kind);
                self.mark_seen(thread_id, agent);
                self.persist(thread_id);
                return;
            }
            _ => {}
        }
        self.append(thread_id, kind);
        self.mark_seen(thread_id, agent);
    }

    fn turn_ended(&mut self, thread_id: &str, agent: &str, error: Option<String>) {
        let Ok(i) = self.idx(thread_id) else { return };
        let turn = self.live(thread_id).turn.take();
        if let Some(message) = error {
            if let Some(t) = turn.as_ref().filter(|t| t.resumed && !t.heard && !t.retried) {
                let (prompt, user_index) = (t.prompt.clone(), t.user_index);
                return self.resume_failed(thread_id, agent, &prompt, user_index, &message);
            }
            self.append(thread_id, EventKind::Error { message, recoverable: true });
        }
        tracing::info!(thread = thread_id, agent, "turn ended");
        self.live(thread_id).partial = None;
        self.threads[i].pending_approval = None;
        self.threads[i].status = Status::Idle;
        self.mark_seen(thread_id, agent);
        self.refresh_git();
        self.persist(thread_id);
        self.dequeue(thread_id);
    }

    fn session_exited(&mut self, thread_id: &str, agent: &str, code: Option<i32>, detail: &str) {
        let Ok(i) = self.idx(thread_id) else { return };
        self.live(thread_id).sessions.remove(agent);
        // An idle agent's process ending is not news; the next prompt resumes its session.
        let Some(turn) = self.live(thread_id).turn.take_if(|t| t.agent == agent) else { return };
        if turn.resumed && !turn.heard && !turn.retried {
            return self.resume_failed(thread_id, agent, &turn.prompt, turn.user_index, detail);
        }
        let code = code.map(|c| c.to_string()).unwrap_or_else(|| "a signal".into());
        let mut message = format!("{agent} exited with {code} mid-turn.");
        if !detail.is_empty() {
            message.push_str(&format!(" It said: {detail}."));
        }
        message.push_str(" Send your prompt again to resume the session.");
        tracing::warn!(thread = thread_id, "{message}");
        self.append(thread_id, EventKind::Error { message, recoverable: true });
        self.live(thread_id).partial = None;
        self.threads[i].pending_approval = None;
        self.threads[i].status = Status::Failed;
        self.persist(thread_id);
    }

    /// SPEC.md switch algorithm, step 6: the native session is gone (purged, agent updated,
    /// directory moved). Drop the binding and start over with a full brief.
    fn resume_failed(&mut self, thread_id: &str, agent: &str, prompt: &str, user_index: u64, why: &str) {
        let Ok(i) = self.idx(thread_id) else { return };
        self.live(thread_id).sessions.remove(agent);
        self.threads[i].bindings.remove(agent);
        let mut message = format!("Could not resume the {agent} session, so desl started a new one with a full brief.");
        if !why.trim().is_empty() {
            message.push_str(&format!(" {agent} said: {}.", why.trim().trim_end_matches('.')));
        }
        tracing::warn!(thread = thread_id, "{message}");
        self.append(thread_id, EventKind::Error { message, recoverable: true });
        self.dispatch(thread_id, prompt, Some(user_index));
    }

    // ---- commands ----------------------------------------------------------------------------

    pub fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Connected { client, out } => {
                self.clients.insert(client, Client { out, subscribed: false, viewing: None, shows: HashSet::new() });
                self.refresh_git();
            }
            Msg::Disconnected { client } => {
                self.clients.remove(&client);
                self.waiters.retain(|(c, _, _)| *c != client);
            }
            Msg::Request { client, request } => {
                let id = request.id;
                if let Some(outcome) = self.command(client, id, request.command) {
                    self.reply(client, id, outcome);
                }
            }
            Msg::Adapter { thread_id, agent, generation, event } => self.on_adapter(&thread_id, &agent, generation, event),
            Msg::Models { client, id, thread_id, agent, result } => {
                let outcome = result.and_then(|models| {
                    self.models.insert(agent.clone(), models);
                    self.models_reply(&thread_id, &agent)
                });
                self.reply(client, id, outcome);
            }
            Msg::PtyOutput { thread_id, generation, data } => self.pty_output(&thread_id, generation, &data),
            Msg::PtyExit { thread_id, generation } => self.pty_exit(&thread_id, generation),
            Msg::Git { states } => self.git_states(states),
            Msg::Tick => self.tick(),
        }
    }

    // ---- agents' own interfaces (the native route) --------------------------------------------

    /// Starts the thread's active agent in a PTY: resuming its session when it has one.
    fn start_pty(&mut self, i: usize, rows: u16, cols: u16) -> Result<(), String> {
        let t = &self.threads[i];
        let (thread_id, agent) = (t.id.clone(), t.active_agent.clone());
        let adapter = if is_agent(&agent) {
            self.usable(&agent)?;
            Some(self.adapter(&agent).ok_or_else(|| format!("desl has no adapter for {agent}."))?)
        } else {
            None
        };
        let binding = t.bindings.get(&agent);
        let launch = match &adapter {
            None if agent == FILES => files_launch(&self.file_manager().ok_or_else(|| self.no_file_manager())?),
            None if agent == TASKS => tasks_launch(&self.tasks_program, &t.cwd),
            None => shell_launch(),
            Some(adapter) => {
                let open = match binding.and_then(|b| b.session_id.clone()) {
                    // A session that never had a turn has no file, and its agent cannot resume it.
                    Some(id) if adapter.session_file(Path::new(&t.cwd), Some(&id), UNIX_EPOCH).is_none() => {
                        Open::New { session_id: Some(id) }
                    }
                    Some(session_id) => Open::Resume { session_id },
                    // A thread with history and no session of this agent yet, such as a fork:
                    // its conversation is copied into a new session, as a switch does.
                    None if t.head > 0 => {
                        let kinds: Vec<EventKind> = self.store.all_events(&t.id).unwrap_or_default().into_iter().map(|e| e.kind).collect();
                        match adapter.write_session(Path::new(&t.cwd), &kinds) {
                            Ok((session_id, _, _)) => Open::Resume { session_id },
                            Err(e) => {
                                tracing::warn!(thread = %t.id, agent = %agent, "could not copy the conversation into a new session: {e}");
                                Open::New { session_id: Some(desl_ir::adapter::new_uuid()) }
                            }
                        }
                    }
                    None => Open::New { session_id: Some(desl_ir::adapter::new_uuid()) },
                };
                adapter.launch(&LaunchOpts {
                    cwd: t.cwd.clone().into(),
                    open,
                    model: binding.and_then(|b| b.model_choice.clone()),
                    effort: binding.and_then(|b| b.effort_choice.clone()),
                    permission_mode: t.permission_mode,
                    mcp: self.mcp_for(&thread_id),
                })
            }
        };
        self.generation += 1;
        let generation = self.generation;
        let (out_tx, exit_tx) = (self.tx.clone(), self.tx.clone());
        let (out_id, exit_id) = (thread_id.clone(), thread_id.clone());
        let pty = Pty::spawn(
            &agent,
            &thread_id,
            &launch,
            Path::new(&t.cwd),
            rows,
            cols,
            move |data| {
                let _ = out_tx.send(Msg::PtyOutput { thread_id: out_id.clone(), generation, data });
            },
            move || {
                let _ = exit_tx.send(Msg::PtyExit { thread_id: exit_id.clone(), generation });
            },
        )?;
        tracing::info!(thread = %thread_id, agent = %agent, args = ?launch.args, "started the agent's interface");
        self.ptys.insert(thread_id.clone(), (generation, pty));
        self.follow(i, SystemTime::now(), Vec::new());
        if let Some(session_id) = launch.session_id {
            let ts = now();
            let b = self.threads[i].bindings.entry(agent.clone()).or_insert_with(|| Binding {
                agent: agent.clone(),
                session_id: None,
                seen_up_to: 0,
                model: None,
                model_choice: None,
                effort_choice: None,
                last_used_at: ts,
                file_offset: 0,
            });
            b.session_id = Some(session_id);
            b.last_used_at = ts;
            self.persist(&thread_id);
        }
        Ok(())
    }

    /// Follows the session file of the agent running in thread `i`, which started at `since`.
    /// `partial` is the start of a line already read from the file and not finished.
    fn follow(&mut self, i: usize, since: SystemTime, partial: Vec<u8>) {
        let t = &self.threads[i];
        let agent = t.active_agent.clone();
        let Some(reader) = self.adapter(&agent).and_then(|a| a.session_reader(Path::new(&t.cwd))) else { return };
        let read_before = t.bindings.get(&agent).is_some_and(|b| b.file_offset > 0);
        self.tails.insert(
            t.id.clone(),
            Tail {
                agent,
                reader,
                path: None,
                since,
                skip_existing: !read_before && t.head > 0,
                partial,
                last_poll: Instant::now(),
                title_asked: None,
            },
        );
    }

    // ---- handing every terminal to the next daemon (SPEC.md "Upgrades") ------------------------

    /// The binary to start again from, once nothing is half done.
    pub fn handover_due(&self) -> Option<PathBuf> {
        let h = self.handover.as_ref()?;
        let settled = self.typing.is_empty() && self.pending.is_empty() && self.restarting.is_empty();
        (settled || h.asked.elapsed() >= HANDOVER_WAIT).then(|| h.exe.clone())
    }

    /// Stops reading every terminal. What the readers had read is on its way here as messages;
    /// what the programs draw from now on waits in their terminals.
    pub fn hold_terminals(&mut self) {
        for (_, pty) in self.ptys.values_mut() {
            pty.hold();
        }
        for (_, pty) in self.ptys.values_mut() {
            pty.held();
        }
    }

    /// True while some terminal is being read: one started since the readers were stopped.
    pub fn reading_terminals(&self) -> bool {
        self.ptys.values().any(|(_, pty)| pty.reading())
    }

    /// Reads every terminal: again after a handover that did not happen, or for the first time
    /// in the daemon that took them over.
    pub fn read_terminals(&mut self) {
        for (_, pty) in self.ptys.values_mut() {
            pty.read();
        }
    }

    /// Every running program as the next daemon is to receive it.
    pub fn giving(&mut self) -> (term::Answers, Vec<Giving<'_>>) {
        let ids: Vec<String> = self.ptys.keys().cloned().collect();
        for id in &ids {
            // What the agent wrote to its session file up to now is in the store.
            self.poll_tail(id);
        }
        let mut all: Vec<Giving<'_>> = Vec::new();
        for (id, (_, pty)) in &self.ptys {
            let since_ms = pty.since().duration_since(UNIX_EPOCH).map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX));
            all.push(Giving {
                kept: Kept::new(id, &pty.agent, pty.pid(), pty.size(), since_ms),
                master: pty.master(),
                log: pty.log(),
                partial: self.tails.get(id).map_or(&[][..], |t| &t.partial),
            });
        }
        (self.answers.clone(), all)
    }

    /// Keys the user pressed while the terminals changed hands still reach their agents: this
    /// daemon can write to a terminal until it is gone.
    pub fn last_keys(&mut self, msg: Msg) {
        if let Msg::Request { client, request: Request { id, command: command @ Command::Input { .. } } } = msg {
            self.command(client, id, command);
        }
    }

    /// The next daemon serves, as process `pid`: this one lets go of every program and says so.
    pub fn handed_over(&mut self, pid: u32) {
        let kept = self.ptys.len();
        for (_, (_, pty)) in self.ptys.drain() {
            pty.hand_over();
        }
        self.tails.clear();
        // A switch that was still starting ends here; its thread stays on the agent it was on.
        self.pending.clear();
        if let Some(h) = self.handover.take() {
            self.reply(h.client, h.id, Ok(Reply::HandedOver { pid, kept }));
        }
        self.broadcast(Change::Restarting);
    }

    /// The handover did not happen: this daemon goes on as it was, and says why.
    pub fn handover_failed(&mut self, why: &str) {
        self.read_terminals();
        if let Some(h) = self.handover.take() {
            self.reply(h.client, h.id, Err(why.to_string()));
        }
    }

    /// A daemon that could not start serving lets go of what it was given: the daemon that
    /// leaves still has every terminal, and stays.
    pub fn let_go(&mut self) {
        for (_, (_, pty)) in self.ptys.drain() {
            pty.hand_over();
        }
        self.tails.clear();
    }

    pub fn terminals(&self) -> usize {
        self.ptys.len()
    }

    /// Takes one program the daemon before this one handed over.
    fn adopt(&mut self, g: Given) {
        let Given { kept, master, log, partial } = g;
        let Ok(i) = self.idx(&kept.thread_id) else {
            tracing::warn!(thread = %kept.thread_id, "a terminal came for a thread that is not in the store; its program ends with the daemon that leaves");
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        let (out_tx, exit_tx) = (self.tx.clone(), self.tx.clone());
        let (out_id, exit_id) = (kept.thread_id.clone(), kept.thread_id.clone());
        let since = UNIX_EPOCH + Duration::from_millis(kept.since_ms);
        let pty = Pty::adopt(
            &kept.agent,
            master,
            kept.pid,
            kept.rows,
            kept.cols,
            since,
            log,
            move |data| {
                let _ = out_tx.send(Msg::PtyOutput { thread_id: out_id.clone(), generation, data });
            },
            move || {
                let _ = exit_tx.send(Msg::PtyExit { thread_id: exit_id.clone(), generation });
            },
        );
        match pty {
            Ok(pty) => {
                self.ptys.insert(kept.thread_id.clone(), (generation, pty));
                // Only the agent the thread is on now has a session file worth following.
                if self.threads[i].active_agent == kept.agent {
                    self.follow(i, since, partial);
                }
                tracing::info!(thread = %kept.thread_id, agent = %kept.agent, pid = ?kept.pid, "took over the agent's interface");
            }
            Err(e) => tracing::error!(thread = %kept.thread_id, "{e}"),
        }
    }

    fn attach(&mut self, client: ClientId, thread_id: &str, rows: u16, cols: u16, answers: term::Answers) -> Outcome {
        let i = self.idx(thread_id)?;
        if answers != term::Answers::default() {
            self.answers = answers;
        }
        match self.ptys.get_mut(thread_id) {
            Some((_, pty)) => pty.resize(rows, cols),
            None => self.start_pty(i, rows, cols)?,
        }
        if let Some(c) = self.clients.get_mut(&client) {
            c.shows.insert(thread_id.to_string());
        }
        let (_, pty) = &self.ptys[thread_id];
        Ok(Reply::Attached { thread_id: thread_id.to_string(), agent: pty.agent.clone(), replay: b64::encode(pty.log()) })
    }

    fn pty_output(&mut self, thread_id: &str, generation: u64, data: &[u8]) {
        if let Some(p) = self.pending.get_mut(thread_id).filter(|p| p.generation == generation) {
            // Out of sight: kept for the swap, and its questions to the terminal answered here.
            p.pty.took(data);
            p.last_output = Instant::now();
            p.saw_output = true;
            let replies = term::replies(data, &self.answers, p.pty.cursor());
            if !replies.is_empty() {
                p.pty.write(&replies);
            }
            return;
        }
        let Some((g, pty)) = self.ptys.get_mut(thread_id) else { return };
        if *g != generation {
            return;
        }
        pty.took(data);
        if self.tails.get(thread_id).is_some_and(|t| t.last_poll.elapsed() >= Duration::from_millis(250)) {
            self.poll_tail(thread_id);
        }
        let Some((_, pty)) = self.ptys.get_mut(thread_id) else { return };
        let shown: Vec<ClientId> = self.clients.iter().filter(|(_, c)| c.shows.contains(thread_id)).map(|(id, _)| *id).collect();
        if shown.is_empty() {
            // Nobody's terminal is there to answer what the agent asks it: answer as it would.
            let replies = term::replies(data, &self.answers, pty.cursor());
            if !replies.is_empty() {
                pty.write(&replies);
            }
            return;
        }
        let frame = ServerFrame::Pty { thread_id: thread_id.to_string(), data: b64::encode(data) };
        for client in shown {
            self.send(client, &frame);
        }
    }

    /// Reads what the agent added to its session file, appends it to the thread's log, and sets
    /// the thread's status from it and from the agent's screen.
    fn poll_tail(&mut self, thread_id: &str) {
        let Ok(i) = self.idx(thread_id) else { return };
        let Some(tail) = self.tails.get_mut(thread_id) else { return };
        tail.last_poll = Instant::now();
        let agent = tail.agent.clone();
        let cwd = self.threads[i].cwd.clone();
        let mut changed = false;
        if tail.path.is_none() {
            let Some(adapter) = self.adapters.iter().find(|a| a.id() == agent).cloned() else { return };
            let known = self.threads[i].bindings.get(&agent).and_then(|b| b.session_id.clone());
            let Some((session_id, path)) = adapter.session_file(Path::new(&cwd), known.as_deref(), tail.since) else {
                return self.check_asking(thread_id);
            };
            tail.path = Some(path);
            let ts = now();
            let b = self.threads[i].bindings.entry(agent.clone()).or_insert_with(|| Binding { agent: agent.clone(), ..Binding::default() });
            if b.session_id.as_deref() != Some(&session_id) {
                tracing::info!(thread = %thread_id, agent = %agent, "found the agent's own session");
                b.session_id = Some(session_id);
                b.last_used_at = ts;
                changed = true;
            }
        }
        let Some(path) = tail.path.clone() else { return };
        let b = self.threads[i].bindings.get_mut(&agent);
        let Some(b) = b else { return };
        let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if tail.skip_existing {
            tail.skip_existing = false;
            b.file_offset = len;
            changed = true;
        }
        if len < b.file_offset {
            // The agent rewrote its file: read it again from the start.
            b.file_offset = 0;
            tail.partial.clear();
        }
        let mut readings = Vec::new();
        if len > b.file_offset {
            let mut bytes = Vec::new();
            if let Ok(mut f) = std::fs::File::open(&path) {
                use std::io::{Read, Seek, SeekFrom};
                if f.seek(SeekFrom::Start(b.file_offset)).is_ok() {
                    let _ = f.take(len - b.file_offset).read_to_end(&mut bytes);
                }
            }
            b.file_offset += bytes.len() as u64;
            tail.partial.extend_from_slice(&bytes);
            if let Some(end) = tail.partial.iter().rposition(|c| *c == b'\n') {
                let complete: Vec<u8> = tail.partial.drain(..=end).collect();
                for line in String::from_utf8_lossy(&complete).lines() {
                    readings.extend(tail.reader.line(line));
                }
            }
            changed = true;
        }
        for r in readings {
            match r {
                Reading::Event(kind) => {
                    if let EventKind::UserMessage { text, .. } = &kind
                        && self.threads[i].name.is_empty()
                    {
                        self.threads[i].name = derive_name(text);
                    }
                    self.append(thread_id, kind);
                }
                Reading::Working => {
                    self.threads[i].status = Status::Working;
                    self.awaiting_turn.remove(thread_id);
                }
                Reading::Idle => self.threads[i].status = Status::Idle,
                Reading::Title(title) => {
                    self.retitle(i, &title);
                }
                // Chosen in the agent's own interface: the agent's next session starts the same.
                Reading::Settings { model, effort } => {
                    if let Some(b) = self.threads[i].bindings.get_mut(&agent) {
                        if let Some(m) = model {
                            b.model = Some(m.clone());
                            b.model_choice = Some(m);
                        }
                        if effort.is_some() {
                            b.effort_choice = effort;
                        }
                    }
                }
            }
        }
        if let Some(tail) = self.tails.get_mut(thread_id)
            && tail.title_asked.is_none_or(|at| at.elapsed() >= TITLE_EVERY)
            && let Some(session_id) = self.threads[i].bindings.get(&agent).and_then(|b| b.session_id.clone())
            && let Some(adapter) = self.adapters.iter().find(|a| a.id() == agent).cloned()
        {
            tail.title_asked = Some(Instant::now());
            if let Some(title) = adapter.session_title(&session_id) {
                changed |= self.retitle(i, &title);
            }
        }
        if changed {
            let head = self.threads[i].head;
            if let Some(b) = self.threads[i].bindings.get_mut(&agent) {
                // The agent wrote all of it, so it has seen all of it.
                b.seen_up_to = head;
            }
            self.persist(thread_id);
        }
        self.refresh_subagents(thread_id);
        self.check_asking(thread_id);
        self.drain(thread_id);
    }

    /// The next queued prompt goes into an agent desl runs in its own interface once it is free:
    /// its turn ended, or it waits on a form. Also for an agent that has no session file yet,
    /// such as one whose only events are forms.
    fn drain(&mut self, thread_id: &str) {
        let Ok(i) = self.idx(thread_id) else { return };
        if self.threads[i].queued.is_empty()
            || !self.agent_free(i)
            || self.typing.contains_key(thread_id)
            || !self.ptys.contains_key(thread_id)
        {
            return;
        }
        if let Ok(Some(text)) = self.store.queue_pop(thread_id) {
            self.threads[i].queued.remove(0);
            self.type_prompt(thread_id, text);
        }
    }

    /// Gives thread `i` the agent's own title for its conversation, unless the user named it.
    /// The caller persists the thread; true when the name changed.
    fn retitle(&mut self, i: usize, title: &str) -> bool {
        let t = &mut self.threads[i];
        let name = derive_name(title);
        if t.named || !is_agent(&t.active_agent) || title.trim().is_empty() || t.name == name {
            return false;
        }
        tracing::info!(thread = %t.id, from = %t.name, to = %name, "the agent titled the thread");
        t.name = name;
        true
    }

    /// Puts `text` on its way into the thread's agent's own interface, as if typed there.
    fn type_prompt(&mut self, thread_id: &str, text: String) {
        let Ok(i) = self.idx(thread_id) else { return };
        self.typing.insert(thread_id.to_string(), Typing { text, typed: None });
        self.threads[i].status = Status::Working;
        self.persist(thread_id);
        self.deliver(thread_id);
    }

    /// Types a waiting prompt once the agent is ready for it and not asking something; enter
    /// follows a tick later.
    fn deliver(&mut self, thread_id: &str) {
        let Some(ty) = self.typing.get_mut(thread_id) else { return };
        let Some((_, pty)) = self.ptys.get_mut(thread_id) else {
            self.typing.remove(thread_id);
            return;
        };
        match ty.typed {
            Some(at) if at.elapsed() >= Duration::from_millis(150) => {
                pty.write(b"\r");
                self.typing.remove(thread_id);
                self.entered.insert(thread_id.to_string(), Instant::now());
                self.awaiting_turn.insert(thread_id.to_string(), Instant::now());
            }
            Some(_) => {}
            None if pty.ready() => {
                let asking = self.adapters.iter().find(|a| a.id() == pty.agent).is_some_and(|a| a.waiting(&pty.tail_text(ASKING_ROWS)));
                if asking {
                    // Its own question comes first: the thread waits for the user.
                    if let Ok(i) = self.idx(thread_id)
                        && self.threads[i].status != Status::Waiting
                    {
                        self.threads[i].status = Status::Waiting;
                        self.asked_on_screen.insert(thread_id.to_string());
                        self.persist(thread_id);
                    }
                    return;
                }
                let text = ty.text.clone();
                // Lines after the first would each send the prompt: they go as one paste.
                let bytes = if text.contains('\n') && pty.bracketed_paste() {
                    format!("\x1b[200~{text}\x1b[201~").into_bytes()
                } else {
                    text.replace('\n', " ").into_bytes()
                };
                pty.write(&bytes);
                ty.typed = Some(Instant::now());
            }
            None => {}
        }
    }

    /// The helpers the thread's agent started in its current session, from the agent's files,
    /// added to those its agents started before: a switch keeps the thread's subagents.
    fn refresh_subagents(&mut self, thread_id: &str) {
        let Ok(i) = self.idx(thread_id) else { return };
        let t = &self.threads[i];
        let agent = t.active_agent.clone();
        let Some(session) = t.bindings.get(&agent).and_then(|b| b.session_id.clone()) else { return };
        let Some(adapter) = self.adapter(&agent) else { return };
        let found = adapter.subagents(Path::new(&t.cwd), &session);
        let mut all = self.threads[i].subagents.clone();
        for mut s in found {
            s.agent = agent.clone();
            s.session_id = session.clone();
            match all.iter_mut().find(|o| o.id == s.id) {
                Some(o) => *o = s,
                None => all.push(s),
            }
        }
        if all != self.threads[i].subagents {
            self.threads[i].subagents = all;
            self.persist(thread_id);
        }
    }

    /// `waiting` while the agent's screen shows it asking something in the middle of a turn.
    fn check_asking(&mut self, thread_id: &str) {
        let Ok(i) = self.idx(thread_id) else { return };
        let Some((_, pty)) = self.ptys.get(thread_id) else { return };
        let Some(adapter) = self.adapter(&pty.agent) else { return };
        let asking = adapter.waiting(&pty.tail_text(ASKING_ROWS));
        let status = self.threads[i].status;
        // Only a wait on the screen ends with the screen: a form waits on the user, not on it.
        let on_screen = self.asked_on_screen.contains(thread_id) || self.threads[i].open_forms.is_empty();
        let next = match status {
            Status::Working if asking => Status::Waiting,
            Status::Waiting if !asking && on_screen => Status::Working,
            other => other,
        };
        if next != status {
            self.threads[i].status = next;
            if next == Status::Waiting {
                self.asked_on_screen.insert(thread_id.to_string());
            }
            self.persist(thread_id);
        }
    }

    /// SPEC.md switch algorithm on the native route: copy the conversation into a new session of
    /// `to`, start `to` out of sight, and swap when it has drawn (see `finish_switch`).
    fn switch_native(&mut self, thread_id: &str, to: &str) -> Outcome {
        let i = self.idx(thread_id)?;
        let from = self.threads[i].active_agent.clone();
        if from == to {
            return Err(format!("This thread is already on {to}."));
        }
        if let Some(p) = self.pending.get(thread_id) {
            return Err(format!("desl is already switching this thread to {}. It takes a few seconds.", p.to));
        }
        self.usable(to)?;
        let adapter = self.adapter(to).ok_or_else(|| format!("desl has no adapter for {to}."))?;
        // The last of what the agent wrote, so the copy is whole.
        self.poll_tail(thread_id);
        // A prompt sent a moment ago may not have reached the agent's files yet.
        let just_sent = self.entered.get(thread_id).is_some_and(|t| t.elapsed() < Duration::from_secs(3));
        if self.mid_turn(i) || just_sent {
            return Err(format!(
                "{from} is still working, and a half-finished turn is never copied. Wait for it to finish, then press {} s again.",
                self.prefix()
            ));
        }
        let t = &self.threads[i];
        let cwd = t.cwd.clone();
        let kinds: Vec<EventKind> = self.store.all_events(thread_id).unwrap_or_default().into_iter().map(|e| e.kind).collect();
        let (open, copied, brief) = match adapter.write_session(Path::new(&cwd), &kinds) {
            Ok((session_id, _, len)) => (Open::Resume { session_id: session_id.clone() }, Some((session_id, len)), None),
            Err(e) => {
                tracing::warn!(thread = %thread_id, to, "could not copy the conversation, sending the brief: {e}");
                let brief = self.brief_for(i, to);
                (Open::New { session_id: Some(desl_ir::adapter::new_uuid()) }, None, brief.needed.then_some(brief.text))
            }
        };
        let t = &self.threads[i];
        let binding = t.bindings.get(to);
        let launch = adapter.launch(&LaunchOpts {
            cwd: cwd.clone().into(),
            open,
            model: binding.and_then(|b| b.model_choice.clone()),
            effort: binding.and_then(|b| b.effort_choice.clone()),
            permission_mode: t.permission_mode,
            mcp: self.mcp_for(thread_id),
        });
        let (rows, cols) = self.ptys.get(thread_id).map_or((24, 80), |(_, p)| p.size());
        self.generation += 1;
        let generation = self.generation;
        let (out_tx, exit_tx) = (self.tx.clone(), self.tx.clone());
        let (out_id, exit_id) = (thread_id.to_string(), thread_id.to_string());
        let pty = Pty::spawn(
            to,
            thread_id,
            &launch,
            Path::new(&cwd),
            rows,
            cols,
            move |data| {
                let _ = out_tx.send(Msg::PtyOutput { thread_id: out_id.clone(), generation, data });
            },
            move || {
                let _ = exit_tx.send(Msg::PtyExit { thread_id: exit_id.clone(), generation });
            },
        )?;
        tracing::info!(thread = %thread_id, from = %from, to, args = ?launch.args, "switching");
        let copied = copied.or_else(|| launch.session_id.clone().map(|id| (id, 0)));
        let now = Instant::now();
        self.pending.insert(
            thread_id.to_string(),
            Pending {
                to: to.to_string(),
                generation,
                pty,
                started: now,
                started_at: SystemTime::now(),
                last_output: now,
                saw_output: false,
                copied,
                brief,
            },
        );
        Ok(Reply::Done)
    }

    /// The receiving agent has drawn the conversation: it becomes the thread's agent and every
    /// client showing the thread gets its screen in one frame. The old agent ends; its session
    /// stays in its own resume list.
    fn finish_switch(&mut self, thread_id: &str) {
        let Some(mut p) = self.pending.remove(thread_id) else { return };
        let Ok(i) = self.idx(thread_id) else { return };
        self.poll_tail(thread_id);
        self.tails.remove(thread_id);
        self.ptys.remove(thread_id);
        let from = std::mem::replace(&mut self.threads[i].active_agent, p.to.clone());
        let head = self.threads[i].head;
        let ts = now();
        let b = self.threads[i].bindings.entry(p.to.clone()).or_insert_with(|| Binding { agent: p.to.clone(), ..Binding::default() });
        b.session_id = p.copied.as_ref().map(|(id, _)| id.clone());
        b.file_offset = p.copied.as_ref().map_or(0, |(_, len)| *len);
        b.seen_up_to = head;
        b.last_used_at = ts;
        self.threads[i].status = Status::Idle;
        let brief_event_id = p.brief.as_ref().and_then(|text| {
            let token_estimate = projector::estimate_tokens(text);
            self.append(thread_id, EventKind::Brief { text: text.clone(), token_estimate, covers_range: (1, head), delta: false })
                .map(|e| e.index)
        });
        self.append(thread_id, EventKind::AgentSwitch { from, to: p.to.clone(), brief_event_id });
        if let Some(text) = &p.brief {
            // Into the prompt, as a paste; the user adds their question and sends both.
            p.pty.write(format!("\x1b[200~{text}\x1b[201~").as_bytes());
        }
        let replay = b64::encode(p.pty.log());
        if let Some(adapter) = self.adapter(&p.to)
            && let Some(reader) = adapter.session_reader(Path::new(&self.threads[i].cwd))
        {
            self.tails.insert(
                thread_id.to_string(),
                Tail {
                    agent: p.to.clone(),
                    reader,
                    path: None,
                    since: p.started_at,
                    skip_existing: false,
                    partial: Vec::new(),
                    last_poll: Instant::now(),
                    title_asked: None,
                },
            );
        }
        self.ptys.insert(thread_id.to_string(), (p.generation, p.pty));
        self.persist(thread_id);
        let frame = ServerFrame::PtyReplace { thread_id: thread_id.to_string(), agent: p.to.clone(), data: replay };
        for client in self.showing(thread_id) {
            self.send(client, &frame);
        }
        tracing::info!(thread = %thread_id, to = %p.to, "switched");
    }

    fn remove_thread(&mut self, thread_id: &str) {
        let Ok(i) = self.idx(thread_id) else { return };
        // Dropping the sessions ends their processes.
        self.live.remove(thread_id);
        self.ptys.remove(thread_id);
        self.tails.remove(thread_id);
        self.pending.remove(thread_id);
        self.restarting.remove(thread_id);
        for c in self.clients.values_mut() {
            c.shows.remove(thread_id);
        }
        if let Err(e) = self.store.delete_thread(thread_id) {
            tracing::error!(thread = %thread_id, "could not delete the thread: {e:#}");
        }
        self.threads.remove(i);
        self.broadcast(Change::ThreadRemoved { thread_id: thread_id.to_string() });
    }

    fn showing(&self, thread_id: &str) -> Vec<ClientId> {
        self.clients.iter().filter(|(_, c)| c.shows.contains(thread_id)).map(|(id, _)| *id).collect()
    }

    fn pty_exit(&mut self, thread_id: &str, generation: u64) {
        if self.pending.get(thread_id).is_some_and(|p| p.generation == generation) {
            let to = self.pending.remove(thread_id).map(|p| p.to).unwrap_or_default();
            let from = self.idx(thread_id).map(|i| self.threads[i].active_agent.clone()).unwrap_or_default();
            let frame = ServerFrame::Notice {
                thread_id: thread_id.to_string(),
                text: format!("{to} quit before it was ready, so the thread stays on {from}. Press {} s to try again.", self.prefix()),
            };
            for client in self.showing(thread_id) {
                self.send(client, &frame);
            }
            return;
        }
        // A restart ends here either way: this is the exit it waited for, or something started
        // since has taken the thread.
        let restart = self.restarting.remove(thread_id);
        if self.ptys.get(thread_id).is_none_or(|(g, _)| *g != generation) {
            return;
        }
        // What the agent wrote last, before it is forgotten.
        self.poll_tail(thread_id);
        self.tails.remove(thread_id);
        if let Ok(i) = self.idx(thread_id)
            && self.threads[i].status.busy()
        {
            self.threads[i].status = Status::Idle;
            self.persist(thread_id);
        }
        let code = self.ptys.get_mut(thread_id).and_then(|(_, p)| p.exit_code());
        let (rows, cols) = self.ptys.get(thread_id).map_or((40, 120), |(_, p)| p.size());
        self.ptys.remove(thread_id);
        tracing::info!(thread = %thread_id, ?code, "the agent's interface ended");
        if restart && let Ok(i) = self.idx(thread_id) {
            match self.start_pty(i, rows, cols) {
                Ok(()) => {
                    let agent = self.threads[i].active_agent.clone();
                    let frame = ServerFrame::PtyReplace { thread_id: thread_id.to_string(), agent, data: String::new() };
                    for client in self.showing(thread_id) {
                        self.send(client, &frame);
                    }
                    tracing::info!(thread = %thread_id, "the agent started again on its conversation");
                    return;
                }
                Err(e) => tracing::warn!(thread = %thread_id, "the agent could not start again: {e}"),
            }
        }
        let frame = ServerFrame::PtyEnded { thread_id: thread_id.to_string(), code };
        // A task list closed on screen stays in the sidebar while its file is there.
        let own_program = self.idx(thread_id).is_ok_and(|i| !is_agent(&self.threads[i].active_agent) && !self.keeps_list(i));
        let shown: Vec<ClientId> = self.clients.iter().filter(|(_, c)| c.shows.contains(thread_id)).map(|(id, _)| *id).collect();
        for client in shown {
            self.send(client, &frame);
            if let Some(c) = self.clients.get_mut(&client) {
                c.shows.remove(thread_id);
            }
        }
        // A shell or a file manager that exited is gone: there is nothing to come back to.
        if own_program {
            self.remove_thread(thread_id);
        }
    }

    /// True for a tasks thread whose project has its list file: it stays in the sidebar.
    fn keeps_list(&self, i: usize) -> bool {
        let t = &self.threads[i];
        t.active_agent == TASKS
            && self.projects.iter().find(|p| p.id == t.project_id).is_some_and(|p| Path::new(&p.path).join(TASK_LIST).is_file())
    }

    /// A project in the sidebar whose root holds a task list shows it, as a thread of its own,
    /// whoever made the file: an agent that starts one with `desl task add` needs no one to open
    /// it. Its program starts only when the thread is shown.
    fn task_lists(&mut self) {
        let missing: Vec<String> = self
            .projects
            .iter()
            .filter(|p| self.threads.iter().any(|t| t.project_id == p.id))
            .filter(|p| !self.threads.iter().any(|t| t.active_agent == TASKS && t.project_id == p.id))
            .filter(|p| Path::new(&p.path).join(TASK_LIST).is_file())
            .map(|p| p.id.clone())
            .collect();
        for project_id in missing {
            let Ok(id) = self.create_thread(&project_id, TASKS) else { continue };
            if let Ok(i) = self.idx(&id) {
                self.threads[i].name = TASKS.to_string();
            }
            self.persist(&id);
            tracing::info!(thread = %id, project = %project_id, "the project has a task list");
        }
    }

    /// Branch state changes behind desl's back; the threads on screen are checked every few seconds.
    fn tick(&mut self) {
        let ready: Vec<String> = self.pending.iter().filter(|(_, p)| p.ready()).map(|(id, _)| id.clone()).collect();
        for id in ready {
            self.finish_switch(&id);
        }
        let typing: Vec<String> = self.typing.keys().cloned().collect();
        for id in typing {
            self.deliver(&id);
        }
        let stale: Vec<String> =
            self.awaiting_turn.iter().filter(|(_, t)| t.elapsed() >= TURN_START_LIMIT).map(|(id, _)| id.clone()).collect();
        for id in stale {
            self.awaiting_turn.remove(&id);
            if let Ok(i) = self.idx(&id)
                && self.threads[i].status == Status::Working
            {
                tracing::warn!(thread = %id, "the agent took a prompt but started no turn");
                self.threads[i].status = Status::Idle;
                self.persist(&id);
            }
        }
        self.ticks += 1;
        if !self.ticks.is_multiple_of(TICKS_PER_SECOND) {
            return;
        }
        let tailed: Vec<String> = self.tails.keys().cloned().collect();
        for id in tailed {
            self.poll_tail(&id);
        }
        let queued: Vec<String> = self.threads.iter().filter(|t| !t.queued.is_empty()).map(|t| t.id.clone()).collect();
        for id in queued {
            self.drain(&id);
        }
        if !self.ticks.is_multiple_of(5 * TICKS_PER_SECOND) {
            return;
        }
        self.task_lists();
        self.refresh_git();
    }

    /// Reads every project's branch and uncommitted count, and the shown threads', on a worker
    /// thread: `git status` on a large tree takes long enough to stall the agents' output if it
    /// ran here. Nothing is read while no client is attached, or while a read is under way.
    fn refresh_git(&mut self) {
        if self.git_busy || self.clients.is_empty() {
            return;
        }
        let mut dirs: Vec<String> = self.projects.iter().map(|p| p.path.clone()).collect();
        for c in self.clients.values() {
            for id in c.shows.iter().chain(c.viewing.iter()) {
                if let Ok(i) = self.idx(id)
                    && !dirs.contains(&self.threads[i].cwd)
                {
                    dirs.push(self.threads[i].cwd.clone());
                }
            }
        }
        self.git_busy = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let states = dirs
                .into_iter()
                .map(|d| {
                    let state = git::state(Path::new(&d));
                    (d, state)
                })
                .collect();
            let _ = tx.send(Msg::Git { states });
        });
    }

    /// The worker's answer: every project and thread in a directory whose state moved tells the
    /// clients.
    fn git_states(&mut self, states: Vec<(String, GitState)>) {
        self.git_busy = false;
        for (dir, state) in states {
            let projects: Vec<usize> =
                self.projects.iter().enumerate().filter(|(_, p)| p.path == dir && p.git != state).map(|(i, _)| i).collect();
            for i in projects {
                self.projects[i].git = state.clone();
                self.broadcast(Change::Project { project: self.projects[i].clone() });
            }
            let threads: Vec<usize> =
                self.threads.iter().enumerate().filter(|(_, t)| t.cwd == dir && t.git != state).map(|(i, _)| i).collect();
            for i in threads {
                self.threads[i].git = state.clone();
                self.broadcast(Change::Thread { thread: Box::new(self.threads[i].clone()) });
            }
        }
    }

    fn workspace(&self, project_id: Option<String>, focus: Option<String>) -> Workspace {
        Workspace {
            projects: self.projects.clone(),
            threads: self.threads.clone(),
            agents: self.agents.clone(),
            file_manager: self.file_manager(),
            view: self.store.meta_get("view").ok().flatten().and_then(|v| serde_json::from_str(&v).ok()),
            project_id,
            focus,
        }
    }

    /// What the thread's next prompt would carry if `to` were its agent now.
    fn brief_for(&self, i: usize, to: &str) -> BriefPreview {
        let t = &self.threads[i];
        let (seen, has_session) = match t.bindings.get(to) {
            Some(b) => (b.seen_up_to.min(t.head), b.session_id.is_some()),
            None => (0, false),
        };
        let unseen = if t.head > seen { self.store.events(&t.id, seen + 1, t.head).unwrap_or_default() } else { Vec::new() };
        let needed = unseen.iter().any(|e| e.kind.conversational());
        let diff = if has_session { String::new() } else { git::diff_head(Path::new(&t.cwd)) };
        let brief = projector::build(&unseen, None, has_session, &diff, projector::BUDGET_TOKENS);
        BriefPreview {
            to: to.to_string(),
            text: brief.text,
            token_estimate: brief.token_estimate,
            turns: brief.turns,
            delta: has_session,
            needed,
        }
    }

    /// Returns None when the reply comes later (`wait`).
    fn command(&mut self, client: ClientId, id: u64, command: Command) -> Option<Outcome> {
        Some(match command {
            Command::Hello { protocol, client: name } => {
                if protocol != PROTOCOL {
                    Err(format!(
                        "This desl speaks protocol {protocol} and the running daemon speaks {PROTOCOL}. Run `desl daemon restart` to start the daemon of this version."
                    ))
                } else {
                    tracing::debug!(client, name, "hello");
                    Ok(Reply::Hello { protocol: PROTOCOL, version: env!("CARGO_PKG_VERSION").to_string(), pid: std::process::id() })
                }
            }
            Command::Workspace => {
                self.task_lists();
                Ok(Reply::Workspace(Box::new(self.workspace(None, None))))
            }
            Command::Subscribe { cwd } => self.subscribe(client, cwd.as_deref()),
            Command::OpenThread { thread_id } => self.idx(&thread_id).map(|i| {
                if let Some(c) = self.clients.get_mut(&client) {
                    c.viewing = Some(thread_id.clone());
                }
                let events = self.store.all_events(&thread_id).unwrap_or_default();
                let partial = self.live.get(&thread_id).and_then(|l| l.partial.clone());
                Reply::Thread { thread: Box::new(self.threads[i].clone()), events, partial }
            }),
            Command::View { thread_id } => self.idx(&thread_id).map(|_| {
                if let Some(c) = self.clients.get_mut(&client) {
                    c.viewing = Some(thread_id.clone());
                }
                let partial = self.live.get(&thread_id).and_then(|l| l.partial.clone());
                Reply::Viewing { thread_id, partial }
            }),
            Command::Events { thread_id } => {
                self.idx(&thread_id).map(|_| Reply::Events { events: self.store.all_events(&thread_id).unwrap_or_default() })
            }
            Command::Prompt { thread_id, text } => self.prompt(&thread_id, &text),
            Command::FormNew { thread_id, form } => self.form_new(&thread_id, &form),
            Command::FormAnswer { form_id, answer } => self.form_answer(&form_id, &answer),
            Command::FormClose { form_id, by } => self.form_close(&form_id, &by),
            Command::FormDraft { form_id, draft } => self.form_draft(&form_id, &draft),
            Command::FormGet { form_id } => self.form_get(&form_id),
            Command::FormImage { form_id, path } => self.form_image(&form_id, &path),
            Command::Interrupt { thread_id } => self.interrupt(&thread_id),
            Command::Exit { thread_id } => self.exit_agent(&thread_id),
            Command::Restart { thread_id } => self.restart_agent(&thread_id),
            Command::Answer { thread_id, call_id, decision } => self.answer(&thread_id, call_id.as_deref(), decision),
            Command::Switch { thread_id, agent } => self.switch(&thread_id, &agent),
            Command::Fork { thread_id, agent, at, worktree } => self.fork(&thread_id, agent.as_deref(), at, worktree),
            Command::NewThread { cwd, agent, prompt, beside, worktree } => {
                self.new_thread(&cwd, agent.as_deref(), prompt.as_deref(), beside.as_deref(), worktree)
            }
            Command::Rename { thread_id, name } => self.idx(&thread_id).and_then(|i| {
                let name: String = name.trim().chars().take(MAX_NAME).collect();
                if name.is_empty() {
                    return Err("Give the thread a name: /rename <name>.".to_string());
                }
                self.threads[i].name = name;
                self.threads[i].named = true;
                self.persist(&thread_id);
                Ok(Reply::Done)
            }),
            Command::MoveProject { project_id, up } => self.move_project(&project_id, up),
            Command::RemoveThread { thread_id } if self.idx(&thread_id).is_ok_and(|i| self.keeps_list(i)) => {
                let project = self.idx(&thread_id).ok().map(|i| self.threads[i].project_id.clone()).unwrap_or_default();
                let root = self.projects.iter().find(|p| p.id == project).map(|p| tilde(&p.path)).unwrap_or_default();
                Err(format!(
                    "The task list stays in the sidebar while {root}/{TASK_LIST} exists: q closes it on screen. Delete the file to remove it for good."
                ))
            }
            Command::RemoveThread { thread_id } => self.idx(&thread_id).map(|i| {
                let t = &self.threads[i];
                let wt = t.worktree.clone().map(|name| (t.git.branch.clone().unwrap_or(name), t.cwd.clone()));
                self.remove_thread(&thread_id);
                // Its worktree goes only when nothing in it would be lost; its branch always stays.
                let note = wt.map(|(branch, path)| match git::worktree_remove(Path::new(&path)) {
                    Ok(()) => format!("Removed its worktree; the branch {branch} keeps its commits."),
                    Err(_) => format!("Its worktree {} has uncommitted changes, so it stays.", tilde(&path)),
                });
                Reply::Removed { note }
            }),
            Command::Brief { thread_id, to } => self.idx(&thread_id).and_then(|i| {
                let to = match to {
                    Some(to) => {
                        self.usable(&to)?;
                        to
                    }
                    None => self.other_agent(&self.threads[i].active_agent)?,
                };
                Ok(Reply::Brief(self.brief_for(i, &to)))
            }),
            Command::Clear { thread_id } => self.idx(&thread_id).and_then(|i| {
                if self.mid_turn(i) {
                    return Err(format!(
                        "{} is still working. Wait for it to finish or press ctrl+c, then clear.",
                        self.threads[i].active_agent
                    ));
                }
                let agent = self.threads[i].active_agent.clone();
                self.live(&thread_id).sessions.remove(&agent);
                // The agent keeps its mark, so the fresh session starts clean: no brief of the old history.
                self.mark_seen(&thread_id, &agent);
                if let Some(b) = self.threads[i].bindings.get_mut(&agent) {
                    b.session_id = None;
                }
                self.persist(&thread_id);
                Ok(Reply::Done)
            }),
            Command::Models { thread_id } => match self.idx(&thread_id) {
                Ok(i) => {
                    let agent = self.threads[i].active_agent.clone();
                    if self.models.contains_key(&agent) {
                        self.models_reply(&thread_id, &agent)
                    } else {
                        // Asking may start the agent; that never happens on this thread.
                        let Some(adapter) = self.adapter(&agent) else {
                            return Some(Err(format!("desl has no adapter for {agent}.")));
                        };
                        let (tx, cwd) = (self.tx.clone(), self.threads[i].cwd.clone());
                        std::thread::spawn(move || {
                            let result = adapter.models(Path::new(&cwd)).map_err(|e| e.0);
                            let _ = tx.send(Msg::Models { client, id, thread_id, agent, result });
                        });
                        return None;
                    }
                }
                Err(e) => Err(e),
            },
            Command::SetModel { thread_id, model } => self.set_model(&thread_id, model.as_deref()),
            Command::SetEffort { thread_id, effort } => self.set_effort(&thread_id, effort.as_deref()),
            Command::SetMode { thread_id, mode } => self.set_mode(&thread_id, mode),
            Command::Wait { thread_id } => match self.idx(&thread_id) {
                Ok(i) if self.threads[i].status != Status::Working => Ok(Reply::Waited { status: self.threads[i].status }),
                Ok(_) => {
                    self.waiters.push((client, id, thread_id));
                    return None;
                }
                Err(e) => Err(e),
            },
            Command::Agents { refresh } => {
                if refresh {
                    self.models.clear();
                    self.agents = detect(&self.adapters);
                    self.broadcast(Change::Agents { agents: self.agents.clone() });
                }
                Ok(Reply::Agents { agents: self.agents.clone() })
            }
            Command::History { limit } => self
                .store
                .recent_prompts(limit.min(2000))
                .map(|prompts| Reply::History { prompts })
                .map_err(|e| format!("Could not read the prompt history ({e:#}). `desl doctor` checks the store.")),
            Command::Files { thread_id } => {
                self.idx(&thread_id).map(|i| Reply::Files { files: git::files(Path::new(&self.threads[i].cwd), 5000) })
            }
            Command::Blob { hash } => match self.store.blob_get(&hash) {
                Ok(Some(text)) => Ok(Reply::Blob { text }),
                _ => Err("That output is no longer stored. Only what the transcript shows is left.".to_string()),
            },
            Command::SaveView { view } => {
                if let Err(e) = self.store.meta_set("view", &view.to_string()) {
                    tracing::error!("could not save the view: {e:#}");
                }
                Ok(Reply::Done)
            }
            Command::Attach { thread_id, rows, cols, answers } => self.attach(client, &thread_id, rows, cols, answers),
            Command::Input { thread_id, data } => {
                if let (Some((_, pty)), Some(bytes)) = (self.ptys.get_mut(&thread_id), b64::decode(&data)) {
                    pty.write(&bytes);
                    if bytes.contains(&b'\r') || bytes.windows(4).any(|w| w == b"[13u" || w == b"13;1") {
                        self.entered.insert(thread_id.clone(), Instant::now());
                    }
                }
                return None;
            }
            Command::Resize { thread_id, rows, cols } => {
                if let Some((_, pty)) = self.ptys.get_mut(&thread_id) {
                    pty.resize(rows, cols);
                }
                return None;
            }
            Command::Leave { thread_id } => {
                if let Some(c) = self.clients.get_mut(&client) {
                    c.shows.remove(&thread_id);
                }
                return None;
            }
            Command::Subagent { thread_id, subagent_id } => self.idx(&thread_id).map(|i| {
                let t = &self.threads[i];
                // Read from the session that started it, which may be another agent's now.
                let (agent, session) = match t.subagents.iter().find(|s| s.id == subagent_id).filter(|s| !s.agent.is_empty()) {
                    Some(s) => (s.agent.clone(), Some(s.session_id.clone())),
                    None => (t.active_agent.clone(), t.bindings.get(&t.active_agent).and_then(|b| b.session_id.clone())),
                };
                let kinds = match (self.adapter(&agent), session) {
                    (Some(a), Some(s)) => a.subagent_events(Path::new(&t.cwd), &s, &subagent_id),
                    _ => Vec::new(),
                };
                let events = kinds
                    .into_iter()
                    .enumerate()
                    .map(|(n, kind)| Event { index: n as u64 + 1, thread_id: thread_id.clone(), timestamp: 0, agent: agent.clone(), kind })
                    .collect();
                Reply::Events { events }
            }),
            Command::Doctor => Ok(Reply::Doctor { checks: self.doctor() }),
            Command::Shutdown => {
                self.stopping = true;
                Ok(Reply::Done)
            }
            Command::Handover { exe } => {
                if self.handover.is_some() {
                    return Some(Err("The desl daemon is already starting again. Wait a moment, then run desl.".to_string()));
                }
                let exe = PathBuf::from(exe);
                if !exe.is_file() {
                    return Some(Err(format!(
                        "There is no desl binary at {}. Install desl again, then run `desl daemon restart`.",
                        exe.display()
                    )));
                }
                // Answered by the server once the next daemon serves, or with why it does not.
                self.handover = Some(Handover { client, id, exe, asked: Instant::now() });
                return None;
            }
        })
    }

    fn models_reply(&self, thread_id: &str, agent: &str) -> Outcome {
        let i = self.idx(thread_id)?;
        let models = self.models.get(agent).cloned().unwrap_or_default();
        let binding = self.threads[i].bindings.get(agent);
        let choice = binding.and_then(|b| b.model_choice.clone());
        let current = choice.or_else(|| models.iter().find(|m| m.default).map(|m| m.id.clone()));
        Ok(Reply::Models { agent: agent.to_string(), models, current, effort: binding.and_then(|b| b.effort_choice.clone()) })
    }

    /// The model a thread's agent runs: the user's pick, else the agent's default. None while
    /// the agent's list has not been read.
    fn model_in_use(&self, i: usize, agent: &str) -> Option<&ModelInfo> {
        let list = self.models.get(agent)?;
        match self.threads[i].bindings.get(agent).and_then(|b| b.model_choice.as_deref()) {
            Some(choice) => list.iter().find(|m| m.id == choice),
            None => list.iter().find(|m| m.default),
        }
    }

    /// `/model`: the pick is kept in the thread's binding and reaches the agent with its next
    /// turn, which starts a process with that model and resumes the native session.
    fn set_model(&mut self, thread_id: &str, model: Option<&str>) -> Outcome {
        let i = self.idx(thread_id)?;
        let agent = self.threads[i].active_agent.clone();
        if self.mid_turn(i) {
            return Err(format!("{agent} is still working. Wait for it to finish or press ctrl+c, then change the model."));
        }
        let picked = match model.map(str::trim).filter(|m| !m.is_empty()) {
            None => None,
            Some(wanted) => match self.models.get(&agent) {
                // The list is known: the name has to be on it.
                Some(list) => match list.iter().find(|m| m.id.eq_ignore_ascii_case(wanted) || m.name.eq_ignore_ascii_case(wanted)) {
                    Some(m) => Some(m.clone()),
                    // No model is called that: it means the agent's own.
                    None if wanted.eq_ignore_ascii_case("default") => None,
                    None => {
                        let ids: Vec<&str> = list.iter().map(|m| m.id.as_str()).collect();
                        return Err(format!("{agent} has no model named {wanted}. It offers {}.", ids.join(", ")));
                    }
                },
                None => Some(ModelInfo::named(wanted)),
            },
        };
        let b = self.threads[i].bindings.entry(agent.clone()).or_insert_with(|| Binding { agent: agent.clone(), ..Binding::default() });
        // Picking the agent's default is the same as not picking.
        b.model_choice = picked.as_ref().filter(|m| !m.default).map(|m| m.id.clone());
        // The live process keeps its model; ending it makes the next turn start one with the new model.
        self.live(thread_id).sessions.remove(&agent);
        // An effort the new model does not take would fail its next turn: drop it with the old model.
        let kept = self.threads[i].bindings.get(&agent).and_then(|b| b.effort_choice.clone());
        let fits = match (&kept, self.model_in_use(i, &agent)) {
            (Some(level), Some(m)) => m.efforts.iter().any(|e| &e.id == level),
            _ => true,
        };
        if !fits && let Some(b) = self.threads[i].bindings.get_mut(&agent) {
            b.effort_choice = None;
        }
        self.persist(thread_id);
        let effort = self.threads[i].bindings.get(&agent).and_then(|b| b.effort_choice.clone());
        Ok(Reply::ModelSet { agent, model: picked, effort })
    }

    /// The second half of `/model`: how hard the agent thinks. Kept in the binding like the
    /// model, and it reaches the agent the same way, with its next turn.
    fn set_effort(&mut self, thread_id: &str, effort: Option<&str>) -> Outcome {
        let i = self.idx(thread_id)?;
        let agent = self.threads[i].active_agent.clone();
        if self.mid_turn(i) {
            return Err(format!("{agent} is still working. Wait for it to finish or press ctrl+c, then change the effort."));
        }
        let model = self.model_in_use(i, &agent).cloned();
        let wanted = effort.map(|e| e.trim().to_lowercase()).filter(|e| !e.is_empty() && e != "default");
        // The model's levels are known: the level has to be one of them.
        if let (Some(level), Some(m)) = (&wanted, &model)
            && !m.efforts.iter().any(|e| &e.id == level)
        {
            let levels: Vec<&str> = m.efforts.iter().map(|e| e.id.as_str()).collect();
            return Err(match levels.is_empty() {
                true => format!("{} has no effort levels to choose from.", m.name),
                false => format!("{} has no effort level named {level}. It takes {}.", m.name, levels.join(", ")),
            });
        }
        let b = self.threads[i].bindings.entry(agent.clone()).or_insert_with(|| Binding { agent: agent.clone(), ..Binding::default() });
        b.effort_choice = wanted.clone();
        self.live(thread_id).sessions.remove(&agent);
        self.persist(thread_id);
        Ok(Reply::ModelSet { agent, model: model.filter(|m| !m.default), effort: wanted })
    }

    fn subscribe(&mut self, client: ClientId, cwd: Option<&str>) -> Outcome {
        // An agent installed or removed since the daemon last looked: look again, so the menu
        // a client draws from this answer offers what can be started.
        if self.adapters.iter().zip(&self.agents).any(|(a, info)| a.installed() != info.detection.installed) {
            self.models.clear();
            self.agents = detect(&self.adapters);
            self.broadcast(Change::Agents { agents: self.agents.clone() });
        }
        let mut project_id = None;
        let mut focus = None;
        if let Some(cwd) = cwd {
            let pid = self.project_for(Path::new(cwd))?;
            if !self.threads.iter().any(|t| t.project_id == pid) {
                let agent = self.default_agent(&pid)?;
                self.create_thread(&pid, &agent)?;
            }
            let view: Option<serde_json::Value> = self.store.meta_get("view").ok().flatten().and_then(|v| serde_json::from_str(&v).ok());
            let last = view.as_ref().and_then(|v| v.get("current")).and_then(|c| c.as_str()).map(str::to_string);
            focus = last
                .filter(|id| self.threads.iter().any(|t| t.id == *id && t.project_id == pid))
                .or_else(|| self.threads.iter().filter(|t| t.project_id == pid).max_by_key(|t| t.updated_at).map(|t| t.id.clone()));
            project_id = Some(pid);
        }
        if let Some(c) = self.clients.get_mut(&client) {
            c.subscribed = true;
        }
        Ok(Reply::Workspace(Box::new(self.workspace(project_id, focus))))
    }

    /// A prompt to the thread's agent, after a line for each form the user dismissed since its
    /// last one, so it knows no answer is coming.
    fn prompt(&mut self, thread_id: &str, text: &str) -> Outcome {
        self.idx(thread_id)?;
        let text = text.trim();
        if text.is_empty() {
            return Err("The prompt is empty. Type something to send.".to_string());
        }
        let notes = self.store.untold_dismissals(thread_id).unwrap_or_default();
        if notes.is_empty() {
            return self.send_prompt(thread_id, text);
        }
        let lines: Vec<String> = notes.iter().map(|(id, title)| form::dismissed_note(id, title)).collect();
        let sent = self.send_prompt(thread_id, &format!("{}\n\n{text}", lines.join("\n")))?;
        let ids: Vec<String> = notes.into_iter().map(|(id, _)| id).collect();
        if let Err(e) = self.store.mark_told(&ids) {
            tracing::warn!(thread = thread_id, "could not note that the agent heard of a dismissed form: {e:#}");
        }
        Ok(sent)
    }

    fn send_prompt(&mut self, thread_id: &str, text: &str) -> Outcome {
        let i = self.idx(thread_id)?;
        let agent = self.threads[i].active_agent.clone();
        if !is_agent(&agent) {
            return Err(format!(
                "A {} thread has no agent to prompt. Start one with {} c, or desl new.",
                program_word(&agent),
                self.prefix()
            ));
        }
        // An agent desl runs in its own interface gets the prompt typed there; the headless
        // turn is left for agents without one.
        let native = self.adapter(&agent).is_some_and(|a| a.session_reader(Path::new(&self.threads[i].cwd)).is_some());
        if !self.agent_free(i) || self.typing.contains_key(thread_id) {
            self.store
                .queue_push(thread_id, text, now())
                .map_err(|e| format!("Could not queue the prompt ({e:#}). Send it again when the agent is idle."))?;
            self.threads[i].queued.push(text.to_string());
            self.persist(thread_id);
            return Ok(Reply::Sent { queued: true });
        }
        self.usable(&agent)?;
        if native {
            if !self.ptys.contains_key(thread_id) {
                self.start_pty(i, 40, 120)?;
            }
            self.type_prompt(thread_id, text.to_string());
            return Ok(Reply::Sent { queued: false });
        }
        self.dispatch(thread_id, text, None);
        Ok(Reply::Sent { queued: false })
    }

    // ---- forms (docs/design/2026-11-21-web-forms.md) ----------------------------------------

    fn new_form_id(&self) -> String {
        use std::hash::{Hash, Hasher};
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        for salt in 0u32.. {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (nanos, salt, std::process::id(), "form").hash(&mut h);
            let id = format!("f-{:04x}", h.finish() & 0xffff);
            if self.store.form(&id).ok().flatten().is_none() {
                return id;
            }
        }
        unreachable!("a free form id always exists")
    }

    fn form_new(&mut self, thread_id: &str, form: &Value) -> Outcome {
        let i = self.idx(thread_id)?;
        let agent = self.threads[i].active_agent.clone();
        if !is_agent(&agent) {
            return Err(format!("A {} thread has no agent to put a form in front of the user.", program_word(&agent)));
        }
        let json = form.to_string();
        if json.len() > form::MAX_BYTES {
            return Err(format!(
                "The form is {} KB; desl takes at most 256 KB. Put long text in a file of the project and name the file in the form.",
                json.len().div_ceil(1024)
            ));
        }
        let checked = form::check(form)?;
        let dir = PathBuf::from(&self.threads[i].cwd);
        for path in form::image_paths(&checked) {
            crate::picture::check(&dir, path)?;
        }
        let blob = self.store.blob_put(&json).map_err(|e| format!("desl could not keep the form ({e:#}). Try again."))?;
        let id = self.new_form_id();
        let at = now();
        self.store
            .form_put(&id, thread_id, &checked.title, at, &blob)
            .map_err(|e| format!("desl could not keep the form ({e:#}). Try again."))?;
        self.append(thread_id, EventKind::Form { form_id: id.clone(), title: checked.title.clone(), blob });
        self.threads[i].open_forms.push(OpenForm { id: id.clone(), title: checked.title, at });
        self.persist(thread_id);
        Ok(Reply::FormShown { form_id: id })
    }

    /// Thread `i`'s agent has no turn going and its screen asks nothing, so a prompt can go in.
    /// A thread waiting on a form is free: the agent is idle, and the user may talk to it.
    fn agent_free(&self, i: usize) -> bool {
        !self.mid_turn(i)
    }

    /// A turn is under way on thread `i`, or a dialog on its agent's screen or an approval waits
    /// on the user. A wait on a form alone is not one: switch, model, clear and stop treat the
    /// thread as idle. A waiting thread with no form open waits on its screen even when this
    /// daemon did not see the dialog begin, as after a handover.
    fn mid_turn(&self, i: usize) -> bool {
        let t = &self.threads[i];
        t.mid_turn() || (t.status == Status::Waiting && self.asked_on_screen.contains(&t.id))
    }

    /// A form the store has, open: a sentence otherwise.
    fn open_form(&self, form_id: &str) -> Result<StoredForm, String> {
        let row = self
            .store
            .form(form_id)
            .map_err(|e| format!("desl could not read {form_id} ({e:#})."))?
            .ok_or_else(|| format!("There is no form {form_id}. `desl form list` lists the open ones."))?;
        match row.closed_by.as_deref() {
            None => Ok(row),
            Some("user") => Err(format!("{form_id} was answered already. Nothing more was sent.")),
            Some("withdrawn") => Err(format!("{form_id} was withdrawn by its agent. Nothing was sent.")),
            Some(_) => Err(format!("{form_id} was dismissed. Nothing was sent.")),
        }
    }

    fn form_answer(&mut self, form_id: &str, answer: &Value) -> Outcome {
        let row = self.open_form(form_id)?;
        self.idx(&row.thread_id)?;
        let text = self.store.blob_get(&row.blob).ok().flatten().ok_or_else(|| format!("desl lost the form {form_id}."))?;
        let form = serde_json::from_str::<Value>(&text).map_err(|e| e.to_string()).and_then(|v| form::check(&v))?;
        form::check_answer(&form, answer)?;
        let blob =
            self.store.blob_put(&answer.to_string()).map_err(|e| format!("desl could not keep the answer ({e:#}). Send it again."))?;
        self.closed(&row, "user", Some(blob))?;
        self.prompt(&row.thread_id, &form::answer_prompt(form_id, &row.title, answer))
            .map_err(|e| format!("{form_id} is answered and its answer kept, but desl could not send it to the agent: {e}"))?;
        Ok(Reply::Done)
    }

    fn form_close(&mut self, form_id: &str, by: &str) -> Outcome {
        if !matches!(by, "withdrawn" | "dismissed") {
            return Err(format!("A form closes unanswered as dismissed or withdrawn, not \"{by}\"."));
        }
        let row = self.open_form(form_id)?;
        self.closed(&row, by, None)?;
        Ok(Reply::Done)
    }

    /// Closes the form in the store and in its thread, which no longer waits on it.
    fn closed(&mut self, row: &StoredForm, by: &str, answer: Option<String>) -> Result<(), String> {
        let done =
            self.store.form_close(&row.id, by, answer.as_deref()).map_err(|e| format!("desl could not close {} ({e:#}).", row.id))?;
        if !done {
            return Err(format!("{} was closed already. Nothing was sent.", row.id));
        }
        self.append(&row.thread_id, EventKind::FormAnswer { form_id: row.id.clone(), by: by.to_string(), blob: answer });
        if let Ok(i) = self.idx(&row.thread_id) {
            self.threads[i].open_forms.retain(|f| f.id != row.id);
            if self.threads[i].status == Status::Waiting && !self.asked_on_screen.contains(&row.thread_id) {
                self.threads[i].status = Status::Idle;
            }
        }
        self.persist(&row.thread_id);
        Ok(())
    }

    fn form_draft(&mut self, form_id: &str, draft: &Value) -> Outcome {
        let json = draft.to_string();
        if json.len() > form::MAX_BYTES {
            return Err("The draft is over 256 KB, so desl did not keep it.".to_string());
        }
        self.open_form(form_id)?;
        match self.store.form_draft(form_id, &json) {
            Ok(true) => Ok(Reply::Done),
            Ok(false) => Err(format!("{form_id} closed meanwhile, so the draft was not kept.")),
            Err(e) => Err(format!("desl could not keep the draft ({e:#}).")),
        }
    }

    fn form_get(&self, form_id: &str) -> Outcome {
        let row = self
            .store
            .form(form_id)
            .map_err(|e| format!("desl could not read {form_id} ({e:#})."))?
            .ok_or_else(|| format!("There is no form {form_id}."))?;
        let json = |hash: Option<&str>| {
            hash.and_then(|h| self.store.blob_get(h).ok().flatten()).and_then(|t| serde_json::from_str::<Value>(&t).ok())
        };
        let form = json(Some(&row.blob)).ok_or_else(|| format!("desl lost the form {form_id}."))?;
        Ok(Reply::Form {
            form_id: row.id,
            thread_id: row.thread_id,
            form,
            draft: row.draft.as_deref().and_then(|d| serde_json::from_str(d).ok()),
            answer: json(row.answer_blob.as_deref()),
            closed_by: row.closed_by,
        })
    }

    /// A picture the form shows, read from its thread's directory for the page.
    fn form_image(&self, form_id: &str, path: &str) -> Outcome {
        let row = self
            .store
            .form(form_id)
            .map_err(|e| format!("desl could not read {form_id} ({e:#})."))?
            .ok_or_else(|| format!("There is no form {form_id}."))?;
        let text = self.store.blob_get(&row.blob).ok().flatten().ok_or_else(|| format!("desl lost the form {form_id}."))?;
        let form = serde_json::from_str::<Value>(&text).map_err(|e| e.to_string()).and_then(|v| form::check(&v))?;
        // Only what the form names: the page cannot read other files through it.
        if !form::image_paths(&form).contains(&path) {
            return Err(format!("{form_id} shows no image \"{path}\"."));
        }
        let i = self.idx(&row.thread_id)?;
        let (mime, bytes) = crate::picture::read(Path::new(&self.threads[i].cwd), path)?;
        use base64::Engine as _;
        Ok(Reply::Image { mime: mime.to_string(), data: base64::engine::general_purpose::STANDARD.encode(bytes) })
    }

    fn interrupt(&mut self, thread_id: &str) -> Outcome {
        let i = self.idx(thread_id)?;
        if !self.mid_turn(i) {
            return Err(format!("{} is not working, so there is nothing to interrupt.", self.threads[i].active_agent));
        }
        let agent = self.threads[i].active_agent.clone();
        if self.ptys.contains_key(thread_id) {
            // A prompt not yet typed never reached the agent: it is dropped, and the thread is idle.
            if self.typing.remove(thread_id).is_some_and(|ty| ty.typed.is_none()) {
                self.threads[i].status = Status::Idle;
                self.persist(thread_id);
                return Ok(Reply::Done);
            }
            let key = self
                .adapter(&agent)
                .and_then(|a| a.interrupt_key())
                .ok_or_else(|| format!("desl knows no key that interrupts {agent}. Run `desl` and stop it in its own screen."))?;
            // As the user would press it; the agent's session file then records the interruption
            // and the end of the turn, and the thread follows it.
            if let Some((_, pty)) = self.ptys.get_mut(thread_id) {
                pty.write(key);
            }
            return Ok(Reply::Done);
        }
        let over = match self.live(thread_id).sessions.get_mut(&agent) {
            Some((_, session)) => session.interrupt(),
            None => true,
        };
        self.append(thread_id, EventKind::Interrupt);
        self.threads[i].pending_approval = None;
        if over {
            let live = self.live(thread_id);
            live.turn = None;
            live.partial = None;
            self.threads[i].status = Status::Idle;
            self.mark_seen(thread_id, &agent);
            self.persist(thread_id);
            self.dequeue(thread_id);
        } else {
            // The agent confirms with the end of its turn.
            self.threads[i].status = Status::Working;
            self.persist(thread_id);
        }
        Ok(Reply::Done)
    }

    /// Ends the thread's agent and keeps the thread: the agent's session holds the conversation,
    /// and the next prompt or view resumes it, as after idle reaping (SPEC.md "Process hygiene").
    fn exit_agent(&mut self, thread_id: &str) -> Outcome {
        self.end_agent(thread_id)?;
        Ok(Reply::Done)
    }

    /// Ends the thread's agent and, when its exit arrives, starts it again on the same
    /// conversation: it runs the binary and settings installed now, such as a new MCP server.
    /// Every client showing the thread gets the new screen in one frame, as after a switch.
    fn restart_agent(&mut self, thread_id: &str) -> Outcome {
        self.end_agent(thread_id)?;
        self.restarting.insert(thread_id.to_string());
        Ok(Reply::Done)
    }

    /// Ends the thread's agent now, as closing its terminal would; its exit arrives as any other.
    fn end_agent(&mut self, thread_id: &str) -> Result<(), String> {
        let i = self.idx(thread_id)?;
        let agent = self.threads[i].active_agent.clone();
        if !is_agent(&agent) {
            let what = program_word(&agent);
            return Err(format!(
                "{thread_id} is a {what}, and a {what} that ends closes its thread. `desl thread rm {thread_id}` closes it."
            ));
        }
        let Some((_, pty)) = self.ptys.get_mut(thread_id) else {
            return Err(format!("{agent} is not running in {thread_id}. Its next prompt starts it again."));
        };
        pty.end();
        // A switch under way ends with it; the thread stays on the agent it was on.
        self.pending.remove(thread_id);
        self.typing.remove(thread_id);
        self.awaiting_turn.remove(thread_id);
        if self.mid_turn(i) {
            self.append(thread_id, EventKind::Interrupt);
            self.threads[i].pending_approval = None;
        }
        tracing::info!(thread = %thread_id, agent = %agent, "ending the agent on request");
        // The agent's exit arrives as any other: its last lines are read, the thread goes idle.
        Ok(())
    }

    fn answer(&mut self, thread_id: &str, call_id: Option<&str>, decision: Decision) -> Outcome {
        let i = self.idx(thread_id)?;
        let Some(pending) = self.threads[i].pending_approval.clone().filter(|p| call_id.is_none_or(|c| c == p.call_id)) else {
            return Err(format!("Nothing in {thread_id} is waiting for an answer."));
        };
        let agent = self.threads[i].active_agent.clone();
        match self.live(thread_id).sessions.get_mut(&agent) {
            Some((_, session)) => session.answer(&pending.call_id, decision).map_err(|e| e.0)?,
            None => return Err(format!("{agent} is no longer running, so the answer was not sent. Send your prompt again to resume.")),
        }
        self.append(thread_id, EventKind::ApprovalResponse { call_id: pending.call_id, decision, by: "user".into() });
        if decision == Decision::Always {
            self.always(thread_id).insert(pending.tool);
        }
        self.threads[i].pending_approval = None;
        self.threads[i].status = Status::Working;
        self.persist(thread_id);
        Ok(Reply::Done)
    }

    /// `/permissions`: what the thread's agents may do without asking. The broker follows the
    /// new mode at once, also for a question that is on screen now; an agent's own flags follow
    /// with its next turn, so a turn that is running is never cut short.
    fn set_mode(&mut self, thread_id: &str, mode: PermissionMode) -> Outcome {
        let i = self.idx(thread_id)?;
        if mode == PermissionMode::Worktree {
            return Err("The worktree mode arrives with worktrees. Pick one of ask, edits or yolo.".to_string());
        }
        let agent = self.threads[i].active_agent.clone();
        let meaning = self.agents.iter().find(|a| a.id == agent).map_or_else(|| mode.meaning().to_string(), |a| a.mode_meaning(mode));
        if self.threads[i].permission_mode != mode {
            self.threads[i].permission_mode = mode;
            let live = self.live(thread_id);
            let started: Vec<String> = live.sessions.keys().cloned().collect();
            live.stale.extend(started);
            // A question already on screen is answered by the new policy.
            if mode == PermissionMode::Yolo
                && let Some(pending) = self.threads[i].pending_approval.take()
            {
                if let Some((_, session)) = self.live(thread_id).sessions.get_mut(&agent)
                    && let Err(e) = session.answer(&pending.call_id, Decision::Allow)
                {
                    tracing::warn!(thread = thread_id, "could not answer an approval: {e}");
                }
                self.append(
                    thread_id,
                    EventKind::ApprovalResponse { call_id: pending.call_id, decision: Decision::Allow, by: "policy".into() },
                );
                self.threads[i].status = Status::Working;
            }
            self.persist(thread_id);
        }
        Ok(Reply::ModeSet { mode, meaning })
    }

    /// SPEC.md switch algorithm, step 1. The brief goes out with the next prompt (steps 3 to 5).
    fn switch(&mut self, thread_id: &str, to: &str) -> Outcome {
        if let Some(what) = self.idx(thread_id).ok().map(|i| &self.threads[i].active_agent).filter(|a| !is_agent(a)) {
            return Err(format!(
                "A {} has no conversation to switch. Press {} c to start an agent in its directory.",
                program_word(what),
                self.prefix()
            ));
        }
        if self.ptys.contains_key(thread_id) {
            return self.switch_native(thread_id, to);
        }
        let i = self.idx(thread_id)?;
        let from = self.threads[i].active_agent.clone();
        if from == to {
            return Err(format!("This thread is already on {to}."));
        }
        self.usable(to)?;
        if self.mid_turn(i) {
            return Err(format!("{from} is still working. Wait for it to finish or press ctrl+c, then switch."));
        }
        self.threads[i].active_agent = to.to_string();
        self.append(thread_id, EventKind::AgentSwitch { from, to: to.to_string(), brief_event_id: None });
        if self.threads[i].status == Status::Failed {
            self.threads[i].status = Status::Idle;
        }
        self.persist(thread_id);
        Ok(Reply::Done)
    }

    /// SPEC.md fork algorithm. The fork gets the parent's events up to the fork point and no
    /// bindings, so its first prompt carries a full brief to a fresh native session.
    /// Gives thread `id` a git worktree of its own, branched from the commit `from` is on: the
    /// thread's agent starts there.
    fn give_worktree(&mut self, id: &str, from: &Path) -> Result<(), String> {
        let i = self.idx(id)?;
        let root = self
            .projects
            .iter()
            .find(|p| p.id == self.threads[i].project_id)
            .map(|p| PathBuf::from(&p.path))
            .ok_or("The thread has no project.")?;
        let name = id.replace('/', "-");
        let cfg = self.settings(Some(&root));
        let path = git::worktree_add(&root, &name, from, &cfg.worktree_dir, &cfg.branch_prefix)?;
        let t = &mut self.threads[i];
        t.worktree = Some(name);
        t.cwd = path.to_string_lossy().into_owned();
        t.git = git::state(&path);
        self.persist(id);
        Ok(())
    }

    fn fork(&mut self, thread_id: &str, agent: Option<&str>, at: Option<u64>, worktree: bool) -> Outcome {
        let i = self.idx(thread_id)?;
        let parent = self.threads[i].clone();
        if !is_agent(&parent.active_agent) {
            return Err(format!(
                "A {} has no conversation to fork. Press {} c to start an agent in its directory.",
                program_word(&parent.active_agent),
                self.prefix()
            ));
        }
        if parent.head == 0 {
            return Err("There is nothing to fork yet. Send a prompt first.".to_string());
        }
        let point = at.unwrap_or(parent.head);
        if point == 0 || point > parent.head {
            return Err(format!("{thread_id} has events 1 to {}; {point} is not one of them.", parent.head));
        }
        let agent = agent.map(str::to_string).unwrap_or_else(|| parent.active_agent.clone());
        self.usable(&agent)?;
        let mut k = 1;
        while self.threads.iter().any(|t| t.id == format!("{}/{k}", parent.id)) {
            k += 1;
        }
        let ts = now();
        let fork = Thread {
            id: format!("{}/{k}", parent.id),
            parent_id: Some(parent.id.clone()),
            fork_point: Some(point),
            active_agent: agent,
            status: Status::Idle,
            created_at: ts,
            updated_at: ts,
            head: point,
            // The parent's model and effort choices carry over; its sessions do not.
            bindings: parent
                .bindings
                .iter()
                .map(|(a, b)| {
                    let b = Binding { session_id: None, seen_up_to: 0, file_offset: 0, last_used_at: ts, ..b.clone() };
                    (a.clone(), b)
                })
                .collect(),
            queued: Vec::new(),
            pending_approval: None,
            subagents: Vec::new(),
            open_forms: Vec::new(),
            // The copy's agent may title it apart from the parent.
            named: false,
            // `yolo` is set per thread, by hand; a fork does not inherit it.
            permission_mode: match parent.permission_mode {
                PermissionMode::Yolo => PermissionMode::default(),
                mode => mode,
            },
            ..parent.clone()
        };
        let id = fork.id.clone();
        self.store
            .put_thread(&fork)
            .and_then(|()| self.store.copy_events(&parent.id, &id, point))
            .map_err(|e| format!("Could not fork {thread_id} ({e:#}). `desl doctor` checks the store."))?;
        self.threads.push(fork);
        self.append(&id, EventKind::Fork { parent_id: parent.id, fork_point: point });
        self.persist(&id);
        if worktree && let Err(why) = self.give_worktree(&id, Path::new(&parent.cwd)) {
            self.remove_thread(&id);
            return Err(why);
        }
        Ok(Reply::Created { thread_id: id })
    }

    fn new_thread(&mut self, cwd: &str, agent: Option<&str>, prompt: Option<&str>, beside: Option<&str>, worktree: bool) -> Outcome {
        let beside = beside.and_then(|id| self.idx(id).ok());
        if agent == Some(SHELL) {
            // In the project of the thread the user is on, at `cwd`, to find a directory from.
            let project_id = match beside {
                Some(b) => self.threads[b].project_id.clone(),
                None => self.project_for(Path::new(cwd))?,
            };
            let id = self.create_thread(&project_id, SHELL)?;
            let i = self.idx(&id)?;
            if Path::new(cwd).is_dir() {
                self.threads[i].cwd = cwd.to_string();
            }
            self.threads[i].name = shell_name();
            self.persist(&id);
            return Ok(Reply::Created { thread_id: id });
        }
        if agent == Some(TASKS) {
            // One list per project: the thread that shows it, else a new one at the project's root.
            let project_id = match beside {
                Some(b) => self.threads[b].project_id.clone(),
                None => self.project_for(Path::new(cwd))?,
            };
            if let Some(t) = self.threads.iter().find(|t| t.active_agent == TASKS && t.project_id == project_id) {
                return Ok(Reply::Created { thread_id: t.id.clone() });
            }
            let id = self.create_thread(&project_id, TASKS)?;
            let i = self.idx(&id)?;
            self.threads[i].name = TASKS.to_string();
            self.persist(&id);
            return Ok(Reply::Created { thread_id: id });
        }
        if agent == Some(FILES) {
            let program = self.file_manager().ok_or_else(|| self.no_file_manager())?;
            // In the folder of the thread the user is on: where its shell or file manager is
            // now, else the thread's own directory, which is its worktree when it has one.
            let (project_id, dir) = match beside {
                Some(b) => {
                    let t = &self.threads[b];
                    let live = if is_agent(&t.active_agent) { None } else { self.ptys.get(&t.id).and_then(|(_, p)| p.cwd()) };
                    (t.project_id.clone(), live.unwrap_or_else(|| PathBuf::from(&t.cwd)))
                }
                None => (self.project_for(Path::new(cwd))?, PathBuf::from(cwd)),
            };
            let id = self.create_thread(&project_id, FILES)?;
            let i = self.idx(&id)?;
            if dir.is_dir() {
                self.threads[i].cwd = dir.to_string_lossy().into_owned();
            }
            self.threads[i].name = program;
            self.persist(&id);
            return Ok(Reply::Created { thread_id: id });
        }
        // From a shell or a file manager: where it is now becomes the project, and it joins it.
        let from_shell = beside.filter(|&b| !is_agent(&self.threads[b].active_agent)).map(|b| self.threads[b].id.clone());
        let here = from_shell.as_ref().and_then(|id| self.ptys.get(id)).and_then(|(_, p)| p.cwd());
        let dir = here.clone().unwrap_or_else(|| cwd.into());
        let project_id = self.project_for(&dir)?;
        let agent = match agent {
            Some(a) => {
                self.usable(a)?;
                a.to_string()
            }
            None => self.default_agent(&project_id)?,
        };
        let id = self.create_thread(&project_id, &agent)?;
        if worktree {
            let root = self.projects.iter().find(|p| p.id == project_id).map(|p| PathBuf::from(&p.path)).unwrap_or_else(|| dir.clone());
            if let Err(why) = self.give_worktree(&id, &root) {
                self.remove_thread(&id);
                return Err(why);
            }
        }
        if let (Some(shell), Some(dir)) = (from_shell, here) {
            let i = self.idx(&shell)?;
            self.threads[i].project_id = project_id.clone();
            self.threads[i].cwd = dir.to_string_lossy().into_owned();
            self.persist(&shell);
        }
        if let Some(text) = prompt.map(str::trim).filter(|t| !t.is_empty()) {
            self.prompt(&id, text)?;
        }
        Ok(Reply::Created { thread_id: id })
    }

    /// Moves a project past the nearest one above or below it that the sidebar lists: a
    /// project without a thread has no row, and moving past it would look like nothing moved.
    fn move_project(&mut self, project_id: &str, up: bool) -> Outcome {
        let Some(i) = self.projects.iter().position(|p| p.id == project_id) else {
            return Err(format!("There is no project {project_id}. `desl status` lists the projects."));
        };
        let listed = |p: &Project| self.threads.iter().any(|t| t.project_id == p.id);
        let to = if up {
            (0..i).rev().find(|&j| listed(&self.projects[j]))
        } else {
            (i + 1..self.projects.len()).find(|&j| listed(&self.projects[j]))
        };
        let Some(to) = to else {
            return Err(format!("{project_id} is already the {} project.", if up { "first" } else { "last" }));
        };
        let project = self.projects.remove(i);
        self.projects.insert(to, project);
        let ids: Vec<String> = self.projects.iter().map(|p| p.id.clone()).collect();
        if let Err(e) = self.store.meta_set(PROJECT_ORDER, &serde_json::json!(ids).to_string()) {
            tracing::warn!("could not save the order of the projects: {e:#}");
        }
        self.broadcast(Change::ProjectOrder { ids });
        Ok(Reply::Done)
    }

    fn doctor(&self) -> Vec<Check> {
        let mut checks = Vec::new();
        let integrity = self.store.integrity().unwrap_or_else(|e| format!("{e:#}"));
        let (threads, events) = self.store.counts().unwrap_or((0, 0));
        checks.push(Check {
            name: "store".into(),
            ok: integrity == "ok",
            detail: if integrity == "ok" {
                format!("sound, {threads} threads and {events} events")
            } else {
                format!("SQLite reports: {integrity}. Stop the daemon and restore the database from a backup.")
            },
        });
        for a in &self.agents {
            let d = &a.detection;
            let (ok, detail) = if !d.installed {
                (false, format!("not installed. {}", d.install_hint))
            } else if d.logged_in == Some(false) {
                (false, format!("installed, not logged in. {}", d.login_hint))
            } else {
                let version = d.version.clone().unwrap_or_else(|| "unknown version".into());
                let fixtures = if d.version.as_deref() == Some(d.fixtures_version.as_str()) {
                    "matches the adapter's fixtures".to_string()
                } else {
                    format!("the adapter's fixtures are from {}; a newer agent may behave differently", d.fixtures_version)
                };
                (true, format!("{version}, logged in, {fixtures}"))
            };
            checks.push(Check { name: a.id.clone(), ok, detail });
        }
        let sessions: usize = self.live.values().map(|l| l.sessions.len()).sum();
        let working = (0..self.threads.len()).filter(|i| self.mid_turn(*i)).count();
        checks.push(Check {
            name: "daemon".into(),
            ok: true,
            detail: format!("pid {}, {sessions} agent sessions open, {working} threads working", std::process::id()),
        });
        checks
    }

    /// Ends every agent process and tells the clients. Called once, when the loop stops.
    pub fn shutdown(&mut self) {
        let busy: Vec<String> = (0..self.threads.len()).filter(|i| self.mid_turn(*i)).map(|i| self.threads[i].id.clone()).collect();
        for id in busy {
            self.append(
                &id,
                EventKind::Error {
                    message: "The desl daemon was stopped while this agent was working. Send your prompt again to resume.".into(),
                    recoverable: true,
                },
            );
            if let Ok(i) = self.idx(&id) {
                self.threads[i].status = Status::Idle;
                self.threads[i].pending_approval = None;
            }
            self.persist(&id);
        }
        // Dropping a session closes it: SIGTERM to its process group, SIGKILL after five seconds.
        self.live.clear();
        self.ptys.clear();
        self.tails.clear();
        self.pending.clear();
        self.broadcast(Change::Bye);
    }
}

#[cfg(test)]
mod tests;
