//! The client: renders what the daemon holds and turns keys into commands. It can be killed at
//! any time; the only state it owns is scroll position and focus, and the daemon keeps those too.

use crate::commands::COMMANDS;
use crate::conn::Link;
use crate::input::Input;
use crate::keymap::{self, Action, Binding, Ctx};
use crate::model::{Ask, AskKind, Item, Status, Thread};
use crate::picker::{Entry as PickEntry, Picker};
use desl_ir::{Decision, GitState, ModelInfo, PermissionMode, Project};
use desl_proto::{AgentInfo, BriefPreview, Change, Command, Reply, ServerFrame, StreamDelta};
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub enum Msg {
    Term(Event),
    Daemon(ServerFrame),
    DaemonGone,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Input,
    Transcript,
    Sidebar,
}

pub enum Overlay {
    None,
    Help,
    Picker(Picker, PickerKind),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PickerKind {
    Threads,
    Commands,
    Files,
    History,
    Agents,
    Models,
    Efforts,
    Modes,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Error,
}

pub struct Notice {
    pub text: String,
    pub level: Level,
    pub at: Instant,
}

/// Measurements the renderer leaves behind so key handling can clamp scrolling.
#[derive(Default)]
pub struct View {
    pub transcript_height: usize,
    pub transcript_lines: usize,
}

/// What a reply is for, by request id. Commands are sent and answered later; the screen never waits.
enum Pending {
    Subscribe,
    Open(String),
    View(String),
    /// A brief preview that decides what `/switch` asks.
    SwitchAsk {
        thread_id: String,
        to: String,
    },
    ShowBrief {
        thread_id: String,
    },
    /// A thread this client asked for: jump to it when it exists.
    Created,
    Files,
    History,
    /// The active agent's models, to choose from.
    ModelPicker,
    /// A model or an effort was picked: say what the agent will use.
    ModelSet,
    /// A permission mode was picked: say what it means with this agent.
    ModeSet,
    /// `/model <name> <effort>`: the model is set, now the effort.
    ModelThenEffort {
        thread_id: String,
        effort: String,
    },
    /// On success, say this.
    Say(String),
    /// Nothing to do on success. An error becomes a notice either way.
    Quiet,
}

pub struct App {
    pub cwd: PathBuf,
    pub projects: Vec<Project>,
    /// In sidebar order: by project, then by age.
    pub threads: Vec<Thread>,
    pub agents: Vec<AgentInfo>,
    pub current: usize,
    link: Link,
    pending: HashMap<u64, Pending>,
    pub attached: bool,
    /// A thread to select as soon as the daemon reports it.
    jump_to: Option<String>,
    pub input: Input,
    pub focus: Focus,
    pub overlay: Overlay,
    pub prefix_pending: bool,
    pub sidebar_visible: bool,
    pub notice: Option<Notice>,
    pub should_quit: bool,
    pub farewell: Option<String>,
    last_ctrl_c: Option<Instant>,
    pub dirty: bool,
    pub view: View,
    pub help_scroll: usize,
    pub clock: String,
    /// A file to open in `$EDITOR`; the main loop leaves the screen, runs the editor and returns.
    pub open_in_editor: Option<PathBuf>,
    /// What the last `/model` listed, so picking a model can go on to its effort levels.
    model_list: Vec<ModelInfo>,
    /// The thread's effort pick when that list was read.
    effort_choice: Option<String>,
}

const INFO_TTL: Duration = Duration::from_secs(6);

/// The row of the effort list that means not choosing, and what `/model <name> default` takes.
const DEFAULT_EFFORT: &str = "default";

fn fmt_tokens(n: u64) -> String {
    if n >= 1000 { format!("~{:.1}k tokens", n as f64 / 1000.0) } else { format!("~{n} tokens") }
}

/// What the screen shows for the few milliseconds before the daemon's first reply.
fn placeholder() -> Thread {
    Thread::new(desl_ir::Thread {
        id: String::new(),
        project_id: String::new(),
        name: String::new(),
        cwd: String::new(),
        worktree: None,
        parent_id: None,
        fork_point: None,
        active_agent: String::new(),
        status: Status::Idle,
        permission_mode: PermissionMode::default(),
        created_at: 0,
        updated_at: 0,
        head: 0,
        bindings: Default::default(),
        queued: Vec::new(),
        pending_approval: None,
        git: GitState::default(),
    })
}

impl App {
    pub fn new(cwd: PathBuf, link: Link) -> Self {
        let mut app = App {
            cwd,
            projects: Vec::new(),
            threads: vec![placeholder()],
            agents: Vec::new(),
            current: 0,
            link,
            pending: HashMap::new(),
            attached: false,
            jump_to: None,
            input: Input::default(),
            focus: Focus::Input,
            overlay: Overlay::None,
            prefix_pending: false,
            sidebar_visible: true,
            notice: None,
            should_quit: false,
            farewell: None,
            last_ctrl_c: None,
            dirty: true,
            view: View::default(),
            help_scroll: 0,
            clock: now_hhmm(),
            open_in_editor: None,
            model_list: Vec::new(),
            effort_choice: None,
        };
        let cwd = app.cwd.to_string_lossy().to_string();
        app.request(Command::Subscribe { cwd: Some(cwd) }, Pending::Subscribe);
        app
    }

    fn request(&mut self, command: Command, pending: Pending) {
        let id = self.link.request(command);
        self.pending.insert(id, pending);
    }

    pub fn thread(&self) -> &Thread {
        &self.threads[self.current.min(self.threads.len() - 1)]
    }
    pub fn thread_mut(&mut self) -> &mut Thread {
        let i = self.current.min(self.threads.len() - 1);
        &mut self.threads[i]
    }
    fn thread_by_id(&mut self, id: &str) -> Option<&mut Thread> {
        self.threads.iter_mut().find(|t| t.id() == id)
    }

    /// Runs on every pass of the loop: times each working turn and keeps the working line moving.
    pub fn pulse(&mut self) {
        for t in &mut self.threads {
            match t.status() {
                Status::Working => {
                    t.working_since.get_or_insert_with(Instant::now);
                }
                Status::Waiting => {}
                _ => t.working_since = None,
            }
        }
        if self.thread().status() == Status::Working {
            self.dirty = true;
        }
    }

    pub fn tick(&mut self) {
        let clock = now_hhmm();
        if clock != self.clock {
            self.clock = clock;
            self.dirty = true;
        }
        if let Some(n) = &self.notice
            && n.level == Level::Info
            && n.at.elapsed() > INFO_TTL
        {
            self.notice = None;
            self.dirty = true;
        }
    }

    pub fn handle(&mut self, msg: Msg) {
        self.dirty = true;
        match msg {
            Msg::Term(Event::Key(k)) => {
                if matches!(k.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                    self.on_key(k);
                }
            }
            Msg::Term(Event::Paste(s)) => {
                if self.focus == Focus::Input {
                    self.input.insert_str(&s);
                }
            }
            Msg::Term(_) => {}
            Msg::Daemon(frame) => self.on_frame(frame),
            Msg::DaemonGone => {
                if !self.should_quit {
                    self.error("Lost the desl daemon. Type /quit, then run desl again to reconnect.");
                }
            }
        }
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice { text: text.into(), level: Level::Info, at: Instant::now() });
    }
    pub fn error(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice { text: text.into(), level: Level::Error, at: Instant::now() });
    }

    pub fn approval_pending(&self) -> bool {
        self.thread().meta.pending_approval.is_some()
    }
    /// A one-line question is waiting above the prompt: an approval or an ask.
    pub fn strip_pending(&self) -> bool {
        self.approval_pending() || self.thread().ask.is_some()
    }

    // ---- daemon frames -------------------------------------------------------------------------

    /// Sidebar order: projects as the daemon lists them, threads by age within each.
    fn sort_threads(&mut self) {
        let current = self.thread().id().to_string();
        let order: HashMap<&str, usize> = self.projects.iter().enumerate().map(|(i, p)| (p.id.as_str(), i)).collect();
        self.threads.sort_by(|a, b| {
            let key =
                |t: &Thread| (order.get(t.meta.project_id.as_str()).copied().unwrap_or(usize::MAX), t.meta.created_at, t.meta.id.clone());
            key(a).cmp(&key(b))
        });
        if let Some(i) = self.threads.iter().position(|t| t.id() == current) {
            self.current = i;
        }
    }

    fn on_frame(&mut self, frame: ServerFrame) {
        match frame {
            ServerFrame::Reply { id, ok, error } => {
                let pending = self.pending.remove(&id).unwrap_or(Pending::Quiet);
                match (ok, error) {
                    (Some(reply), _) => self.on_reply(pending, reply),
                    (None, Some(sentence)) => {
                        if matches!(pending, Pending::Subscribe) {
                            // Nothing to attach to: say why and leave cleanly.
                            self.farewell = Some(sentence);
                            self.should_quit = true;
                        } else {
                            self.error(sentence);
                        }
                    }
                    (None, None) => {}
                }
            }
            ServerFrame::Event { change } => self.on_change(change),
            ServerFrame::Stream { thread_id, delta } => {
                if let Some(t) = self.thread_by_id(&thread_id).filter(|t| t.loaded) {
                    match delta {
                        StreamDelta::Start => t.start_text(),
                        StreamDelta::Text { text } => t.push_text_delta(&text),
                    }
                }
            }
            // The transcript client never attaches to an agent's own interface.
            ServerFrame::Pty { .. } | ServerFrame::PtyEnded { .. } => {}
        }
    }

    fn on_reply(&mut self, pending: Pending, reply: Reply) {
        match (pending, reply) {
            (Pending::Subscribe, Reply::Workspace(ws)) => {
                self.projects = ws.projects;
                self.agents = ws.agents;
                self.threads = ws.threads.into_iter().map(Thread::new).collect();
                if self.threads.is_empty() {
                    self.threads.push(placeholder());
                }
                self.current = 0;
                self.sort_threads();
                self.attached = true;
                if let Some(v) = &ws.view {
                    self.restore_view(v);
                }
                if let Some(i) = ws.focus.as_deref().and_then(|id| self.threads.iter().position(|t| t.id() == id)) {
                    self.current = i;
                }
                self.enter_thread();
            }
            (Pending::Open(id), Reply::Thread { thread, events, partial }) => {
                if let Some(t) = self.thread_by_id(&id) {
                    t.meta = *thread;
                    t.load(&events, partial.as_deref());
                }
            }
            (Pending::View(id), Reply::Viewing { partial, .. }) => {
                if let Some(t) = self.thread_by_id(&id) {
                    t.set_partial(partial.as_deref());
                }
            }
            (Pending::SwitchAsk { thread_id, to }, Reply::Brief(brief)) => self.ask_switch(&thread_id, &to, &brief),
            (Pending::ShowBrief { thread_id }, Reply::Brief(brief)) => {
                if let Some(t) = self.thread_by_id(&thread_id) {
                    let label = if brief.needed {
                        format!("brief desl would send to {} now", brief.to)
                    } else {
                        format!("{} has seen everything; a switch would send nothing. The brief of the whole thread", brief.to)
                    };
                    t.items.push(Item::Brief { label, text: brief.text, tokens: brief.token_estimate, expanded: true });
                    t.follow = true;
                }
            }
            (Pending::Created, Reply::Created { thread_id }) => {
                match self.threads.iter().position(|t| t.id() == thread_id) {
                    Some(i) => self.select(i),
                    None => self.jump_to = Some(thread_id),
                }
                self.focus = Focus::Input;
            }
            (Pending::Files, Reply::Files { files }) => self.show_file_picker(files),
            (Pending::History, Reply::History { prompts }) => {
                let mut seen = std::collections::HashSet::new();
                let entries: Vec<PickEntry> = prompts
                    .into_iter()
                    .filter(|p| !p.text.starts_with('/') && seen.insert(p.text.clone()))
                    .map(|p| {
                        let one: String = p.text.split_whitespace().collect::<Vec<_>>().join(" ");
                        PickEntry { key: p.text, label: one.chars().take(70).collect(), hint: p.thread_id }
                    })
                    .collect();
                if entries.is_empty() {
                    self.info("No prompts yet in this workspace.");
                } else {
                    self.overlay = Overlay::Picker(Picker::new("history", entries), PickerKind::History);
                }
            }
            (Pending::ModelPicker, Reply::Models { agent, models, current, effort }) => {
                let at = models.iter().position(|m| Some(&m.id) == current.as_ref()).unwrap_or(0);
                let entries: Vec<PickEntry> = models
                    .iter()
                    .map(|m| {
                        let now = Some(&m.id) == current.as_ref();
                        let hint = match (now, &effort) {
                            (true, Some(level)) => format!("✓ current · {level} effort · {}", m.description),
                            (true, None) => format!("✓ current · {}", m.description),
                            (false, _) => m.description.clone(),
                        };
                        PickEntry { hint, label: m.name.clone(), key: m.id.clone() }
                    })
                    .collect();
                let picker = Picker::new(format!("{agent} models"), entries)
                    .starting_at(at)
                    .with_note("enter picks · it takes effect with your next prompt");
                self.overlay = Overlay::Picker(picker, PickerKind::Models);
                self.model_list = models;
                self.effort_choice = effort;
            }
            (Pending::ModelSet, Reply::ModelSet { agent, model, effort }) => {
                self.effort_choice = effort.clone();
                let model = model.filter(|m| !m.default).map(|m| m.name);
                // Short enough to be read whole in the status bar of an 80-column terminal.
                self.info(match (model, effort) {
                    (Some(m), Some(e)) => format!("{m} at {e} effort from your next prompt."),
                    (Some(m), None) => format!("{agent} will use {m} from your next prompt."),
                    (None, Some(e)) => format!("{agent} is at {e} effort from your next prompt."),
                    (None, None) => format!("{agent} is on its default model and effort."),
                });
            }
            (Pending::ModeSet, Reply::ModeSet { mode, meaning }) => self.info(format!("{}: {meaning}.", mode.word())),
            (Pending::ModelThenEffort { thread_id, effort }, Reply::ModelSet { .. }) => {
                self.request(Command::SetEffort { thread_id, effort: Some(effort) }, Pending::ModelSet);
            }
            (Pending::Say(text), _) => self.info(text),
            _ => {}
        }
    }

    fn on_change(&mut self, change: Change) {
        match change {
            Change::Project { project } => {
                if !self.projects.iter().any(|p| p.id == project.id) {
                    self.projects.push(project);
                    self.sort_threads();
                }
            }
            Change::Thread { thread } => {
                let id = thread.id.clone();
                match self.threads.iter_mut().find(|t| t.id() == id) {
                    Some(t) => {
                        let asked = t.meta.pending_approval.is_none() && thread.pending_approval.is_some();
                        t.meta = *thread;
                        // One bell when a thread starts waiting on the user, never a repeat.
                        if asked {
                            bell();
                        }
                    }
                    None => {
                        self.threads.retain(|t| !t.id().is_empty());
                        let mut t = Thread::new(*thread);
                        // A thread born while this client watches has no history to fetch.
                        t.loaded = t.meta.head == 0;
                        self.threads.push(t);
                        self.sort_threads();
                    }
                }
                if self.jump_to.as_deref() == Some(id.as_str())
                    && let Some(i) = self.threads.iter().position(|t| t.id() == id)
                {
                    self.jump_to = None;
                    self.select(i);
                }
            }
            Change::ThreadRemoved { thread_id } => {
                let current = self.thread().id().to_string();
                self.threads.retain(|t| t.id() != thread_id);
                if self.threads.is_empty() {
                    self.threads.push(placeholder());
                }
                match self.threads.iter().position(|t| t.id() == current) {
                    Some(i) => self.current = i,
                    None => {
                        self.current = self.current.min(self.threads.len() - 1);
                        self.enter_thread();
                    }
                }
            }
            Change::Appended { event } => {
                if let Some(t) = self.thread_by_id(&event.thread_id).filter(|t| t.loaded) {
                    t.apply(&event);
                }
            }
            Change::Agents { agents } => self.agents = agents,
            Change::Bye => {
                self.farewell = Some(
                    "The desl daemon was stopped. Every thread is kept; run desl again to start it and resume the native sessions.".into(),
                );
                self.should_quit = true;
            }
        }
    }

    /// The current thread changed: fetch its events once, and move the token stream to it.
    fn enter_thread(&mut self) {
        let t = self.thread();
        let id = t.id().to_string();
        if id.is_empty() {
            return;
        }
        if t.loaded {
            self.request(Command::View { thread_id: id.clone() }, Pending::View(id));
        } else {
            self.request(Command::OpenThread { thread_id: id.clone() }, Pending::Open(id));
        }
    }

    fn select(&mut self, idx: usize) {
        if idx < self.threads.len() && idx != self.current {
            self.current = idx;
            self.enter_thread();
        }
    }

    fn view_state(&self) -> Value {
        let threads: serde_json::Map<String, Value> =
            self.threads.iter().map(|t| (t.id().to_string(), json!({ "scroll": t.scroll, "follow": t.follow }))).collect();
        json!({
            "current": self.thread().id(),
            "sidebar": self.sidebar_visible,
            "focus": match self.focus { Focus::Input => "input", Focus::Transcript => "transcript", Focus::Sidebar => "sidebar" },
            "threads": threads,
        })
    }

    fn restore_view(&mut self, v: &Value) {
        if let Some(s) = v.get("sidebar").and_then(Value::as_bool) {
            self.sidebar_visible = s;
        }
        self.focus = match v.get("focus").and_then(Value::as_str) {
            Some("transcript") => Focus::Transcript,
            Some("sidebar") if self.sidebar_visible => Focus::Sidebar,
            _ => Focus::Input,
        };
        if let Some(map) = v.get("threads").and_then(Value::as_object) {
            for t in self.threads.iter_mut() {
                if let Some(s) = map.get(t.id()) {
                    t.scroll = s.get("scroll").and_then(Value::as_u64).unwrap_or(0) as usize;
                    t.follow = s.get("follow").and_then(Value::as_bool).unwrap_or(true);
                }
            }
        }
    }

    /// Quitting detaches: the daemon and its agents keep running.
    fn detach(&mut self) {
        let view = self.view_state();
        self.request(Command::SaveView { view }, Pending::Quiet);
        let n = self.threads.iter().filter(|t| !t.id().is_empty()).count();
        let working = self.threads.iter().filter(|t| t.busy()).count();
        let mut msg = format!("Detached. {n} thread{} kept", if n == 1 { "" } else { "s" });
        if working > 0 {
            msg.push_str(&format!(", {working} still working"));
        }
        msg.push_str(". Run desl to reattach; `desl daemon stop` stops the agents.");
        self.farewell = Some(msg);
        self.should_quit = true;
    }

    // ---- keys ----------------------------------------------------------------------------------

    fn on_key(&mut self, k: KeyEvent) {
        if let Some(n) = &self.notice
            && n.level == Level::Info
        {
            self.notice = None;
        }
        if matches!(self.overlay, Overlay::Picker(..)) {
            self.picker_key(k);
            return;
        }
        if matches!(self.overlay, Overlay::Help) {
            match k.code {
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') | KeyCode::Enter => self.overlay = Overlay::None,
                KeyCode::Down | KeyCode::Char('j') => self.help_scroll += 1,
                KeyCode::Up | KeyCode::Char('k') => self.help_scroll = self.help_scroll.saturating_sub(1),
                KeyCode::PageDown => self.help_scroll += 10,
                KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(10),
                _ => {}
            }
            return;
        }
        if self.prefix_pending {
            self.prefix_pending = false;
            if k.code == KeyCode::Esc {
                return;
            }
            match keymap::lookup(&k, Ctx::Workspace, true) {
                Some(b) => self.run(b),
                None => self.info(format!(
                    "{} {} is not a key. Press {} ? for the list.",
                    keymap::PREFIX_LABEL,
                    keymap::event_label(&k),
                    keymap::PREFIX_LABEL
                )),
            }
            return;
        }
        if keymap::is_prefix(&k) {
            self.prefix_pending = true;
            return;
        }
        if let Some(n) = keymap::ctrl_digit(&k) {
            self.jump_to(n - 1);
            return;
        }

        let focused = match self.focus {
            Focus::Input => Ctx::Prompt,
            Focus::Transcript => Ctx::Transcript,
            Focus::Sidebar => Ctx::Sidebar,
        };
        if let Some(b) = keymap::lookup(&k, focused, false) {
            self.run(b);
            return;
        }
        if self.focus == Focus::Input {
            if let KeyCode::Char(c) = k.code
                && !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            {
                if self.input.is_empty() {
                    if self.strip_pending()
                        && let Some(b) = keymap::lookup(&k, Ctx::Approval, false)
                    {
                        self.run(b);
                        return;
                    }
                    if let Some(b) = keymap::lookup(&k, Ctx::Workspace, false) {
                        self.run(b);
                        return;
                    }
                }
                self.input.insert(c);
                return;
            }
            if self.input.handle(&k) {
                return;
            }
        } else if self.strip_pending()
            && let Some(b) = keymap::lookup(&k, Ctx::Approval, false)
        {
            self.run(b);
            return;
        }
        if let Some(b) = keymap::lookup(&k, Ctx::Workspace, false) {
            self.run(b);
        }
    }

    fn run(&mut self, b: &'static Binding) {
        if !b.built {
            self.info(format!("{} ({}) is not built yet. It arrives with panes and worktrees.", b.desc, keymap::binding_label(b)));
            return;
        }
        match b.action {
            Action::Palette if self.input.is_empty() => self.open_palette(),
            Action::Palette => self.input.insert('/'),
            Action::FilePicker => self.open_file_picker(),
            Action::History => self.request(Command::History { limit: 500 }, Pending::History),
            Action::Send if self.focus == Focus::Sidebar => self.focus = Focus::Input,
            Action::Send => self.submit(),
            Action::Newline => self.input.insert('\n'),
            Action::Interrupt => self.interrupt(),
            Action::Back => self.back(),
            Action::Up if self.focus == Focus::Sidebar => self.jump_to(self.current.saturating_sub(1)),
            Action::Down if self.focus == Focus::Sidebar => self.jump_to(self.current + 1),
            Action::Up if self.focus == Focus::Transcript => self.move_cursor(-1),
            Action::Down if self.focus == Focus::Transcript => self.move_cursor(1),
            Action::Up => self.scroll_by(-1),
            Action::Down => self.scroll_by(1),
            Action::PageUp => self.scroll_by(-(self.view.transcript_height.max(2) as isize - 1)),
            Action::PageDown => self.scroll_by(self.view.transcript_height.max(2) as isize - 1),
            Action::Top => {
                let t = self.thread_mut();
                t.follow = false;
                t.scroll = 0;
                if !t.items.is_empty() {
                    t.cursor = Some(0);
                }
            }
            Action::Bottom => {
                let t = self.thread_mut();
                t.follow = true;
                t.cursor = t.items.len().checked_sub(1);
            }
            Action::Expand => self.expand_selected(),
            Action::OpenFile => self.open_selected(),
            Action::NextPane => {
                self.focus = match self.focus {
                    Focus::Input => Focus::Transcript,
                    Focus::Transcript if self.sidebar_visible => Focus::Sidebar,
                    Focus::Transcript | Focus::Sidebar => Focus::Input,
                };
                self.focus_changed();
            }
            Action::PrevPane => {
                self.focus = match self.focus {
                    Focus::Input if self.sidebar_visible => Focus::Sidebar,
                    Focus::Input | Focus::Sidebar => Focus::Transcript,
                    Focus::Transcript => Focus::Input,
                };
                self.focus_changed();
            }
            Action::NewThread => self.new_thread(),
            Action::JumpThread => self.open_thread_picker(),
            Action::Help => {
                self.overlay = Overlay::Help;
                self.help_scroll = 0;
            }
            Action::ToggleSidebar => {
                self.sidebar_visible = !self.sidebar_visible;
                if !self.sidebar_visible && self.focus == Focus::Sidebar {
                    self.focus = Focus::Input;
                }
            }
            Action::Switch => self.open_agent_picker(),
            Action::Fork => self.fork(""),
            Action::Detach => self.detach(),
            Action::Approve => {
                if self.thread().ask.is_some() {
                    self.ask_answer(true);
                } else {
                    self.answer(Decision::Allow);
                }
            }
            Action::Deny => {
                if self.thread().ask.is_some() {
                    self.ask_answer(false);
                } else {
                    self.answer(Decision::Deny);
                }
            }
            Action::Always => {
                if self.thread().ask.is_some() {
                    self.ask_answer(true);
                } else {
                    self.answer(Decision::Always);
                }
            }
            _ => {}
        }
    }

    fn focus_changed(&mut self) {
        let transcript = self.focus == Focus::Transcript;
        let t = self.thread_mut();
        if transcript {
            if t.cursor.is_none() {
                t.cursor = t.items.len().checked_sub(1);
            }
        } else {
            t.cursor = None;
        }
    }

    fn back(&mut self) {
        if self.notice.as_ref().is_some_and(|n| n.level == Level::Error) {
            self.notice = None;
        } else if self.focus == Focus::Sidebar {
            self.focus = Focus::Input;
        } else if self.focus == Focus::Transcript {
            let t = self.thread_mut();
            t.follow = true;
            t.cursor = None;
            self.focus = Focus::Input;
        } else if self.thread().ask.is_some() {
            self.ask_answer(false);
        } else if !self.thread().follow {
            self.thread_mut().follow = true;
        }
    }

    fn scroll_by(&mut self, delta: isize) {
        let height = self.view.transcript_height.max(1);
        let total = self.view.transcript_lines;
        let max = total.saturating_sub(height);
        let t = self.thread_mut();
        let from = if t.follow { max } else { t.scroll.min(max) };
        let to = (from as isize + delta).clamp(0, max as isize) as usize;
        if to >= max {
            t.follow = true;
            t.scroll = max;
        } else {
            t.follow = false;
            t.scroll = to;
        }
    }

    /// j/k in the transcript select items, lazygit style; the view keeps the selection visible.
    fn move_cursor(&mut self, delta: isize) {
        let t = self.thread_mut();
        if t.items.is_empty() {
            return;
        }
        let last = t.items.len() - 1;
        let cur = t.cursor.unwrap_or(last) as isize;
        let next = (cur + delta).clamp(0, last as isize) as usize;
        t.cursor = Some(next);
        t.follow = next == last;
    }

    fn expand_selected(&mut self) {
        let t = self.thread_mut();
        let Some(i) = t.cursor else { return };
        match t.items.get_mut(i) {
            Some(item) if item.expandable() => item.toggle(),
            Some(_) => self.info("Only tool calls and briefs expand. j and k move between them."),
            None => {}
        }
    }

    /// `o`: the selected read or edit opens in `$EDITOR`.
    fn open_selected(&mut self) {
        let t = self.thread();
        let Some(path) = t.cursor.and_then(|i| t.items.get(i)).and_then(Item::path) else {
            self.info("Select a read or an edit with j and k, then press o to open its file.");
            return;
        };
        if std::env::var_os("EDITOR").is_none_or(|e| e.is_empty()) {
            self.info("Set $EDITOR to open files from here.");
            return;
        }
        let path = PathBuf::from(path);
        self.open_in_editor = Some(if path.is_absolute() { path } else { PathBuf::from(&t.meta.cwd).join(path) });
    }

    fn interrupt(&mut self) {
        let twice = self.last_ctrl_c.is_some_and(|t| t.elapsed() < Duration::from_millis(1500));
        self.last_ctrl_c = Some(Instant::now());
        let working = self.thread().busy();
        if working && !twice {
            let thread_id = self.thread().id().to_string();
            self.request(Command::Interrupt { thread_id }, Pending::Quiet);
        } else if !self.input.is_empty() {
            self.input.clear();
        } else if !working {
            self.info("Nothing to interrupt. To leave, type /quit; agents keep running.");
        }
    }

    // ---- threads -------------------------------------------------------------------------------

    pub fn new_thread(&mut self) {
        let cwd = Some(self.thread().meta.cwd.clone()).filter(|c| !c.is_empty()).unwrap_or_else(|| self.cwd.to_string_lossy().to_string());
        self.request(Command::NewThread { cwd, agent: None, prompt: None }, Pending::Created);
    }

    pub fn jump_to(&mut self, idx: usize) {
        if idx < self.threads.len() {
            self.select(idx);
        } else {
            self.info(format!("There is no thread {}. ctrl+n opens one.", idx + 1));
        }
    }

    fn open_thread_picker(&mut self) {
        let entries = self
            .threads
            .iter()
            .enumerate()
            .map(|(i, t)| PickEntry {
                key: i.to_string(),
                label: format!("{} {}", t.id(), if t.name().is_empty() { "new" } else { t.name() }),
                hint: format!("{} {} · {}", t.status().glyph(), t.agent(), t.meta.project_id),
            })
            .collect();
        self.overlay = Overlay::Picker(Picker::new("threads", entries), PickerKind::Threads);
    }

    fn picker_key(&mut self, k: KeyEvent) {
        let Overlay::Picker(p, kind) = &mut self.overlay else { return };
        let kind = *kind;
        match (k.code, k.modifiers.contains(KeyModifiers::CONTROL)) {
            (KeyCode::Esc, _) | (KeyCode::Char('c'), true) => self.overlay = Overlay::None,
            (KeyCode::Enter, _) => {
                let typed = p.query.trim().to_string();
                let chosen = if kind == PickerKind::Commands && typed.contains(' ') {
                    Some(format!("/{}", typed.trim_start_matches('/')))
                } else {
                    p.current().map(|e| e.key.clone())
                };
                self.overlay = Overlay::None;
                if let Some(key) = chosen {
                    self.picker_choose(kind, &key);
                }
            }
            (KeyCode::Down, _) | (KeyCode::Char('n'), true) | (KeyCode::Char('j'), true) => p.down(),
            (KeyCode::Up, _) | (KeyCode::Char('p'), true) | (KeyCode::Char('k'), true) => p.up(),
            (KeyCode::Backspace, _) => p.backspace(),
            (KeyCode::Char('u'), true) => {
                p.query.clear();
                p.selected = 0;
            }
            (KeyCode::Char(c), false) => p.type_char(c),
            _ => {}
        }
    }

    fn picker_choose(&mut self, kind: PickerKind, key: &str) {
        match kind {
            PickerKind::Threads => {
                if let Ok(i) = key.parse::<usize>() {
                    self.jump_to(i);
                    self.focus = Focus::Input;
                }
            }
            PickerKind::Agents => self.switch_to(key),
            PickerKind::Models => {
                let thread_id = self.thread().id().to_string();
                self.request(Command::SetModel { thread_id, model: Some(key.to_string()) }, Pending::ModelSet);
                // Then how hard it should think, the way Codex asks after a model is picked.
                self.open_effort_picker(key);
            }
            PickerKind::Modes => self.set_mode(key),
            PickerKind::Efforts => {
                let thread_id = self.thread().id().to_string();
                let effort = Some(key.to_string()).filter(|k| k != DEFAULT_EFFORT);
                self.request(Command::SetEffort { thread_id, effort }, Pending::ModelSet);
            }
            PickerKind::Commands => {
                // A full command typed into the palette runs as typed; a bare name runs or waits for its argument.
                if key.starts_with('/') && key.contains(' ') {
                    if !self.slash(key) {
                        self.info(format!("{key} is not a desl command; type it in the prompt to send it to the agent."));
                    }
                } else if let Some(c) = COMMANDS.iter().find(|c| c.name == key) {
                    if !c.built {
                        self.info(format!("{} is not built yet. It arrives with panes and worktrees.", c.name));
                    } else if c.args.is_empty() || c.args.starts_with('[') {
                        self.slash(c.name);
                    } else {
                        self.input.insert_str(&format!("{} ", c.name));
                    }
                }
            }
            PickerKind::Files => {
                if !self.input.is_empty() && !self.input.as_string().ends_with(' ') {
                    self.input.insert(' ');
                }
                self.input.insert_str(&format!("@{key} "));
            }
            PickerKind::History => {
                self.input.clear();
                self.input.insert_str(key);
            }
        }
    }

    /// What the thread's agent may do without asking, with the mode in use marked. Each row
    /// says what the mode means with this agent, which is not the same for one that cannot ask.
    fn open_mode_picker(&mut self) {
        let t = self.thread();
        let (now, agent) = (t.meta.permission_mode, self.agents.iter().find(|a| a.id == t.agent()));
        let entries: Vec<PickEntry> = PermissionMode::CHOICES
            .into_iter()
            .map(|mode| {
                let meaning = agent.map_or_else(|| mode.meaning().to_string(), |a| a.mode_meaning(mode));
                PickEntry {
                    key: mode.word().to_string(),
                    label: mode.word().to_string(),
                    hint: if mode == now { format!("✓ current · {meaning}") } else { meaning },
                }
            })
            .collect();
        let at = PermissionMode::CHOICES.iter().position(|m| *m == now).unwrap_or(0);
        let picker = Picker::new("permissions", entries).starting_at(at).with_note("enter picks · for this thread only");
        self.overlay = Overlay::Picker(picker, PickerKind::Modes);
    }

    fn set_mode(&mut self, word: &str) {
        let word = word.trim().to_lowercase();
        match PermissionMode::parse(&word) {
            Some(mode) => {
                let thread_id = self.thread().id().to_string();
                self.request(Command::SetMode { thread_id, mode }, Pending::ModeSet);
            }
            None => self.error(format!("There is no permission mode named {word}. Pick one of ask, edits or yolo.")),
        }
    }

    /// The effort levels of the model just picked, with the one in use marked. The first row
    /// is not choosing: the agent's own setting, named where the agent says what it is.
    fn open_effort_picker(&mut self, model_id: &str) {
        let Some(m) = self.model_list.iter().find(|m| m.id == model_id).filter(|m| !m.efforts.is_empty()) else { return };
        let agent = self.thread().agent().to_string();
        let chosen = self.effort_choice.as_ref().filter(|c| m.efforts.iter().any(|e| &e.id == *c));
        let mark = |now: bool, text: String| match (now, text.is_empty()) {
            (true, true) => "✓ current".to_string(),
            (true, false) => format!("✓ current · {text}"),
            (false, _) => text,
        };
        let own = match &m.default_effort {
            Some(level) => format!("{agent}'s own setting, now {level}"),
            None => format!("{agent}'s own setting"),
        };
        let mut entries =
            vec![PickEntry { key: DEFAULT_EFFORT.to_string(), label: DEFAULT_EFFORT.to_string(), hint: mark(chosen.is_none(), own) }];
        entries.extend(m.efforts.iter().map(|e| PickEntry {
            key: e.id.clone(),
            label: e.id.clone(),
            hint: mark(chosen == Some(&e.id), e.description.clone()),
        }));
        let at = chosen.and_then(|c| m.efforts.iter().position(|e| &e.id == c)).map_or(0, |i| i + 1);
        let picker =
            Picker::new(format!("{} effort", m.name), entries).starting_at(at).with_note("enter picks · esc keeps the effort as it is");
        self.overlay = Overlay::Picker(picker, PickerKind::Efforts);
    }

    fn open_palette(&mut self) {
        let entries = COMMANDS
            .iter()
            .map(|c| PickEntry {
                key: c.name.to_string(),
                label: if c.args.is_empty() { c.name.to_string() } else { format!("{} {}", c.name, c.args) },
                hint: if c.built { c.desc.to_string() } else { format!("{} · not yet", c.desc) },
            })
            .collect();
        self.overlay = Overlay::Picker(Picker::new("commands", entries), PickerKind::Commands);
    }

    fn open_file_picker(&mut self) {
        let thread_id = self.thread().id().to_string();
        self.request(Command::Files { thread_id }, Pending::Files);
    }

    fn show_file_picker(&mut self, files: Vec<String>) {
        if files.is_empty() {
            self.info("No files to pick: this directory is not a git repository, or it is empty. Type a path instead.");
            self.input.insert('@');
            return;
        }
        let entries = files
            .into_iter()
            .map(|f| {
                let (dir, name) = f.rsplit_once('/').map(|(d, n)| (d.to_string(), n.to_string())).unwrap_or((String::new(), f.clone()));
                PickEntry { key: f, label: name, hint: dir }
            })
            .collect();
        self.overlay = Overlay::Picker(Picker::new("files", entries), PickerKind::Files);
    }

    // ---- switch and fork -----------------------------------------------------------------------

    fn agent_names(&self) -> String {
        self.agents.iter().map(|a| a.id.as_str()).collect::<Vec<_>>().join(" and ")
    }

    /// `/switch` with no name: every agent desl drives, the current one marked, and why an
    /// agent cannot be chosen when it cannot. Nobody has to remember what is installed.
    fn open_agent_picker(&mut self) {
        let t = self.thread();
        let mut likely = None;
        let entries: Vec<PickEntry> = self
            .agents
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let d = &a.detection;
                let model = t.meta.bindings.get(&a.id).and_then(|b| b.model_choice.clone().or_else(|| b.model.clone()));
                let hint = if a.id == t.agent() {
                    match model {
                        Some(m) => format!("✓ current · {m}"),
                        None => "✓ current".to_string(),
                    }
                } else if !d.installed {
                    "not installed".to_string()
                } else if d.logged_in == Some(false) {
                    "not logged in".to_string()
                } else {
                    likely.get_or_insert(i);
                    match (model, &d.version) {
                        (Some(m), _) => m,
                        (None, Some(v)) => format!("ready · {v}"),
                        (None, None) => "ready".to_string(),
                    }
                };
                PickEntry { key: a.id.clone(), label: a.id.clone(), hint }
            })
            .collect();
        let picker =
            Picker::new("agents", entries).starting_at(likely.unwrap_or(0)).with_note("enter continues this thread with that agent");
        self.overlay = Overlay::Picker(picker, PickerKind::Agents);
    }

    fn switch_to(&mut self, to: &str) {
        let to = to.trim().to_string();
        let t = self.thread();
        if t.agent() == to {
            return self.info(format!("This thread is already on {to}."));
        }
        let Some(agent) = self.agents.iter().find(|a| a.id == to) else {
            return self.info(format!("desl drives {}, not {to}. /switch alone lists them.", self.agent_names()));
        };
        if !agent.detection.installed {
            return self.info(format!("{to} is not installed. {}", agent.detection.install_hint));
        }
        if agent.detection.logged_in == Some(false) {
            return self.info(format!("{to} is not logged in. {}", agent.detection.login_hint));
        }
        if t.busy() {
            return self.info(format!("{} is still working. Wait for it to finish or press ctrl+c, then switch.", t.agent()));
        }
        if t.ask.is_some() {
            return;
        }
        // What would be handed over decides what to ask; the daemon knows.
        let thread_id = t.id().to_string();
        self.request(Command::Brief { thread_id: thread_id.clone(), to: Some(to.clone()) }, Pending::SwitchAsk { thread_id, to });
    }

    fn ask_switch(&mut self, thread_id: &str, to: &str, brief: &BriefPreview) {
        if !brief.needed {
            // The other agent has seen everything: nothing to hand over, nothing to confirm.
            self.request(Command::Switch { thread_id: thread_id.to_string(), agent: to.to_string() }, Pending::Quiet);
            return;
        }
        let turns = brief.turns.max(1);
        let s = if turns == 1 { "" } else { "s" };
        let text = if brief.delta {
            format!("Handing {turns} new turn{s} to {to} as a delta brief ({}). Continue?", fmt_tokens(brief.token_estimate))
        } else {
            format!("Handing {turns} turn{s} to {to} as a brief ({}). Continue?", fmt_tokens(brief.token_estimate))
        };
        if let Some(t) = self.thread_by_id(thread_id) {
            t.ask = Some(Ask { text, kind: AskKind::Switch(to.to_string()) });
        }
    }

    fn ask_answer(&mut self, yes: bool) {
        let Some(ask) = self.thread_mut().ask.take() else { return };
        match ask.kind {
            AskKind::Switch(to) => {
                if yes {
                    let thread_id = self.thread().id().to_string();
                    self.request(Command::Switch { thread_id, agent: to }, Pending::Quiet);
                }
            }
        }
    }

    /// `/fork [agent] [@n]`: a new thread with this one's history, up to message n when given.
    fn fork(&mut self, arg: &str) {
        let t = self.thread();
        let mut agent = None;
        let mut at = None;
        for word in arg.split_whitespace() {
            if word == "--worktree" {
                return self
                    .info("--worktree is not built yet; it arrives with worktrees. /fork without it shares this thread's directory.");
            } else if let Some(n) = word.strip_prefix('@') {
                match n.parse::<usize>().ok().filter(|n| *n >= 1).and_then(|n| t.messages.get(n - 1)) {
                    Some(index) => at = Some(*index),
                    None => return self.info(format!("This thread has messages 1 to {}; @{n} is not one of them.", t.messages.len())),
                }
            } else {
                agent = Some(word.to_string());
            }
        }
        let thread_id = t.id().to_string();
        self.request(Command::Fork { thread_id, agent, at }, Pending::Created);
    }

    // ---- prompt --------------------------------------------------------------------------------

    fn submit(&mut self) {
        let text = self.input.take().trim().to_string();
        if text.is_empty() {
            return;
        }
        if text.starts_with('/') && self.slash(&text) {
            return;
        }
        let thread_id = self.thread().id().to_string();
        self.thread_mut().follow = true;
        self.request(Command::Prompt { thread_id, text }, Pending::Quiet);
    }

    /// Returns true when desl handled the command itself; false forwards it to the agent.
    fn slash(&mut self, text: &str) -> bool {
        let mut parts = text.splitn(2, ' ');
        let cmd = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("").trim().to_string();
        let thread_id = self.thread().id().to_string();
        match cmd {
            "/quit" | "/exit" | "/q" | "/detach" => self.detach(),
            "/help" => {
                self.overlay = Overlay::Help;
                self.help_scroll = 0;
            }
            "/info" => {
                let t = self.thread();
                let mut text = format!(
                    "{} · on {} · {} turns · ${:.2} · permissions {}",
                    t.id(),
                    t.agent(),
                    t.turns,
                    t.cost_usd,
                    t.meta.permission_mode.word()
                );
                for b in t.meta.bindings.values() {
                    text.push_str(&format!(
                        "\n{}: model {}{} · session {} · seen {} of {} events",
                        b.agent,
                        b.model.clone().unwrap_or_else(|| "unknown".into()),
                        b.effort_choice.as_ref().map(|e| format!(" at {e} effort")).unwrap_or_default(),
                        b.session_id.clone().unwrap_or_else(|| "not started".into()),
                        b.seen_up_to,
                        t.meta.head
                    ));
                }
                let t = self.thread_mut();
                t.items.push(Item::Note(text));
                t.follow = true;
            }
            // Named, it switches; bare, it shows what there is to switch to.
            "/switch" if arg.is_empty() => self.open_agent_picker(),
            "/switch" => self.switch_to(&arg),
            "/fork" => self.fork(&arg),
            "/brief" => self.request(Command::Brief { thread_id: thread_id.clone(), to: None }, Pending::ShowBrief { thread_id }),
            "/new" => self.new_thread(),
            "/threads" | "/resume" => self.open_thread_picker(),
            "/rename" => {
                if arg.is_empty() {
                    self.info("Give the thread a name: /rename <name>.");
                } else {
                    self.request(Command::Rename { thread_id, name: arg }, Pending::Quiet);
                }
            }
            "/clear" => {
                let agent = self.thread().agent().to_string();
                self.request(
                    Command::Clear { thread_id },
                    Pending::Say(format!("Fresh context. The history stays here; {agent} starts clean.")),
                );
            }
            // Named, it sets the model; bare, it shows the agent's models with the current one marked.
            "/model" if arg.is_empty() => self.request(Command::Models { thread_id }, Pending::ModelPicker),
            // `/model sonnet high`: a second word is the effort.
            "/model" => match arg.split_once(char::is_whitespace) {
                Some((model, effort)) => {
                    let then = Pending::ModelThenEffort { thread_id: thread_id.clone(), effort: effort.trim().to_string() };
                    self.request(Command::SetModel { thread_id, model: Some(model.to_string()) }, then);
                }
                None => self.request(Command::SetModel { thread_id, model: Some(arg) }, Pending::ModelSet),
            },
            // Named, it sets the mode; bare, it shows the modes with the current one marked.
            "/permissions" if arg.is_empty() => self.open_mode_picker(),
            "/permissions" => self.set_mode(&arg),
            "/export" => self.export(&arg),
            "/compact" | "/diff" | "/pr" | "/worktree" => {
                self.info(format!("{cmd} is not built yet. It arrives with panes and worktrees."));
            }
            _ => return false,
        }
        true
    }

    /// `/export [file]`: the thread as markdown.
    fn export(&mut self, arg: &str) {
        let t = self.thread();
        let mut out = format!("# {} ({})\n", if t.name().is_empty() { "thread" } else { t.name() }, t.id());
        for item in &t.items {
            match item {
                Item::User { text } => out.push_str(&format!("\n## User\n\n{}\n", text.trim())),
                Item::Assistant { text, .. } => out.push_str(&format!("\n{}\n", text.trim())),
                Item::Tool { verb, target, outcome, .. } => {
                    let o = outcome.as_ref().map(|o| format!(" {}", o.summary)).unwrap_or_default();
                    out.push_str(&format!("\n- `{verb} {target}`{o}\n"));
                }
                Item::Divider(label) => out.push_str(&format!("\n---\n\n*{label}*\n")),
                Item::Error(text) => out.push_str(&format!("\n> {text}\n")),
                Item::Brief { .. } | Item::Note(_) => {}
            }
        }
        let name = if arg.is_empty() { format!("{}.md", t.id().replace('/', "-")) } else { arg.to_string() };
        let path = self.cwd.join(name);
        match std::fs::write(&path, out) {
            Ok(()) => self.info(format!("Wrote {}.", path.display())),
            Err(e) => self.error(format!("Could not write {} ({e}). Give /export a path you can write to.", path.display())),
        }
    }

    fn answer(&mut self, decision: Decision) {
        let t = self.thread();
        let Some(call_id) = t.meta.pending_approval.as_ref().map(|a| a.call_id.clone()) else { return };
        let thread_id = t.id().to_string();
        self.request(Command::Answer { thread_id, call_id: Some(call_id), decision }, Pending::Quiet);
    }
}

fn now_hhmm() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

fn bell() {
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x07");
    let _ = out.flush();
}
