//! The client: renders the host's log and turns keys into commands.

use crate::agent::AgentEvent;
use crate::commands::COMMANDS;
use crate::git::{self, GitInfo};
use crate::input::Input;
use crate::keymap::{self, Action, Binding, Ctx};
use crate::link::HostLink;
use crate::log::{Command, Entry, Frame};
use crate::model::{Agent, Ask, AskKind, Item, Status, Thread};
use crate::picker::{Entry as PickEntry, Picker};
use crate::projector;
use crate::workspace::Workspace;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use serde_json::{Value, json};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub enum Msg {
    Term(Event),
    Host(Frame),
    HostGone,
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

pub struct App {
    pub cwd: PathBuf,
    pub project: String,
    pub ws: Workspace,
    pub current: usize,
    host: HostLink,
    pub attached: bool,
    jump_to_new: bool,
    pub input: Input,
    pub focus: Focus,
    pub overlay: Overlay,
    pub prefix_pending: bool,
    pub sidebar_visible: bool,
    pub notice: Option<Notice>,
    pub git: GitInfo,
    git_refreshed: Instant,
    pub should_quit: bool,
    pub farewell: Option<String>,
    last_ctrl_c: Option<Instant>,
    pub dirty: bool,
    pub view: View,
    pub help_scroll: usize,
    pub clock: String,
}

const INFO_TTL: Duration = Duration::from_secs(6);

fn fmt_tokens(n: usize) -> String {
    if n >= 1000 { format!("~{:.1}k tokens", n as f64 / 1000.0) } else { format!("~{n} tokens") }
}

impl App {
    pub fn new(cwd: PathBuf, host: HostLink) -> Self {
        let project = cwd.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "project".into());
        let git = git::read(&cwd);
        let mut ws = Workspace::new(cwd.clone());
        // A placeholder until the host's snapshot arrives, a few milliseconds from now.
        ws.threads.push(Thread::new(Agent::Claude));
        App {
            cwd,
            project,
            ws,
            current: 0,
            host,
            attached: false,
            jump_to_new: false,
            input: Input::default(),
            focus: Focus::Input,
            overlay: Overlay::None,
            prefix_pending: false,
            sidebar_visible: true,
            notice: None,
            git,
            git_refreshed: Instant::now(),
            should_quit: false,
            farewell: None,
            last_ctrl_c: None,
            dirty: true,
            view: View::default(),
            help_scroll: 0,
            clock: now_hhmm(),
        }
    }

    pub fn thread(&self) -> &Thread {
        &self.ws.threads[self.current.min(self.ws.threads.len() - 1)]
    }
    pub fn thread_mut(&mut self) -> &mut Thread {
        let i = self.current.min(self.ws.threads.len() - 1);
        &mut self.ws.threads[i]
    }

    /// Runs on every pass of the loop: times each working turn and keeps the working line moving.
    pub fn pulse(&mut self) {
        for t in &mut self.ws.threads {
            match t.status {
                Status::Working => {
                    t.working_since.get_or_insert_with(Instant::now);
                }
                Status::Waiting => {}
                _ => t.working_since = None,
            }
        }
        if self.thread().status == Status::Working {
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
        if self.git_refreshed.elapsed() > Duration::from_secs(5) {
            self.git = git::read(&self.cwd);
            self.git_refreshed = Instant::now();
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
            Msg::Host(frame) => self.on_frame(frame),
            Msg::HostGone => {
                if !self.should_quit {
                    self.error("Lost the desl host. Type /quit, then run the prototype again to reconnect.");
                }
            }
        }
    }

    fn info(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice { text: text.into(), level: Level::Info, at: Instant::now() });
    }
    fn error(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice { text: text.into(), level: Level::Error, at: Instant::now() });
    }

    pub fn approval_pending(&self) -> bool {
        self.thread().approval.is_some()
    }
    /// A one-line question is waiting above the prompt: an approval or an ask.
    pub fn strip_pending(&self) -> bool {
        self.thread().approval.is_some() || self.thread().ask.is_some()
    }

    // ---- host frames --------------------------------------------------------

    fn on_frame(&mut self, frame: Frame) {
        match frame {
            Frame::Snapshot { entries, view } => {
                let mut ws = Workspace::new(self.cwd.clone());
                for e in &entries {
                    ws.apply(e);
                }
                if ws.threads.is_empty() {
                    ws.threads.push(Thread::new(Agent::Claude));
                }
                self.ws = ws;
                self.attached = true;
                self.current = 0;
                if let Some(v) = view {
                    self.restore_view(&v);
                }
            }
            Frame::Entry(e) => {
                let idx = e.idx();
                let created = matches!(e, Entry::ThreadCreated { .. });
                let approval = matches!(e, Entry::Agent { ev: AgentEvent::ApprovalRequest { .. }, .. });
                self.ws.apply(&e);
                if created && self.jump_to_new {
                    self.jump_to_new = false;
                    self.current = idx.min(self.ws.threads.len() - 1);
                    self.focus = Focus::Input;
                }
                if approval && self.ws.threads.get(idx).is_some_and(|t| t.approval.is_some()) {
                    bell();
                }
            }
            Frame::Bye => {
                self.farewell = Some("The desl host was stopped. Its log is kept, so running the prototype here again restores every thread and resumes the native sessions.".into());
                self.should_quit = true;
            }
        }
    }

    fn view_state(&self) -> Value {
        let threads: serde_json::Map<String, Value> = self
            .ws
            .threads
            .iter()
            .map(|t| (t.id.clone(), json!({ "scroll": t.scroll, "follow": t.follow })))
            .collect();
        json!({
            "current": self.current,
            "sidebar": self.sidebar_visible,
            "focus": match self.focus { Focus::Input => "input", Focus::Transcript => "transcript", Focus::Sidebar => "sidebar" },
            "threads": threads,
        })
    }

    fn restore_view(&mut self, v: &Value) {
        if let Some(c) = v.get("current").and_then(Value::as_u64) {
            self.current = (c as usize).min(self.ws.threads.len() - 1);
        }
        if let Some(s) = v.get("sidebar").and_then(Value::as_bool) {
            self.sidebar_visible = s;
        }
        self.focus = match v.get("focus").and_then(Value::as_str) {
            Some("transcript") => Focus::Transcript,
            Some("sidebar") if self.sidebar_visible => Focus::Sidebar,
            _ => Focus::Input,
        };
        if let Some(map) = v.get("threads").and_then(Value::as_object) {
            for t in self.ws.threads.iter_mut() {
                if let Some(s) = map.get(&t.id) {
                    t.scroll = s.get("scroll").and_then(Value::as_u64).unwrap_or(0) as usize;
                    t.follow = s.get("follow").and_then(Value::as_bool).unwrap_or(true);
                }
            }
        }
        if self.focus == Focus::Transcript {
            self.focus_changed();
        }
    }

    /// Quitting detaches: the host and its agents keep running.
    fn detach(&mut self) {
        self.host.send(&Command::SaveView { view: self.view_state() });
        let n = self.ws.threads.len();
        let working = self.ws.threads.iter().filter(|t| t.busy()).count();
        let mut msg = format!("Detached. {n} thread{} kept", if n == 1 { "" } else { "s" });
        if working > 0 {
            msg.push_str(&format!(", {working} still working"));
        }
        msg.push_str(". Run the prototype here again to reattach; `proto-a stop` stops the host.");
        self.farewell = Some(msg);
        self.should_quit = true;
    }

    // ---- keys -------------------------------------------------------------

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
            match b.action {
                _ => self.info(format!(
                    "{} ({}) is not built yet in this prototype.",
                    b.desc,
                    keymap::binding_label(b)
                )),
            }
            return;
        }
        match b.action {
            Action::Palette if self.input.is_empty() => self.open_palette(),
            Action::Palette => self.input.insert('/'),
            Action::FilePicker => self.open_file_picker(),
            Action::History => self.open_history(),
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
            Action::Switch => {
                let to = self.thread().agent.other();
                self.switch_to(to);
            }
            Action::Detach => self.detach(),
            Action::Approve => {
                if self.thread().ask.is_some() {
                    self.ask_answer(true);
                } else {
                    self.answer(true, false);
                }
            }
            Action::Deny => {
                if self.thread().ask.is_some() {
                    self.ask_answer(false);
                } else {
                    self.answer(false, false);
                }
            }
            Action::Always => {
                if self.thread().ask.is_some() {
                    self.ask_answer(true);
                } else {
                    self.answer(true, true);
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

    fn interrupt(&mut self) {
        let twice = self.last_ctrl_c.is_some_and(|t| t.elapsed() < Duration::from_millis(1500));
        self.last_ctrl_c = Some(Instant::now());
        let working = self.thread().busy();
        if working && !twice {
            let idx = self.current;
            self.host.send(&Command::Interrupt { idx });
        } else if !self.input.is_empty() {
            self.input.clear();
        } else if !working {
            self.info("Nothing to interrupt. To leave, type /quit; agents keep running.");
        }
    }

    // ---- threads -----------------------------------------------------------

    pub fn new_thread(&mut self) {
        self.jump_to_new = true;
        self.host.send(&Command::NewThread);
    }

    pub fn jump_to(&mut self, idx: usize) {
        if idx < self.ws.threads.len() {
            self.current = idx;
        } else {
            self.info(format!("There is no thread {}. ctrl+n opens one.", idx + 1));
        }
    }

    fn open_thread_picker(&mut self) {
        let entries = self
            .ws
            .threads
            .iter()
            .enumerate()
            .map(|(i, t)| PickEntry {
                key: i.to_string(),
                label: format!("{} {}", t.id, if t.name.is_empty() { "new" } else { &t.name }),
                hint: format!("{} {}", t.status.glyph(), t.agent.name()),
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
            PickerKind::Commands => {
                // A full command typed into the palette runs as typed; a bare name runs or waits for its argument.
                if key.starts_with('/') && key.contains(' ') {
                    if !self.slash(key) {
                        self.info(format!("{key} is not a desl command; type it in the prompt to send it to the agent."));
                    }
                } else if let Some(c) = COMMANDS.iter().find(|c| c.name == key) {
                    if !c.built {
                        self.info(format!("{} is not built yet in this prototype.", c.name));
                    } else if c.args.is_empty() {
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
        let mut files: Vec<String> = Vec::new();
        for args in [&["ls-files"][..], &["ls-files", "--others", "--exclude-standard"][..]] {
            if let Ok(out) = std::process::Command::new("git").arg("-C").arg(&self.cwd).args(args).output()
                && out.status.success()
            {
                files.extend(String::from_utf8_lossy(&out.stdout).lines().map(str::to_string));
            }
        }
        if files.is_empty() {
            self.info("No files to pick: this directory is not a git repository, or it is empty. Type a path instead.");
            self.input.insert('@');
            return;
        }
        files.truncate(5000);
        let entries = files
            .into_iter()
            .map(|f| {
                let (dir, name) = f.rsplit_once('/').map(|(d, n)| (d.to_string(), n.to_string())).unwrap_or((String::new(), f.clone()));
                PickEntry { key: f, label: name, hint: dir }
            })
            .collect();
        self.overlay = Overlay::Picker(Picker::new("files", entries), PickerKind::Files);
    }

    fn open_history(&mut self) {
        let mut seen = std::collections::HashSet::new();
        let mut entries: Vec<PickEntry> = Vec::new();
        for t in self.ws.threads.iter().rev() {
            for item in t.items.iter().rev() {
                if let Item::User { text, .. } = item
                    && !text.starts_with('/')
                    && seen.insert(text.clone())
                {
                    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
                    entries.push(PickEntry { key: text.clone(), label: one.chars().take(70).collect(), hint: t.id.clone() });
                }
            }
        }
        if entries.is_empty() {
            self.info("No prompts yet in this workspace.");
            return;
        }
        self.overlay = Overlay::Picker(Picker::new("history", entries), PickerKind::History);
    }

    // ---- switch -------------------------------------------------------------

    fn switch_to(&mut self, to: Agent) {
        let t = self.thread();
        if t.agent == to {
            self.info(format!("This thread is already on {}.", to.name()));
            return;
        }
        if t.busy() {
            self.info(format!("{} is still working. Wait for it to finish or press ctrl+c, then switch.", t.agent.name()));
            return;
        }
        if t.ask.is_some() {
            return;
        }
        let len = t.items.len();
        let (from, delta) = match t.binding(to) {
            Some(b) if b.session_id.is_some() => (b.seen_up_to.min(len), true),
            _ => (0, false),
        };
        let unseen = &t.items[from..];
        if delta && !unseen.iter().any(|i| matches!(i, Item::User { .. } | Item::Assistant { .. } | Item::Tool { .. })) {
            let idx = self.current;
            self.host.send(&Command::Switch { idx, to });
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
        self.thread_mut().ask = Some(Ask { text, kind: AskKind::Switch(to) });
    }

    fn ask_answer(&mut self, yes: bool) {
        let Some(ask) = self.thread_mut().ask.take() else { return };
        match ask.kind {
            AskKind::Switch(to) => {
                if yes {
                    let idx = self.current;
                    self.host.send(&Command::Switch { idx, to });
                }
            }
        }
    }

    // ---- prompt ------------------------------------------------------------

    fn submit(&mut self) {
        let text = self.input.take().trim().to_string();
        if text.is_empty() {
            return;
        }
        if text.starts_with('/') && self.slash(&text) {
            return;
        }
        let idx = self.current;
        self.thread_mut().follow = true;
        self.host.send(&Command::Prompt { idx, text });
    }

    /// Returns true when desl handled the command itself; false forwards it to the agent.
    fn slash(&mut self, text: &str) -> bool {
        let mut parts = text.splitn(2, ' ');
        let cmd = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("").trim().to_string();
        let idx = self.current;
        match cmd {
            "/quit" | "/exit" | "/q" | "/detach" => {
                self.detach();
                true
            }
            "/help" => {
                self.overlay = Overlay::Help;
                self.help_scroll = 0;
                true
            }
            "/info" => {
                let t = self.thread();
                let mut text = format!("{} · on {} · {} turns · ${:.2}", t.id, t.agent.name(), t.turns, t.cost_usd);
                for agent in [Agent::Claude, Agent::Codex] {
                    if let Some(b) = t.binding(agent) {
                        text.push_str(&format!(
                            "\n{}: model {} · session {} · seen {} of {} items",
                            agent.name(),
                            b.model.clone().unwrap_or_else(|| "unknown".into()),
                            b.session_id.clone().unwrap_or_else(|| "not started".into()),
                            b.seen_up_to,
                            t.items.len()
                        ));
                    }
                }
                self.host.send(&Command::Note { idx, text });
                true
            }
            "/switch" => {
                let to = if arg.is_empty() { Some(self.thread().agent.other()) } else { Agent::parse(&arg) };
                match to {
                    Some(to) => self.switch_to(to),
                    None => self.info(format!("desl drives claude and codex. /switch codex or /switch claude, not {arg}.")),
                }
                true
            }
            "/brief" => {
                self.host.send(&Command::ShowBrief { idx });
                true
            }
            "/new" => {
                self.new_thread();
                true
            }
            "/threads" => {
                self.open_thread_picker();
                true
            }
            "/rename" => {
                if arg.is_empty() {
                    self.info("Give the thread a name: /rename <name>.");
                } else {
                    self.host.send(&Command::Rename { idx, name: arg });
                }
                true
            }
            "/fork" | "/resume" | "/model" | "/clear" | "/compact" | "/export" | "/diff" | "/pr" | "/worktree" => {
                self.info(format!("{cmd} is not built yet in this prototype."));
                true
            }
            _ => false,
        }
    }

    fn answer(&mut self, allow: bool, always: bool) {
        let idx = self.current;
        let Some(request_id) = self.thread().approval.as_ref().map(|a| a.request_id.clone()) else { return };
        self.host.send(&Command::Answer { idx, request_id, allow, always });
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
